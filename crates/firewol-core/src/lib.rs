mod index;

use std::fs::{self, File, ReadDir};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[cfg(windows)]
pub mod explorer;
#[cfg(windows)]
mod file_identity;
#[cfg(windows)]
pub mod ntfs;
#[cfg(windows)]
pub mod recycle;

const MAX_PAGE_SIZE: u32 = 128;
const MAX_DIRECTORY_ENTRIES: u32 = 100_000;
const MAX_INDEX_ENTRIES: u32 = 1_000_000;
const INDEX_FILE: &str = "names.sqlite3";
const FIXTURE_FILE: &str = "fixture.json";
const DIRECTORY: &str = "files";

#[derive(Clone, Copy, Deserialize)]
pub struct PageRequest {
    pub offset: u32,
    pub limit: u32,
}

impl PageRequest {
    fn limit(self) -> u32 {
        self.limit.clamp(1, MAX_PAGE_SIZE)
    }
}

#[derive(Deserialize)]
pub struct SearchRequest {
    pub text: String,
    pub extension: Option<String>,
    pub page: PageRequest,
}

#[derive(Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
}

#[derive(Serialize)]
pub struct Page {
    pub items: Vec<Entry>,
    pub has_more: bool,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub struct FixtureInfo {
    pub entries: u32,
    pub index_rows: u32,
}

impl FixtureInfo {
    fn validate(self) -> Result<()> {
        ensure!(
            self.entries <= MAX_DIRECTORY_ENTRIES
                && self.index_rows <= MAX_INDEX_ENTRIES
                && self.entries <= self.index_rows,
            "Use up to {MAX_DIRECTORY_ENTRIES} files and {MAX_INDEX_ENTRIES} index records, with at least one record per file"
        );
        Ok(())
    }
}

pub struct Fixture {
    pub info: FixtureInfo,
    listing: Mutex<Directory>,
    index: Mutex<Connection>,
}

struct Directory {
    source: ReadDir,
    entries: Vec<Entry>,
}

impl Fixture {
    pub fn open(root: &Path) -> Result<Self> {
        let info: FixtureInfo =
            serde_json::from_reader(File::open(root.join(FIXTURE_FILE)).with_context(|| {
                format!("Prepare an M0 fixture before opening {}", root.display())
            })?)?;
        info.validate()?;
        let source = fs::read_dir(root.join(DIRECTORY))?;
        let index = index::open(root.join(INDEX_FILE))?;
        Ok(Self {
            info,
            listing: Mutex::new(Directory {
                source,
                entries: Vec::new(),
            }),
            index: Mutex::new(index),
        })
    }

    pub fn list(&self, page: PageRequest) -> Result<Page> {
        let mut listing = self
            .listing
            .lock()
            .map_err(|_| anyhow::anyhow!("Directory listing unavailable"))?;
        let offset = page.offset.min(self.info.entries) as usize;
        let end = offset + page.limit() as usize;
        let target = (end + 1).min(self.info.entries as usize);
        while listing.entries.len() < target {
            let Some(entry) = listing.source.next().transpose()? else {
                break;
            };
            listing.entries.push(Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                directory: entry.file_type()?.is_dir(),
            });
        }
        let start = offset.min(listing.entries.len());
        let stop = end.min(listing.entries.len());
        Ok(Page {
            items: listing.entries[start..stop].to_vec(),
            has_more: listing.entries.len() > end,
        })
    }

    pub fn search(&self, request: &SearchRequest) -> Result<Page> {
        let db = self
            .index
            .lock()
            .map_err(|_| anyhow::anyhow!("Search index unavailable"))?;
        index::search(&db, request)
    }
}

pub fn fixture_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("FIREWOL_M0_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let local = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unset")?;
    Ok(PathBuf::from(local).join("Firewol").join("M0"))
}

pub fn prepare(root: &Path, info: FixtureInfo) -> Result<()> {
    info.validate()?;
    ensure!(
        !root.exists(),
        "Fixture already exists at {}",
        root.display()
    );
    let parent = root
        .parent()
        .context("Fixture requires a parent directory")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix("firewol-m0-")
        .tempdir_in(parent)?;
    let files = stage.path().join(DIRECTORY);
    fs::create_dir(&files)?;
    for id in 0..info.entries {
        File::create_new(files.join(index::fixture_name(id)))?;
    }
    index::build(&stage.path().join(INDEX_FILE), info.index_rows)?;
    serde_json::to_writer(File::create_new(stage.path().join(FIXTURE_FILE))?, &info)?;
    fs::rename(stage.path(), root)?;
    Ok(())
}

pub fn index_workload(root: &Path, count: u32) -> Result<()> {
    ensure!(count <= MAX_INDEX_ENTRIES, "Too many index records");
    let stage = tempfile::tempdir_in(root)?;
    index::build(&stage.path().join(INDEX_FILE), count)
}
