use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result};
use firewol_core::explorer::{
    Cancellation, Directory, Filters, Item, ItemPage, PAGE_SIZE, SearchIndex, SearchQuery,
    SearchReader, data_dir, display_path, page,
};

use crate::contract::{Entry, FolderProgress, FolderView, IndexState, Page, Segment};

pub enum Notice {
    Progress(FolderProgress),
    Index(IndexState),
}

pub type Notify = Arc<dyn Fn(Notice) + Send + Sync>;

/// Open folder snapshots, keyed by the id the interface holds.
#[derive(Default)]
pub struct Folders {
    next: AtomicU64,
    open: Mutex<HashMap<u64, Arc<Folder>>>,
}

struct Folder {
    owner: String,
    path: PathBuf,
    cancel: Cancellation,
    directory: Mutex<Directory>,
    search: Mutex<Search>,
    query: Mutex<Option<Cancellation>>,
}

#[derive(Default)]
struct Search {
    index: Option<SearchIndex>,
    ready: Option<u64>,
    reader: Option<SearchReader>,
}

impl Folders {
    pub fn open(&self, owner: &str, path: &Path, notify: Notify) -> Result<FolderView> {
        let cancel = Cancellation::default();
        let mut directory = Directory::open(path)?;
        directory.advance(PAGE_SIZE, &cancel)?;
        let first = directory.list_dir(page(0), &cancel)?;
        let path = directory.path().to_owned();
        let (count, complete) = (directory.count(), directory.is_complete());
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let folder = Arc::new(Folder {
            owner: owner.to_owned(),
            path: path.clone(),
            cancel,
            directory: Mutex::new(directory),
            search: Mutex::default(),
            query: Mutex::default(),
        });
        lock(&self.open).insert(id, folder.clone());
        if !complete {
            thread::spawn(move || enumerate(id, &folder, &notify));
        }
        Ok(FolderView {
            id,
            path: text(&path),
            segments: segments(&path),
            count,
            complete,
            page: convert(first),
        })
    }

    pub fn page(&self, id: u64, offset: u32) -> Result<Page> {
        let folder = self.get(id)?;
        let items = lock(&folder.directory).list_dir(page(offset), &folder.cancel)?;
        Ok(convert(items))
    }

    pub fn index(&self, id: u64, notify: Notify) -> Result<IndexState> {
        let folder = self.get(id)?;
        let mut search = lock(&folder.search);
        if search.index.is_some() {
            return Ok(IndexState {
                id,
                ready: search.ready.is_some(),
                count: search.ready.unwrap_or(0),
                error: None,
            });
        }
        let data = data_dir()?;
        let index = SearchIndex::create(&data)?;
        search.index = Some(index.clone());
        drop(search);
        thread::spawn(move || {
            let result = index.watch_folder(&folder.path, &data, &folder.cancel, |count| {
                lock(&folder.search).ready = Some(count);
                notify(Notice::Index(IndexState {
                    id,
                    ready: true,
                    count,
                    error: None,
                }));
            });
            if let Err(error) = result
                && !folder.cancel.is_cancelled()
            {
                lock(&folder.search).index = None;
                notify(Notice::Index(IndexState {
                    id,
                    ready: false,
                    count: 0,
                    error: Some(format!("{error:#}")),
                }));
            }
        });
        Ok(IndexState {
            id,
            ready: false,
            count: 0,
            error: None,
        })
    }

    pub fn search(&self, id: u64, text: String, offset: u32) -> Result<Page> {
        let folder = self.get(id)?;
        let cancel = Cancellation::default();
        if let Some(previous) = lock(&folder.query).replace(cancel.clone()) {
            previous.cancel();
        }
        let mut search = lock(&folder.search);
        let Search {
            index,
            ready,
            reader,
        } = &mut *search;
        let index = index
            .as_ref()
            .filter(|_| ready.is_some())
            .context("The folder index is not ready")?;
        if !reader.as_ref().is_some_and(|current| current.reads(index)) {
            *reader = Some(index.reader()?);
        }
        let reader = reader.as_mut().context("Open the folder index")?;
        let query = SearchQuery {
            text,
            regex: false,
            filters: Filters::default(),
            page: page(offset),
        };
        Ok(convert(reader.search(&query, &cancel)?))
    }

    pub fn close(&self, id: u64) {
        if let Some(folder) = lock(&self.open).remove(&id) {
            folder.cancel.cancel();
        }
    }

    pub fn close_owned(&self, owner: &str) {
        lock(&self.open).retain(|_, folder| {
            let keep = folder.owner != owner;
            if !keep {
                folder.cancel.cancel();
            }
            keep
        });
    }

    fn get(&self, id: u64) -> Result<Arc<Folder>> {
        lock(&self.open)
            .get(&id)
            .cloned()
            .context("The folder was closed")
    }
}

fn enumerate(id: u64, folder: &Folder, notify: &Notify) {
    let mut last = Instant::now();
    loop {
        let (count, complete, result) = {
            let mut directory = lock(&folder.directory);
            let result = directory.advance(PAGE_SIZE * 4, &folder.cancel);
            (directory.count(), directory.is_complete(), result)
        };
        if folder.cancel.is_cancelled() {
            return;
        }
        let error = result.err().map(|error| format!("{error:#}"));
        let done = complete || error.is_some();
        if done || last.elapsed() >= Duration::from_millis(85) {
            notify(Notice::Progress(FolderProgress {
                id,
                count,
                complete: done,
                error,
            }));
            last = Instant::now();
        }
        if done {
            return;
        }
    }
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn text(path: &Path) -> String {
    display_path(path).to_string_lossy().into_owned()
}

pub fn segment(path: &Path) -> Segment {
    let path = text(path);
    let name = Path::new(&path).file_name().map_or_else(
        || path.trim_end_matches('\\').to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Segment { name, path }
}

fn segments(path: &Path) -> Vec<Segment> {
    let display = PathBuf::from(display_path(path));
    let mut segments: Vec<_> = display.ancestors().map(segment).collect();
    segments.reverse();
    segments
}

fn convert(page: ItemPage) -> Page {
    Page {
        offset: page.offset,
        has_more: page.has_more,
        items: page.items.iter().map(entry).collect(),
    }
}

fn entry(item: &Item) -> Entry {
    let metadata = fs::symlink_metadata(&item.path).ok();
    Entry {
        name: item.name().to_string_lossy().into_owned(),
        path: text(&item.path),
        directory: item.directory,
        size: metadata
            .as_ref()
            .filter(|metadata| !metadata.is_dir())
            .map(fs::Metadata::len),
        modified: metadata
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs() as i64),
    }
}
