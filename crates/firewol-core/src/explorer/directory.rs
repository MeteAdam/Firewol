use std::fs::{self, ReadDir};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use super::{Cancellation, Item, ItemPage, decode, encode, resolve_directory};
use crate::PageRequest;

/// A directory snapshot whose row numbers stay stable while enumeration proceeds.
pub struct Directory {
    path: PathBuf,
    source: Option<ReadDir>,
    db: Connection,
    count: u32,
}

impl Directory {
    pub fn open(path: &Path) -> Result<Self> {
        let path = resolve_directory(path)?;
        let source = fs::read_dir(&path).with_context(|| format!("List {}", path.display()))?;
        let db = Connection::open("")?;
        crate::index::configure(&db)?;
        // Pages are read in row order, so a small cache keeps each open pane cheap.
        db.execute_batch(
            "PRAGMA cache_size = -1024;
             CREATE TABLE listing (row INTEGER PRIMARY KEY, name BLOB, directory INTEGER)",
        )?;
        Ok(Self {
            path,
            source: Some(source),
            db,
            count: 0,
        })
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

    pub fn advance(&mut self, batch: u32, cancel: &Cancellation) -> Result<()> {
        let Some(source) = &mut self.source else {
            return Ok(());
        };
        let tx = self.db.transaction()?;
        let mut next = self.count;
        let mut complete = false;
        {
            let mut insert = tx.prepare("INSERT INTO listing VALUES (?1, ?2, ?3)")?;
            for _ in 0..batch {
                cancel.check()?;
                let Some(entry) = source.next().transpose()? else {
                    complete = true;
                    break;
                };
                insert.execute(params![
                    next,
                    encode(&entry.file_name()),
                    entry.file_type()?.is_dir()
                ])?;
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
        let mut statement = self.db.prepare_cached(
            "SELECT name, directory FROM listing WHERE row >= ?1 ORDER BY row LIMIT ?2",
        )?;
        let mut rows = statement.query(params![page.offset, page.limit()])?;
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            cancel.check()?;
            let name: Vec<u8> = row.get(0)?;
            items.push(Item::from_path(self.path.join(decode(&name)), row.get(1)?));
        }
        let end = page.offset.saturating_add(items.len() as u32);
        Ok(ItemPage {
            offset: page.offset,
            items,
            has_more: end < self.count || !self.is_complete(),
        })
    }
}
