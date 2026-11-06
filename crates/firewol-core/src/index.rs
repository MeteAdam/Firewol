use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags, params};

use crate::{Entry, Page, SearchRequest};

pub(crate) fn configure(db: &Connection) -> Result<()> {
    db.execute_batch("PRAGMA cache_size = -4096; PRAGMA mmap_size = 0; PRAGMA temp_store = FILE;")?;
    Ok(())
}

pub(super) fn open(path: PathBuf) -> Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    configure(&db)?;
    Ok(db)
}

pub(super) fn fixture_name(id: u32) -> String {
    match id % 10 {
        0 => format!("report-{id:07}.pdf"),
        1 => format!("résumé-{id:07}.txt"),
        _ => format!("record-{id:07}.txt"),
    }
}

pub(super) fn build(path: &Path, count: u32) -> Result<()> {
    let mut db = Connection::open(path)?;
    configure(&db)?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TABLE entries (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            name_key TEXT NOT NULL,
            extension TEXT NOT NULL
        );
        CREATE VIRTUAL TABLE names USING fts5(
            name_key, content='entries', content_rowid='id',
            tokenize='trigram case_sensitive 1'
        );",
    )?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO entries (id, name, name_key, extension) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for id in 0..count {
            let name = fixture_name(id);
            let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
            insert.execute(params![id, name, name.to_lowercase(), extension])?;
        }
    }
    tx.execute_batch(
        "CREATE INDEX entries_prefix ON entries(name_key COLLATE NOCASE);
         INSERT INTO names(names) VALUES ('rebuild');",
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn search(db: &Connection, request: &SearchRequest) -> Result<Page> {
    let text = request.text.trim().to_lowercase();
    ensure!(
        text.len() <= 1024 && !text.contains('\0'),
        "Invalid search text"
    );
    if text.is_empty() {
        return Ok(Page {
            items: Vec::new(),
            has_more: false,
        });
    }
    let (sql, term) = if text.chars().count() < 3 {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        (
            "SELECT name FROM entries
             WHERE name_key LIKE ?1 ESCAPE '\\' AND (?2 IS NULL OR extension = ?2)
             ORDER BY name_key COLLATE NOCASE, id LIMIT ?3 OFFSET ?4",
            format!("{escaped}%"),
        )
    } else {
        (
            "SELECT entries.name FROM names JOIN entries ON entries.id = names.rowid
             WHERE names MATCH ?1 AND (?2 IS NULL OR entries.extension = ?2)
             ORDER BY names.rowid LIMIT ?3 OFFSET ?4",
            format!("\"{}\"", text.replace('"', "\"\"")),
        )
    };
    let limit = request.page.limit();
    let extension = request
        .extension
        .as_ref()
        .map(|ext| ext.trim_start_matches('.').to_lowercase());
    let mut statement = db.prepare_cached(sql)?;
    let mut items = statement
        .query_map(
            params![term, extension, limit + 1, request.page.offset],
            |row| {
                Ok(Entry {
                    name: row.get(0)?,
                    directory: false,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(Page { items, has_more })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PageRequest;

    #[test]
    fn search_uses_literal_terms_and_bounded_pages() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("test.sqlite3");
        build(&path, 40)?;
        let db = open(path)?;
        let mut request = SearchRequest {
            text: "RE".into(),
            extension: Some(".PDF".into()),
            page: PageRequest {
                offset: 0,
                limit: 2,
            },
        };
        let page = search(&db, &request)?;
        assert!(page.has_more);
        assert_eq!(page.items[0].name, "report-0000000.pdf");
        request.page.offset = 2;
        let next = search(&db, &request)?;
        assert!(!next.has_more);
        assert_eq!(next.items[0].name, "report-0000020.pdf");
        request.extension = None;
        request.page.offset = 0;
        request.text = "ÉSU".into();
        assert_eq!(search(&db, &request)?.items[0].name, "résumé-0000001.txt");
        for text in ["%", "_", "x\" OR y"] {
            request.text = text.into();
            assert!(search(&db, &request)?.items.is_empty());
        }
        Ok(())
    }
}
