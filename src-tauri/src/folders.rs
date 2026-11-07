use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{
    Cancellation, Directory, Filters, FolderKey, FolderStyle, History, Item, ItemId, ItemPage,
    PAGE_SIZE, SearchIndex, SearchQuery, SearchReader, Sort, SortColumn, data_dir, display_path,
    page,
};

use crate::contract::{
    Entry, FolderProgress, FolderView, IndexState, ItemRef, Navigation, Page, Segment, SortView,
    Step,
};

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

pub enum Notice {
    Progress(FolderProgress),
    Index(IndexState),
}

pub type Notify = Arc<dyn Fn(Notice) + Send + Sync>;
pub type Styles = BTreeMap<FolderKey, FolderStyle>;
type StyleSource = Arc<dyn Fn() -> Styles + Send + Sync>;

/// Open folder snapshots, keyed by the id the interface holds, and the panes showing them.
#[derive(Default)]
pub struct Folders {
    next: AtomicU64,
    open: Mutex<HashMap<u64, Arc<Folder>>>,
    panes: Mutex<HashMap<String, Pane>>,
    styles: OnceLock<StyleSource>,
}

struct Folder {
    owner: String,
    path: PathBuf,
    cancel: Cancellation,
    directory: Mutex<Directory>,
    sort: Mutex<Sort>,
    search: Mutex<Search>,
    query: Mutex<Option<Cancellation>>,
}

#[derive(Default)]
struct Search {
    index: Option<SearchIndex>,
    ready: Option<u64>,
    reader: Option<SearchReader>,
}

#[derive(Default)]
struct Pane {
    owner: String,
    history: History,
    folder: Option<u64>,
    // Bumped by every navigation, so a slower earlier one cannot replace a later one.
    sequence: u64,
}

impl Folders {
    /// Sets where listings read folder styles from (the shared settings), once at startup.
    pub fn use_styles(&self, source: impl Fn() -> Styles + Send + Sync + 'static) {
        let _ = self.styles.set(Arc::new(source));
    }

    /// Read per page rather than cached, so every listed page shows the latest saved styles.
    fn styles(&self) -> Styles {
        self.styles.get().map(|source| source()).unwrap_or_default()
    }

    /// Opens `path` (or the history entry `step` names) in `pane` and updates its history.
    pub fn navigate(
        &self,
        owner: &str,
        pane: &str,
        path: Option<&str>,
        step: Step,
        sort: SortView,
        notify: Notify,
    ) -> Result<Navigation> {
        let (target, position, sequence) = {
            let mut panes = lock(&self.panes);
            let state = panes.entry(pane.to_owned()).or_default();
            ensure!(
                state.owner.is_empty() || state.owner == owner,
                "This tab belongs to another window"
            );
            state.owner = owner.to_owned();
            state.sequence += 1;
            let (target, position) = match step {
                Step::New => (PathBuf::from(path.context("Enter a folder")?), None),
                Step::Back | Step::Forward => {
                    let (position, target) = state
                        .history
                        .peek(matches!(step, Step::Forward))
                        .context("There is no folder in that direction")?;
                    (target.to_owned(), Some(position))
                }
                Step::Reload => (
                    state
                        .history
                        .current()
                        .map(Path::to_owned)
                        .or_else(|| path.map(PathBuf::from))
                        .context("Open a folder first")?,
                    None,
                ),
            };
            (target, position, state.sequence)
        };
        let folder = self.open(owner, &target, sort, notify)?;
        let mut panes = lock(&self.panes);
        let state = panes.get_mut(pane);
        if state
            .as_ref()
            .is_none_or(|state| state.sequence != sequence || state.owner != owner)
        {
            drop(panes);
            self.close(folder.id);
            anyhow::bail!("A newer navigation replaced this one");
        }
        let state = state.context("The tab was closed")?;
        match (step, position) {
            (Step::Back | Step::Forward, Some(position)) => state.history.go(position),
            (Step::New, _) => state.history.visit(PathBuf::from(&folder.path)),
            (Step::Reload, _) if state.history.current().is_none() => {
                state.history.visit(PathBuf::from(&folder.path))
            }
            _ => {}
        }
        let previous = state.folder.replace(folder.id);
        let navigation = Navigation {
            can_back: state.history.peek(false).is_some(),
            can_forward: state.history.peek(true).is_some(),
            folder,
        };
        drop(panes);
        if let Some(previous) = previous {
            self.close(previous);
        }
        Ok(navigation)
    }

    pub fn close_pane(&self, owner: &str, pane: &str) {
        let mut panes = lock(&self.panes);
        if !panes.get(pane).is_some_and(|pane| pane.owner == owner) {
            return;
        }
        if let Some(folder) = panes.remove(pane).and_then(|state| state.folder) {
            drop(panes);
            self.close(folder);
        }
    }

    pub fn suspend(&self, owner: &str, pane: &str) -> Result<()> {
        let mut panes = lock(&self.panes);
        let Some(state) = panes.get_mut(pane) else {
            return Ok(());
        };
        ensure!(state.owner == owner, "This tab belongs to another window");
        state.sequence += 1;
        let folder = state.folder.take();
        drop(panes);
        if let Some(folder) = folder {
            self.close(folder);
        }
        Ok(())
    }

    pub fn adopt(&self, from: &str, to: &str, pane: &str) -> Result<()> {
        self.suspend(from, pane)?;
        let mut panes = lock(&self.panes);
        let state = panes.get_mut(pane).context("The tab was closed")?;
        ensure!(state.owner == from, "This tab belongs to another window");
        state.owner = to.to_owned();
        Ok(())
    }

    fn open(&self, owner: &str, path: &Path, sort: SortView, notify: Notify) -> Result<FolderView> {
        let cancel = Cancellation::default();
        let mut directory = Directory::open(path)?;
        directory.advance(PAGE_SIZE, &cancel)?;
        let sort = sort_core(sort);
        directory.sort(sort, &cancel)?;
        let sort_ms = directory.sort_ms();
        let first = directory.list_dir(page(0), &cancel)?;
        let path = directory.path().to_owned();
        let (count, complete) = (directory.count(), directory.is_complete());
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let folder = Arc::new(Folder {
            owner: owner.to_owned(),
            path: path.clone(),
            cancel,
            directory: Mutex::new(directory),
            sort: Mutex::new(sort),
            search: Mutex::default(),
            query: Mutex::default(),
        });
        lock(&self.open).insert(id, folder.clone());
        if !complete {
            thread::spawn(move || enumerate(id, &folder, &notify));
        }
        Ok(FolderView {
            id,
            item: ItemId::at(path.clone()).ok().as_ref().map(item_ref),
            path: text(&path),
            segments: segments(&path),
            count,
            complete,
            sort_ms,
            page: Page {
                sorted: complete,
                ..convert(first, &self.styles())
            },
        })
    }

    pub fn page(&self, id: u64, offset: u32, sort: SortView) -> Result<Page> {
        let folder = self.get(id)?;
        let styles = self.styles();
        let sort = sort_core(sort);
        *lock(&folder.sort) = sort;
        let mut directory = lock(&folder.directory);
        directory.sort(sort, &folder.cancel)?;
        let items = directory.list_dir(page(offset), &folder.cancel)?;
        Ok(Page {
            sorted: directory.is_sorted(),
            ..convert(items, &styles)
        })
    }

    pub fn progress(&self, id: u64) -> Result<FolderProgress> {
        let folder = self.get(id)?;
        let directory = lock(&folder.directory);
        Ok(FolderProgress {
            id,
            count: directory.count(),
            complete: directory.is_complete(),
            sort_ms: directory.sort_ms(),
            error: None,
        })
    }

    pub fn locate(&self, id: u64, items: &[ItemRef]) -> Result<Vec<Option<u32>>> {
        let folder = self.get(id)?;
        let directory = lock(&folder.directory);
        items
            .iter()
            .map(|reference| {
                let item = item_id(reference)?;
                if item.verify().is_err() || item.path().parent() != Some(folder.path.as_path()) {
                    return Ok(None);
                }
                directory.position(item.path().file_name().unwrap_or_default())
            })
            .collect()
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
        let items = reader.search(&query, &cancel)?;
        drop(search);
        Ok(convert(items, &self.styles()))
    }

    fn close(&self, id: u64) {
        if let Some(folder) = lock(&self.open).remove(&id) {
            folder.cancel.cancel();
            if let Some(query) = lock(&folder.query).take() {
                query.cancel();
            }
        }
    }

    pub fn close_owned(&self, owner: &str) {
        lock(&self.panes).retain(|_, pane| pane.owner != owner);
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
        let sort = *lock(&folder.sort);
        let (count, complete, sort_ms, result) = {
            let mut directory = lock(&folder.directory);
            let result = directory
                .advance(PAGE_SIZE * 4, &folder.cancel)
                .and_then(|_| directory.sort(sort, &folder.cancel));
            (
                directory.count(),
                directory.is_complete(),
                directory.sort_ms(),
                result,
            )
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
                sort_ms,
                error,
            }));
            last = Instant::now();
        }
        if done {
            return;
        }
    }
}

fn sort_core(sort: SortView) -> Sort {
    use crate::contract::SortColumn as Column;
    Sort {
        column: match sort.column {
            Column::Name => SortColumn::Name,
            Column::Modified => SortColumn::Modified,
            Column::Type => SortColumn::Type,
            Column::Size => SortColumn::Size,
        },
        descending: sort.descending,
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

/// Keeps the core's own path (which may carry the `\\?\` prefix) so long paths still verify.
pub fn item_ref(id: &ItemId) -> ItemRef {
    let (volume, index) = id.identity();
    ItemRef {
        path: id.path().to_string_lossy().into_owned(),
        volume,
        index: index.to_string(),
    }
}

pub fn item_id(item: &ItemRef) -> Result<ItemId> {
    Ok(ItemId::from_parts(
        PathBuf::from(&item.path),
        item.volume,
        item.index.parse().context("Invalid item identity")?,
    ))
}

fn convert(page: ItemPage, styles: &Styles) -> Page {
    Page {
        offset: page.offset,
        has_more: page.has_more,
        sorted: false,
        items: page.items.iter().map(|item| entry(item, styles)).collect(),
    }
}

fn entry(item: &Item, styles: &Styles) -> Entry {
    Entry {
        name: item.name().to_string_lossy().into_owned(),
        path: text(&item.path),
        directory: item.directory,
        icon: (!item.directory).then(|| firewol_core::explorer::icons::key(&item.path)),
        size: item.size,
        modified: item.modified,
        hidden: item
            .attributes
            .is_some_and(|attributes| attributes & FILE_ATTRIBUTE_HIDDEN != 0),
        id: item.id.as_ref().map(item_ref),
        style: item
            .id
            .as_ref()
            .filter(|_| item.directory)
            .and_then(|id| styles.get(&FolderKey::from(id)))
            .map(crate::settings::style_view),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspended_and_adopted_tabs_keep_history_and_release_listings() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let first = fixture.path().join("first");
        let second = fixture.path().join("second");
        std::fs::create_dir(&first)?;
        std::fs::create_dir(&second)?;
        let folders = Folders::default();
        let notify: Notify = Arc::new(|_| {});
        for path in [&first, &second] {
            folders.navigate(
                "main",
                "main:1",
                Some(&text(path)),
                Step::New,
                SortView::default(),
                notify.clone(),
            )?;
        }
        assert_eq!(lock(&folders.open).len(), 1);
        folders.suspend("main", "main:1")?;
        assert!(lock(&folders.open).is_empty());
        folders.adopt("main", "detached", "main:1")?;
        assert!(
            folders
                .navigate(
                    "main",
                    "main:1",
                    None,
                    Step::Back,
                    SortView::default(),
                    notify.clone()
                )
                .is_err()
        );
        let back = folders.navigate(
            "detached",
            "main:1",
            None,
            Step::Back,
            SortView::default(),
            notify,
        )?;
        assert_eq!(back.folder.path, text(&first));
        assert!(back.can_forward);
        folders.close_owned("main");
        assert_eq!(lock(&folders.open).len(), 1);
        folders.close_pane("detached", "main:1");
        assert!(lock(&folders.open).is_empty() && lock(&folders.panes).is_empty());
        Ok(())
    }

    #[test]
    fn listings_attach_folder_styles_by_identity() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let styled = fixture.path().join("styled");
        std::fs::create_dir(&styled)?;
        std::fs::create_dir(fixture.path().join("plain"))?;
        std::fs::write(fixture.path().join("file.txt"), "x")?;
        let key = FolderKey::from(&ItemId::at(styled)?);
        let folders = Folders::default();
        folders.use_styles(move || {
            Styles::from([(
                key,
                FolderStyle {
                    alias: "Work".into(),
                    tags: vec!["alpha".into()],
                    ..FolderStyle::default()
                },
            )])
        });
        let navigation = folders.navigate(
            "main",
            "main:1",
            Some(&text(fixture.path())),
            Step::New,
            SortView::default(),
            Arc::new(|_| {}),
        )?;
        let items = &navigation.folder.page.items;
        let alias = |name: &str| {
            items
                .iter()
                .find(|entry| entry.name == name)
                .and_then(|entry| entry.style.as_ref())
                .map(|style| style.alias.clone())
        };
        assert_eq!(alias("styled").as_deref(), Some("Work"));
        assert_eq!(alias("plain"), None);
        assert_eq!(alias("file.txt"), None);
        Ok(())
    }
}
