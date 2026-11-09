use std::fs::{self, ReadDir};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::date_groups::{SectionStarts, section_sql, section_starts};
use super::{
    Cancellation, Item, ItemPage, decode, encode, resolve_directory, stored_size, unix_seconds,
};
use crate::PageRequest;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortColumn {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    pub column: SortColumn,
    pub descending: bool,
    /// Sections by date come first; the column orders the items inside each one.
    pub group: bool,
}

/// One date section of a grouped listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub start: u32,
    pub count: u32,
    /// Newest first: Today is 0, A long time ago is 7.
    pub bucket: u8,
}

/// `FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM`.
const HIDDEN_ITEM: u32 = 0x2 | 0x4;

/// A directory snapshot whose row numbers stay stable while enumeration proceeds.
pub struct Directory {
    path: PathBuf,
    source: Option<ReadDir>,
    db: Connection,
    count: u32,
    sort: Option<Sort>,
    sort_ms: f64,
    hide_hidden: bool,
    /// Set while the ordering is grouped, so the groups always match the rows.
    sections: Option<SectionStarts>,
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let path = resolve_directory(path)?;
        let source = fs::read_dir(&path).with_context(|| format!("List {}", path.display()))?;
        let db = Connection::open("")?;
        crate::index::configure(&db)?;
        db.create_collation("windows_natural", super::places::natural_order)?;
        // Pages are read in row order, so a small cache keeps each open pane cheap.
        db.execute_batch(
            "PRAGMA cache_size = -1024;
             CREATE TABLE listing (
                row INTEGER PRIMARY KEY, name BLOB, directory INTEGER,
                size INTEGER, modified INTEGER, attributes INTEGER,
                sort_name TEXT, extension TEXT
             );
             CREATE TABLE ordering (position INTEGER PRIMARY KEY, row INTEGER UNIQUE);
             CREATE UNIQUE INDEX listing_name ON listing(name)",
        )?;
        Ok(Self {
            path,
            source: Some(source),
            db,
            count: 0,
            sort: None,
            sort_ms: 0.0,
            hide_hidden: false,
            sections: None,
        })
    }

    /// Leaves items marked hidden or system out of the listing; call it before enumerating.
    pub fn hide_hidden_items(&mut self) {
        self.hide_hidden = true;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn is_complete(&self) -> bool {
        self.source.is_none()
    }

    pub fn is_sorted(&self) -> bool {
        self.sort.is_some()
    }
    pub fn sort_ms(&self) -> f64 {
        self.sort_ms
    }

    pub fn position(&self, name: &std::ffi::OsStr) -> Result<Option<u32>> {
        let sql = if self.is_sorted() {
            "SELECT o.position - 1 FROM listing AS l JOIN ordering AS o ON o.row = l.row WHERE name = ?1"
        } else {
            "SELECT row FROM listing WHERE name = ?1"
        };
        Ok(self
            .db
            .query_row(sql, [encode(name)], |row| row.get(0))
            .optional()?)
    }

    /// Builds the row permutation only once enumeration is complete. The underlying
    /// names stay byte-exact; the text collation affects display order only.
    pub fn sort(&mut self, sort: Sort, cancel: &Cancellation) -> Result<()> {
        if !self.is_complete() || self.sort == Some(sort) {
            return Ok(());
        }
        cancel.check()?;
        let start = Instant::now();
        let column = match sort.column {
            SortColumn::Name => "sort_name COLLATE windows_natural",
            SortColumn::Modified => "modified",
            SortColumn::Type => "extension COLLATE windows_natural",
            SortColumn::Size => "size",
        };
        let direction = if sort.descending { "DESC" } else { "ASC" };
        self.sections = None;
        let sections = sort.group.then(section_starts).transpose()?;
        let section = sections
            .as_ref()
            .map_or_else(String::new, |starts| format!("{}, ", section_sql(starts)));
        // As in Explorer, an order by date is about time: folders take their place among the files.
        let folders_first = if sort.group || sort.column == SortColumn::Modified {
            ""
        } else {
            "directory DESC, "
        };
        let interrupted = cancel.clone();
        self.db
            .progress_handler(1000, Some(move || interrupted.is_cancelled()))?;
        let result = (|| -> Result<()> {
            let tx = self.db.transaction()?;
            tx.execute("DELETE FROM ordering", [])?;
            tx.execute(&format!("INSERT INTO ordering(row) SELECT row FROM listing ORDER BY {section}{folders_first}{column} {direction}, sort_name COLLATE windows_natural ASC, row ASC"), [])?;
            tx.commit()?;
            Ok(())
        })();
        self.db.progress_handler(0, None::<fn() -> bool>)?;
        result?;
        self.sort = Some(sort);
        self.sections = sections;
        self.sort_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    /// The date sections of a grouped listing, in row order; empty when it is not grouped.
    pub fn groups(&self) -> Result<Vec<Group>> {
        let Some(starts) = self.sections.as_ref().filter(|_| self.is_sorted()) else {
            return Ok(Vec::new());
        };
        let sql = format!(
            "SELECT {} AS section, COUNT(*) FROM listing GROUP BY section ORDER BY section",
            section_sql(starts)
        );
        let mut statement = self.db.prepare(&sql)?;
        let mut rows = statement.query([])?;
        let mut groups = Vec::new();
        let mut start = 0;
        while let Some(row) = rows.next()? {
            let count: u32 = row.get(1)?;
            groups.push(Group {
                start,
                count,
                bucket: row.get(0)?,
            });
            start += count;
        }
        Ok(groups)
    }

    pub fn advance(&mut self, batch: u32, cancel: &Cancellation) -> Result<()> {
        let hide = if self.hide_hidden { HIDDEN_ITEM } else { 0 };
        let Some(source) = &mut self.source else {
            return Ok(());
        };
        let tx = self.db.transaction()?;
        let mut next = self.count;
        let mut complete = false;
        {
            let mut insert = tx
                .prepare("INSERT OR IGNORE INTO listing VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")?;
            for _ in 0..batch {
                cancel.check()?;
                let Some(entry) = source.next().transpose()? else {
                    complete = true;
                    break;
                };
                // On Windows the entry's metadata comes from the enumeration itself.
                let (Ok(metadata), Ok(kind)) = (entry.metadata(), entry.file_type()) else {
                    continue;
                };
                if metadata.file_attributes() & hide != 0 {
                    continue;
                }
                let directory = kind.is_dir();
                let added = insert.execute(params![
                    next,
                    encode(&entry.file_name()),
                    directory,
                    (!directory).then(|| stored_size(metadata.file_size())),
                    unix_seconds(metadata.last_write_time()),
                    metadata.file_attributes(),
                    entry.file_name().to_string_lossy(),
                    entry
                        .path()
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase(),
                ])?;
                // A broken server can list one name twice; the unique index drops the repeat.
                if added == 0 {
                    continue;
                }
                next = next.checked_add(1).context("Folder is too large")?;
            }
        }
        tx.commit()?;
        self.count = next;
        if complete {
            self.source = None;
        }
        Ok(())
    }

    pub fn list_dir(&self, page: PageRequest, cancel: &Cancellation) -> Result<ItemPage> {
        self.rows(page, cancel)?.items(cancel)
    }

    /// The database half of a page. Opening the items can stall on a slow share, so a caller
    /// holding this directory's lock finishes with [`ListedRows::items`] after releasing it.
    pub fn rows(&self, page: PageRequest, cancel: &Cancellation) -> Result<ListedRows> {
        let sql = if self.is_sorted() {
            "SELECT name, directory, size, modified, attributes FROM ordering AS o JOIN listing AS l ON l.row = o.row WHERE o.position >= ?1 + 1 ORDER BY o.position LIMIT ?2"
        } else {
            "SELECT name, directory, size, modified, attributes FROM listing WHERE row >= ?1 ORDER BY row LIMIT ?2"
        };
        let mut statement = self.db.prepare_cached(sql)?;
        let mut rows = statement.query(params![page.offset, page.limit()])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            cancel.check()?;
            let name: Vec<u8> = row.get(0)?;
            let size: Option<i64> = row.get(2)?;
            items.push((
                self.path.join(decode(&name)),
                row.get(1)?,
                size.map(u64::try_from).transpose()?,
                row.get(3)?,
                row.get(4)?,
            ));
        }
        let end = page.offset.saturating_add(items.len() as u32);
        Ok(ListedRows {
            offset: page.offset,
            rows: items,
            has_more: end < self.count || !self.is_complete(),
        })
    }
}

pub struct ListedRows {
    offset: u32,
    rows: Vec<(PathBuf, bool, Option<u64>, i64, u32)>,
    has_more: bool,
}

impl ListedRows {
    pub fn items(self, cancel: &Cancellation) -> Result<ItemPage> {
        let mut items = Vec::with_capacity(self.rows.len());
        for (path, directory, size, modified, attributes) in self.rows {
            cancel.check()?;
            items.push(Item::listed(path, directory, size, modified, attributes));
        }
        Ok(ItemPage {
            offset: self.offset,
            items,
            has_more: self.has_more,
        })
    }
}
