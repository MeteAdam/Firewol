use std::fs::{self, ReadDir};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use regex::RegexBuilder;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, Statement, params};
use tempfile::TempDir;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use super::{Cancellation, Item, ItemId, ItemPage, decode, encode, watch::Watch};
use crate::PageRequest;
use crate::file_identity::FileIdentity;

#[derive(Clone, Default)]
pub struct Filters {
    pub extension: Option<String>,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    pub modified_after: Option<i64>,
    pub modified_before: Option<i64>,
}

#[derive(Clone)]
pub struct SearchQuery {
    pub text: String,
    pub regex: bool,
    pub filters: Filters,
    pub page: PageRequest,
}

impl Filters {
    /// Rejects reversed ranges and extensions that are not a single plain suffix.
    pub fn validate(&self) -> Result<()> {
        if let Some(extension) = &self.extension {
            let extension = extension.trim().trim_start_matches('.');
            ensure!(
                extension.len() <= 64
                    && !extension.chars().any(|c| matches!(
                        c,
                        '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
                    )),
                "Enter an extension such as pdf or .txt"
            );
        }
        ensure!(
            self.min_bytes
                .zip(self.max_bytes)
                .is_none_or(|(min, max)| min <= max),
            "The minimum size is larger than the maximum size"
        );
        ensure!(
            self.modified_after
                .zip(self.modified_before)
                .is_none_or(|(after, before)| after <= before),
            "The date range is reversed"
        );
        Ok(())
    }
}

struct Plan {
    source: &'static str,
    predicate: &'static str,
    order: &'static str,
    term: Option<String>,
}

const FILTER: &str = "AND (?2 IS NULL OR extension = ?2)
    AND (?3 IS NULL OR size >= ?3) AND (?4 IS NULL OR size <= ?4)
    AND (?5 IS NULL OR modified >= ?5) AND (?6 IS NULL OR modified <= ?6)";

type FilterParams = (
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn filter_params(filters: &Filters) -> Result<FilterParams> {
    let extension = filters
        .extension
        .as_ref()
        .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
        .filter(|ext| !ext.is_empty());
    Ok((
        extension,
        filters.min_bytes.map(i64::try_from).transpose()?,
        filters.max_bytes.map(i64::try_from).transpose()?,
        filters.modified_after,
        filters.modified_before,
    ))
}

const INDEX_PREFIX: &str = "search-";

/// Removes index folders left by Firewol processes that ended without dropping them
/// (an exiting window process does not run destructors). Folders of running processes stay.
pub fn remove_stale_indexes(data_dir: &Path) {
    let Ok(entries) = fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name
            .to_str()
            .and_then(|name| name.strip_prefix(INDEX_PREFIX))
        else {
            continue;
        };
        let owner = rest
            .split_once('-')
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        if owner.is_some_and(|pid| pid == std::process::id() || process_running(pid)) {
            continue;
        }
        // Folders without an owner come from older builds; files still open cannot be removed.
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn process_running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(process) => process,
        // Access denied means the process exists; an invalid id means it ended.
        Err(error) => {
            return error.code() == windows::Win32::Foundation::ERROR_ACCESS_DENIED.to_hresult();
        }
    };
    let mut code = 0;
    let running =
        unsafe { GetExitCodeProcess(process, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32;
    let _ = unsafe { CloseHandle(process) };
    running
}

#[derive(Clone)]
pub struct SearchIndex(Arc<TempDir>);

pub struct SearchReader {
    db: Connection,
    index: SearchIndex,
}

const MAX_NAME_UNITS: usize = 255;
const MAX_INDEXED_PATH: usize = 4096;
const MAX_INDEXED_DEPTH: usize = 256;

/// Every row stores its whole path and opening a path gets slower with depth, so a crafted tree
/// of thousands of levels could cost gigabytes and hours. No real tree gets near these limits.
pub(super) fn indexable(path: &Path) -> bool {
    path.as_os_str().len() <= 4 * MAX_INDEXED_PATH
        && path.as_os_str().encode_wide().count() <= MAX_INDEXED_PATH
        && path.components().count() <= MAX_INDEXED_DEPTH
}

/// Keeps one index from filling the system drive. Counted by stored path bytes, the column that
/// a hostile tree can inflate, with the free space read again as the index grows.
pub(super) struct IndexBudget {
    folder: PathBuf,
    limit: u64,
    bytes: u64,
    next_check: u64,
}

impl IndexBudget {
    const LIMIT: u64 = 1 << 30;
    const CHECK_EVERY: u64 = 16 << 20;
    const MIN_FREE: u64 = 256 << 20;

    pub fn new(folder: &Path) -> Self {
        Self::with_limit(folder, Self::LIMIT)
    }

    fn with_limit(folder: &Path, limit: u64) -> Self {
        Self {
            folder: folder.to_owned(),
            limit,
            bytes: 0,
            next_check: 0,
        }
    }

    pub fn charge(&mut self, path: &Path) -> Result<()> {
        self.bytes += path.as_os_str().len() as u64;
        ensure!(
            self.bytes <= self.limit,
            "This location is too large to index"
        );
        if self.bytes >= self.next_check {
            self.next_check = self.bytes + Self::CHECK_EVERY;
            let (free, _) = super::free_space(&self.folder)?;
            ensure!(
                free >= Self::MIN_FREE,
                "There is not enough free disk space to index this location"
            );
        }
        Ok(())
    }
}

pub(super) const INSERT: &str = "INSERT INTO entries (id, path, directory, name_key, extension, size, modified, file_ref, file_volume)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

const RESET: &str = "INSERT INTO names(names) VALUES ('delete-all'); DELETE FROM entries;";

impl SearchIndex {
    pub fn create(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir)?;
        let index = Self(Arc::new(
            tempfile::Builder::new()
                .prefix(&format!("{INDEX_PREFIX}{}-", std::process::id()))
                .tempdir_in(data_dir)?,
        ));
        let db = index.writer()?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE entries (
                id INTEGER PRIMARY KEY,
                path BLOB NOT NULL,
                directory INTEGER NOT NULL,
                name_key TEXT NOT NULL,
                extension TEXT NOT NULL,
                size INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                file_ref INTEGER,
                file_volume INTEGER
             );
             CREATE INDEX entries_prefix ON entries(name_key COLLATE NOCASE);
             CREATE VIRTUAL TABLE names USING fts5(
                name_key, content='entries', content_rowid='id', tokenize='trigram case_sensitive 1'
             );
             CREATE TABLE checkpoint (journal INTEGER, cursor INTEGER);",
        )?;
        Ok(index)
    }

    fn path(&self) -> PathBuf {
        self.0.path().join("index.sqlite3")
    }

    pub(super) fn writer(&self) -> Result<Connection> {
        let db = Connection::open(self.path())?;
        crate::index::configure(&db)?;
        Ok(db)
    }

    pub fn reader(&self) -> Result<SearchReader> {
        Ok(SearchReader {
            db: crate::index::open(self.path())?,
            index: self.clone(),
        })
    }

    pub fn watch_folder(
        &self,
        root: &Path,
        excluded: &Path,
        cancel: &Cancellation,
        mut updated: impl FnMut(u64),
    ) -> Result<()> {
        let mut db = self.writer()?;
        let excluded = excluded.canonicalize()?;
        let excluded = if root.starts_with(&excluded) {
            self.0.path().canonicalize()?
        } else {
            excluded
        };
        let relative_excluded = excluded
            .strip_prefix(root)
            .ok()
            .filter(|path| !path.as_os_str().is_empty());
        let mut watch = Watch::open(root, true).context("Watch the selected folder for changes")?;
        while !cancel.is_cancelled() {
            let started = Instant::now();
            let count = cancellable(&mut db, cancel, |db| {
                rebuild_folder(db, root, &excluded, cancel)
            })?;
            updated(count);
            let rest = rest_after(started.elapsed());
            while !cancel.is_cancelled() {
                if watch.changed(relative_excluded, 200)? {
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_secs(1)
                        && !cancel.is_cancelled()
                        && watch.changed(relative_excluded, 150)?
                    {}
                    // Constant writes under a large tree must not mean constant full rebuilds.
                    let start = Instant::now();
                    while start.elapsed() < rest && !cancel.is_cancelled() {
                        watch.changed(relative_excluded, 150)?;
                    }
                    break;
                }
            }
        }
        Ok(())
    }
}

impl SearchReader {
    pub fn reads(&self, index: &SearchIndex) -> bool {
        Arc::ptr_eq(&self.index.0, &index.0)
    }

    pub fn search(&mut self, query: &SearchQuery, cancel: &Cancellation) -> Result<ItemPage> {
        self.guarded(cancel, |reader| reader.query(query, cancel))
    }

    /// Counts matches up to `cap + 1`, so a result above `cap` means "more than `cap`".
    pub fn count(&mut self, query: &SearchQuery, cap: u32, cancel: &Cancellation) -> Result<u64> {
        self.guarded(cancel, |reader| {
            let plan = reader.plan(query)?;
            let sql = format!(
                "SELECT COUNT(*) FROM (SELECT 1 FROM {} WHERE {} {FILTER} LIMIT ?7)",
                plan.source, plan.predicate
            );
            let (extension, min, max, after, before) = filter_params(&query.filters)?;
            let count: i64 = reader.db.prepare_cached(&sql)?.query_row(
                params![
                    plan.term,
                    extension,
                    min,
                    max,
                    after,
                    before,
                    i64::from(cap) + 1
                ],
                |row| row.get(0),
            )?;
            Ok(count as u64)
        })
    }

    fn guarded<T>(
        &mut self,
        cancel: &Cancellation,
        run: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        cancel.check()?;
        let token = cancel.clone();
        self.db
            .progress_handler(1000, Some(move || token.is_cancelled()))?;
        let result = run(self);
        self.db.progress_handler(0, None::<fn() -> bool>)?;
        result
    }

    /// Validates the query and picks the table, term predicate and order it searches with.
    fn plan(&self, query: &SearchQuery) -> Result<Plan> {
        let text = query.text.trim();
        ensure!(
            !text.is_empty() && text.len() <= 1024 && !text.contains('\0'),
            "Enter a search term of 1–1024 bytes"
        );
        query.filters.validate()?;
        Ok(if query.regex {
            let regex = RegexBuilder::new(text)
                .case_insensitive(true)
                .size_limit(1 << 20)
                .build()
                .context("Invalid regular expression")?;
            self.db.create_scalar_function(
                "matches_regex",
                1,
                FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
                move |ctx| Ok(regex.is_match(ctx.get_raw(0).as_str()?)),
            )?;
            Plan {
                source: "entries",
                predicate: "matches_regex(name_key)",
                order: "id",
                term: None,
            }
        } else {
            // No file name is longer, and a long phrase cannot be interrupted once SQLite starts.
            ensure!(
                text.encode_utf16().count() <= MAX_NAME_UNITS,
                "Enter a search term of up to {MAX_NAME_UNITS} characters"
            );
            let text = text.to_lowercase();
            if text.chars().count() < 3 {
                let escaped = text
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                Plan {
                    source: "entries",
                    predicate: "name_key LIKE ?1 ESCAPE '\\'",
                    order: "name_key COLLATE NOCASE, id",
                    term: Some(format!("{escaped}%")),
                }
            } else {
                Plan {
                    source: "names JOIN entries ON entries.id = names.rowid",
                    predicate: "names MATCH ?1",
                    order: "names.rowid",
                    term: Some(format!("\"{}\"", text.replace('"', "\"\""))),
                }
            }
        })
    }

    fn query(&self, query: &SearchQuery, cancel: &Cancellation) -> Result<ItemPage> {
        let Plan {
            source,
            predicate,
            order,
            term,
        } = self.plan(query)?;
        let sql = format!(
            "SELECT path, directory, file_ref, file_volume, size, modified FROM {source}
             WHERE {predicate} {FILTER} ORDER BY {order} LIMIT ?7 OFFSET ?8"
        );
        let (extension, min, max, after, before) = filter_params(&query.filters)?;
        let limit = query.page.limit();
        let mut statement = self.db.prepare_cached(&sql)?;
        let mut rows = statement.query(params![
            term,
            extension,
            min,
            max,
            after,
            before,
            limit + 1,
            query.page.offset,
        ])?;
        let mut items = Vec::new();
        let mut has_more = false;
        for i in 0..=limit {
            cancel.check()?;
            let Some(row) = rows.next()? else { break };
            if i == limit {
                has_more = true;
                break;
            }
            let path: Vec<u8> = row.get(0)?;
            let path = PathBuf::from(decode(&path));
            let directory: bool = row.get(1)?;
            let size: i64 = row.get(4)?;
            let file_ref: Option<i64> = row.get(2)?;
            let Ok(id) = ItemId::at(path.clone()) else {
                continue;
            };
            if let Some(file_ref) = file_ref
                && (id.identity.index != file_ref as u64
                    || id.identity.volume != row.get::<_, u32>(3)?
                    || !super::volume::visible_parent(&path))
            {
                continue;
            }
            items.push(Item {
                path,
                directory,
                size: (!directory).then(|| u64::try_from(size)).transpose()?,
                modified: Some(row.get(5)?),
                attributes: None,
                id: Some(id),
            });
        }
        Ok(ItemPage {
            offset: query.page.offset,
            items,
            has_more,
        })
    }
}

pub(super) struct IndexedItem<'a> {
    pub path: &'a Path,
    pub directory: bool,
    pub size: u64,
    pub modified: i64,
    pub file_id: Option<FileIdentity>,
}

impl IndexedItem<'_> {
    pub fn insert(&self, insert: &mut Statement<'_>, row: Option<i64>) -> Result<()> {
        let name = self
            .path
            .file_name()
            .context("Missing entry name")?
            .to_string_lossy()
            .to_lowercase();
        let extension = self
            .path
            .extension()
            .map_or_else(String::new, |ext| ext.to_string_lossy().to_lowercase());
        insert.execute(params![
            row,
            encode(self.path.as_os_str()),
            self.directory,
            name,
            extension,
            super::stored_size(self.size),
            self.modified,
            self.file_id.map(|id| id.index as i64),
            self.file_id.map(|id| id.volume),
        ])?;
        Ok(())
    }
}

/// How long to wait before the next full pass, in proportion to what the last one cost.
pub(super) fn rest_after(took: Duration) -> Duration {
    (took * 4).min(Duration::from_secs(300))
}

pub(super) fn reset(db: &Connection) -> Result<()> {
    db.execute_batch(RESET)?;
    Ok(())
}

pub(super) fn cancellable<T>(
    db: &mut Connection,
    cancel: &Cancellation,
    run: impl FnOnce(&mut Connection) -> Result<T>,
) -> Result<T> {
    let token = cancel.clone();
    db.progress_handler(1000, Some(move || token.is_cancelled()))?;
    let result = run(db);
    db.progress_handler(0, None::<fn() -> bool>)?;
    result
}

pub(super) fn rebuild(db: &Connection) -> Result<()> {
    db.execute("INSERT INTO names(names) VALUES ('rebuild')", [])?;
    Ok(())
}

fn rebuild_folder(
    db: &mut Connection,
    root: &Path,
    excluded: &Path,
    cancel: &Cancellation,
) -> Result<u64> {
    let tx = db.transaction()?;
    reset(&tx)?;
    let mut count = 0;
    let mut budget = IndexBudget::new(excluded);
    {
        let mut insert = tx.prepare(INSERT)?;
        let mut directories: Vec<ReadDir> = vec![fs::read_dir(root)?];
        while let Some(directory) = directories.last_mut() {
            cancel.check()?;
            let Some(entry) = directory.next() else {
                directories.pop();
                continue;
            };
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path == excluded || !indexable(&path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let reparse = meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0;
            if meta.is_dir()
                && !reparse
                && let Ok(children) = fs::read_dir(&path)
            {
                directories.push(children);
            }
            budget.charge(&path)?;
            let modified = (meta.last_write_time() / 10_000_000) as i64 - 11_644_473_600;
            IndexedItem {
                path: &path,
                directory: meta.is_dir(),
                size: meta.len(),
                modified,
                file_id: None,
            }
            .insert(&mut insert, None)?;
            count += 1;
        }
    }
    rebuild(&tx)?;
    tx.commit()?;
    Ok(count)
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn the_index_budget_stops_at_its_limit() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let mut budget = IndexBudget::with_limit(fixture.path(), 10);
        budget.charge(Path::new("12345"))?;
        budget.charge(Path::new("12345"))?;
        assert!(budget.charge(Path::new("1")).is_err());
        Ok(())
    }

    #[test]
    fn extreme_paths_are_left_out_of_the_index() {
        let deep = PathBuf::from(format!(r"C:\{}", "a\\".repeat(MAX_INDEXED_DEPTH)));
        assert!(!indexable(&deep));
        let long = PathBuf::from(format!(r"C:\{}", "a".repeat(MAX_INDEXED_PATH + 1)));
        assert!(!indexable(&long));
        assert!(indexable(Path::new(r"C:\Users\Mete\Documents\a.txt")));
    }

    #[test]
    fn a_costly_pass_earns_a_longer_rest() {
        assert_eq!(
            rest_after(Duration::from_millis(50)),
            Duration::from_millis(200)
        );
        assert_eq!(
            rest_after(Duration::from_secs(600)),
            Duration::from_secs(300)
        );
    }
}
