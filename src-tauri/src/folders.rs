use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{
    Cancellation, Directory, Filters, FolderKey, FolderStyle, History, Item, ItemId, ItemPage,
    PAGE_SIZE, SavedView, SearchIndex, SearchQuery, SearchReader, Sort, SortColumn, data_dir,
    display_path, icons, page, watch_directory,
};

use crate::contract::{
    Entry, FolderDisplay, FolderProgress, FolderView, IndexState, ItemRef, Navigation, Page,
    SearchCount, SearchFilters, SearchRequest, SearchScope, Segment, SortView, Step,
};
use crate::volumes::{self, Volumes};

/// Result counts stop here; the interface shows "more than" beyond it.
pub const SEARCH_COUNT_CAP: u32 = 100_000;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

pub enum Notice {
    Progress(FolderProgress),
    Index(IndexState),
    /// Something changed inside this open folder.
    Changed(u64),
}

pub type Notify = Arc<dyn Fn(Notice) + Send + Sync>;
pub type Styles = BTreeMap<FolderKey, FolderStyle>;
type StyleSource = Arc<dyn Fn() -> Styles + Send + Sync>;
type HiddenSource = Arc<dyn Fn() -> bool + Send + Sync>;
type ViewSource = Arc<dyn Fn(FolderKey) -> Option<SavedView> + Send + Sync>;

/// Open folder snapshots, keyed by the id the interface holds, and the panes showing them.
#[derive(Default)]
pub struct Folders {
    next: AtomicU64,
    open: Mutex<HashMap<u64, Arc<Folder>>>,
    panes: Mutex<HashMap<String, Pane>>,
    styles: OnceLock<StyleSource>,
    show_hidden: OnceLock<HiddenSource>,
    views: OnceLock<ViewSource>,
    pub volumes: Arc<Volumes>,
    running_indexes: Arc<AtomicUsize>,
}

// Each open folder holds a database and a thread, and each index a crawler and a watcher. Far above
// what four windows of tabs need, low enough that a runaway caller cannot exhaust the machine.
const MAX_PANES: usize = 256;
const MAX_OPEN_PER_WINDOW: usize = 128;
const MAX_INDEXES: usize = 32;
const MAX_QUERY_BYTES: usize = 4096;

struct Folder {
    owner: String,
    path: PathBuf,
    /// Stops the folder index and every request when the folder closes.
    cancel: Cancellation,
    /// Stops the current listing pass; a reload starts a new pass but keeps the folder index.
    listing: Mutex<Cancellation>,
    directory: Mutex<Directory>,
    sort: Mutex<Sort>,
    search: Mutex<Search>,
    /// The latest search and its cancellation; pages of the same search share it.
    query: Mutex<Option<(QueryKey, Cancellation)>>,
}

#[derive(PartialEq)]
struct QueryKey {
    text: String,
    regex: bool,
    filters: SearchFilters,
    scope: SearchScope,
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

    /// Sets where listings read the Show/Hide choice from, once at startup.
    pub fn use_show_hidden(&self, source: impl Fn() -> bool + Send + Sync + 'static) {
        let _ = self.show_hidden.set(Arc::new(source));
    }

    /// Sets where listings read each folder's saved view from, once at startup.
    pub fn use_views(
        &self,
        source: impl Fn(FolderKey) -> Option<SavedView> + Send + Sync + 'static,
    ) {
        let _ = self.views.set(Arc::new(source));
    }

    fn saved_view(&self, folder: &ItemId) -> Option<FolderDisplay> {
        let view = self.views.get()?(FolderKey::from(folder))?;
        Some(FolderDisplay {
            grid: view.grid,
            tile: view.tile,
        })
    }

    fn directory(&self, path: &Path) -> Result<Directory> {
        let mut directory = Directory::open(path)?;
        if self.show_hidden.get().is_some_and(|source| !source()) {
            directory.hide_hidden_items();
        }
        Ok(directory)
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
        let (target, position, sequence, reuse) = {
            let mut panes = lock(&self.panes);
            ensure!(
                panes.len() < MAX_PANES || panes.contains_key(pane),
                "Too many tabs are open"
            );
            let state = panes.entry(pane.to_owned()).or_default();
            ensure!(
                state.owner.is_empty() || state.owner == owner,
                "This tab belongs to another window"
            );
            state.owner = owner.to_owned();
            state.sequence += 1;
            // A reload re-reads the open folder in place, so its live search index survives.
            let reuse = match step {
                Step::Reload => state.folder,
                _ => None,
            };
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
            (target, position, state.sequence, reuse)
        };
        let folder = match reuse.and_then(|id| self.get(id).ok().map(|folder| (id, folder))) {
            Some((id, folder)) => self.reopen(id, &folder, sort, notify)?,
            None => self.open(owner, &target, sort, notify)?,
        };
        let mut panes = lock(&self.panes);
        let state = panes.get_mut(pane);
        if state
            .as_ref()
            .is_none_or(|state| state.sequence != sequence || state.owner != owner)
        {
            drop(panes);
            // A reused folder still belongs to the pane; the newer navigation replaces it.
            if reuse != Some(folder.id) {
                self.close(folder.id);
            }
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
        let previous = state
            .folder
            .replace(folder.id)
            .filter(|id| *id != folder.id);
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
        ensure!(
            lock(&self.open)
                .values()
                .filter(|folder| folder.owner == owner)
                .count()
                < MAX_OPEN_PER_WINDOW,
            "Too many folders are open in this window"
        );
        let cancel = Cancellation::default();
        let listing = Cancellation::default();
        let mut directory = self.directory(path)?;
        directory.advance(PAGE_SIZE, &listing)?;
        let sort = sort_core(sort);
        directory.sort(sort, &listing)?;
        let sort_ms = directory.sort_ms();
        let first = directory.list_dir(page(0), &cancel)?;
        let path = directory.path().to_owned();
        let (count, complete) = (directory.count(), directory.is_complete());
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let folder = Arc::new(Folder {
            owner: owner.to_owned(),
            path: path.clone(),
            cancel,
            listing: Mutex::new(listing.clone()),
            directory: Mutex::new(directory),
            sort: Mutex::new(sort),
            search: Mutex::default(),
            query: Mutex::default(),
        });
        lock(&self.open).insert(id, folder.clone());
        // Lives as long as this open folder; a reload keeps it. Holds no reference to the folder.
        let (watched, stop, changed) = (path.clone(), folder.cancel.clone(), notify.clone());
        thread::spawn(move || {
            watch_directory(&watched, &stop, || changed(Notice::Changed(id)));
        });
        if !complete {
            thread::spawn(move || enumerate(id, &folder, &listing, &notify));
        }
        Ok(self.view(id, &path, count, complete, sort_ms, first))
    }

    /// Re-reads an open folder under the same id: the listing restarts, the folder index and its
    /// watcher keep running.
    fn reopen(
        &self,
        id: u64,
        folder: &Arc<Folder>,
        sort: SortView,
        notify: Notify,
    ) -> Result<FolderView> {
        let listing = Cancellation::default();
        let mut directory = self.directory(&folder.path)?;
        directory.advance(PAGE_SIZE, &listing)?;
        let sort = sort_core(sort);
        directory.sort(sort, &listing)?;
        let sort_ms = directory.sort_ms();
        let first = directory.list_dir(page(0), &listing)?;
        let (count, complete) = (directory.count(), directory.is_complete());
        // The previous pass stops before it can touch the new listing.
        std::mem::replace(&mut *lock(&folder.listing), listing.clone()).cancel();
        *lock(&folder.directory) = directory;
        *lock(&folder.sort) = sort;
        if folder.cancel.is_cancelled() {
            listing.cancel();
            anyhow::bail!("The folder was closed");
        }
        if !complete {
            let folder = folder.clone();
            thread::spawn(move || enumerate(id, &folder, &listing, &notify));
        }
        Ok(self.view(id, &folder.path, count, complete, sort_ms, first))
    }

    fn view(
        &self,
        id: u64,
        path: &Path,
        count: u32,
        complete: bool,
        sort_ms: f64,
        first: ItemPage,
    ) -> FolderView {
        let identity = ItemId::at(path.to_owned()).ok();
        FolderView {
            id,
            view: identity.as_ref().and_then(|folder| self.saved_view(folder)),
            item: identity.as_ref().map(item_ref),
            icon: firewol_core::explorer::icons::folder_icon(path),
            path: text(path),
            segments: segments(path),
            count,
            complete,
            sort_ms,
            page: Page {
                sorted: complete,
                ..convert(first, &self.styles())
            },
        }
    }

    pub fn page(&self, id: u64, offset: u32, sort: SortView) -> Result<Page> {
        let folder = self.get(id)?;
        let styles = self.styles();
        let sort = sort_core(sort);
        *lock(&folder.sort) = sort;
        let (rows, sorted) = {
            let mut directory = lock(&folder.directory);
            directory.sort(sort, &folder.cancel)?;
            let rows = directory.rows(page(offset), &folder.cancel)?;
            (rows, directory.is_sorted())
        };
        Ok(Page {
            sorted,
            ..convert(rows.items(&folder.cancel)?, &styles)
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
        // The folder check comes first: only an item of this folder is worth opening.
        let names = items
            .iter()
            .map(|reference| {
                let item = item_id(reference)?;
                Ok(
                    (item.path().parent() == Some(folder.path.as_path()) && item.verify().is_ok())
                        .then(|| item.path().file_name().unwrap_or_default().to_owned()),
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let directory = lock(&folder.directory);
        names
            .into_iter()
            .map(|name| match name {
                Some(name) => directory.position(&name),
                None => Ok(None),
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
        ensure!(
            self.running_indexes.load(Ordering::Relaxed) < MAX_INDEXES,
            "Too many folders are being indexed"
        );
        let data = data_dir()?;
        let index = SearchIndex::create(&data)?;
        search.index = Some(index.clone());
        drop(search);
        self.running_indexes.fetch_add(1, Ordering::Relaxed);
        let running = self.running_indexes.clone();
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
            running.fetch_sub(1, Ordering::Relaxed);
        });
        Ok(IndexState {
            id,
            ready: false,
            count: 0,
            error: None,
        })
    }

    pub fn search(&self, request: SearchRequest) -> Result<Page> {
        let page = page(request.offset);
        let items = self.read_search(&request, |reader, query, cancel| {
            reader.search(&SearchQuery { page, ..query }, cancel)
        })?;
        Ok(convert(items, &self.styles()))
    }

    pub fn search_count(&self, request: SearchRequest) -> Result<SearchCount> {
        let count = self.read_search(&request, |reader, query, cancel| {
            reader.count(&query, SEARCH_COUNT_CAP, cancel)
        })?;
        Ok(SearchCount {
            count: count.min(u64::from(SEARCH_COUNT_CAP)) as u32,
            more: count > u64::from(SEARCH_COUNT_CAP),
        })
    }

    /// Cancels the folder's previous search when this one differs, then runs `read` with
    /// the reader of the requested scope.
    fn read_search<T>(
        &self,
        request: &SearchRequest,
        read: impl FnOnce(&mut SearchReader, SearchQuery, &Cancellation) -> Result<T>,
    ) -> Result<T> {
        ensure!(
            request.text.len() <= MAX_QUERY_BYTES,
            "The search term is too long"
        );
        let folder = self.get(request.id)?;
        let key = QueryKey {
            text: request.text.clone(),
            regex: request.regex,
            filters: request.filters.clone(),
            scope: request.scope.clone(),
        };
        let cancel = {
            let mut current = lock(&folder.query);
            match &*current {
                Some((previous, cancel)) if *previous == key => cancel.clone(),
                _ => {
                    let cancel = Cancellation::default();
                    if let Some((_, previous)) = current.replace((key, cancel.clone())) {
                        previous.cancel();
                    }
                    cancel
                }
            }
        };
        let filters = &request.filters;
        let query = SearchQuery {
            text: request.text.clone(),
            regex: request.regex,
            filters: Filters {
                extension: filters.extension.clone(),
                min_bytes: filters.min_bytes,
                max_bytes: filters.max_bytes,
                modified_after: filters.modified_after,
                modified_before: filters.modified_before,
            },
            page: page(0),
        };
        if let SearchScope::Volume { drive } = &request.scope {
            return self.volumes.read(volumes::drive(drive)?, |reader| {
                read(reader, query, &cancel)
            });
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
        read(
            reader.as_mut().context("Open the folder index")?,
            query,
            &cancel,
        )
    }

    fn close(&self, id: u64) {
        if let Some(folder) = lock(&self.open).remove(&id) {
            folder.cancel.cancel();
            lock(&folder.listing).cancel();
            if let Some((_, query)) = lock(&folder.query).take() {
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
                lock(&folder.listing).cancel();
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

fn enumerate(id: u64, folder: &Folder, listing: &Cancellation, notify: &Notify) {
    let mut last = Instant::now();
    loop {
        let sort = *lock(&folder.sort);
        let (count, complete, sort_ms, result) = {
            let mut directory = lock(&folder.directory);
            let result = directory
                .advance(PAGE_SIZE * 4, listing)
                .and_then(|_| directory.sort(sort, listing));
            (
                directory.count(),
                directory.is_complete(),
                directory.sort_ms(),
                result,
            )
        };
        if listing.is_cancelled() {
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
    Segment {
        name,
        path,
        icon: None,
    }
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
        icon: if item.directory {
            item.attributes
                .and_then(|attributes| icons::folder_key(&item.path, attributes))
        } else {
            Some(icons::key(&item.path))
        },
        thumb: item
            .modified
            .filter(|_| !item.directory)
            .and_then(|modified| icons::thumbnail_key(&item.path, modified)),
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
    fn panes_and_open_folders_are_capped() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let path = text(fixture.path());
        let folders = Folders::default();
        let notify: Notify = Arc::new(|_| {});
        let go = |pane: &str| {
            folders.navigate(
                "main",
                pane,
                Some(&path),
                Step::New,
                SortView::default(),
                notify.clone(),
            )
        };
        for pane in 0..MAX_OPEN_PER_WINDOW {
            go(&format!("main:{pane}"))?;
        }
        let refused = go("main:one-too-many").err().context("A folder too many")?;
        assert!(refused.to_string().contains("Too many folders"));
        lock(&folders.panes)
            .extend((0..MAX_PANES).map(|pane| (format!("x:{pane}"), Pane::default())));
        let refused = go("main:new").err().context("A pane too many")?;
        assert!(refused.to_string().contains("Too many tabs"));
        Ok(())
    }

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
    fn a_reload_rereads_the_open_folder_and_keeps_its_index_state() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        std::fs::write(fixture.path().join("one.txt"), "1")?;
        let folders = Folders::default();
        let notify: Notify = Arc::new(|_| {});
        let path = text(fixture.path());
        let go = |step| {
            folders.navigate(
                "main",
                "main:1",
                Some(&path),
                step,
                SortView::default(),
                notify.clone(),
            )
        };
        let first = go(Step::New)?.folder;
        // Stands in for a built folder index, which lives in the folder's search state.
        lock(&folders.get(first.id)?.search).ready = Some(1);
        std::fs::write(fixture.path().join("two.txt"), "2")?;
        let reloaded = go(Step::Reload)?.folder;
        assert_eq!(reloaded.id, first.id);
        assert_eq!(reloaded.count, 2);
        assert_eq!(lock(&folders.get(first.id)?.search).ready, Some(1));
        assert_eq!(lock(&folders.open).len(), 1);
        let other = go(Step::New)?.folder;
        assert_ne!(other.id, first.id, "a new navigation opens a new folder");
        assert!(folders.get(first.id).is_err());
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
