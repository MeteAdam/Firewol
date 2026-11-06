mod controls;
mod form;
mod measure;
mod workers;

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{
    Cancellation, Filters, Item, ItemId, ItemPage, Operation, PAGE_SIZE, SearchIndex, SearchQuery,
    Settings, data_dir, display_path, home, page, volume,
};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{COLOR_3DFACE, HBRUSH, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use self::controls::*;
use self::measure::Measurement;
use self::workers::{Event, FileJob, ListJob, QueryJob, Scope, Snapshot, Ticket, WAKE, Workers};

const CLASS: PCWSTR = w!("FirewolExplorer");
const EVENT_TIMER: usize = 1;

#[derive(Default)]
struct History {
    folders: Vec<PathBuf>,
    position: usize,
}

enum Visit {
    Startup,
    New,
    History(usize),
    Refresh(Option<String>),
}

struct Navigation {
    ticket: Ticket,
    cancel: Cancellation,
    visit: Visit,
}

struct Folder {
    ticket: Ticket,
    path: PathBuf,
    id: Option<ItemId>,
    count: u32,
    complete: bool,
    cache: VecDeque<ItemPage>,
    requested: HashSet<u32>,
    notice: Option<String>,
}

impl Folder {
    fn remember(&mut self, page: ItemPage) {
        self.cache.retain(|previous| previous.offset != page.offset);
        self.cache.push_back(page);
        if self.cache.len() > 8 {
            self.cache.pop_front();
        }
    }

    fn item(&mut self, row: u32, workers: &Workers) -> Option<&Item> {
        let offset = row / PAGE_SIZE * PAGE_SIZE;
        let position = self
            .cache
            .iter()
            .position(|page| page.offset == offset && (row - offset) < page.items.len() as u32);
        if let Some(position) = position {
            if let Some(hit) = self.cache.remove(position) {
                self.cache.push_back(hit);
            }
            return self.cache.back()?.items.get((row - offset) as usize);
        }
        if row < self.count && self.requested.insert(offset) {
            let _ = workers.listing.send(ListJob::Page(self.ticket, offset));
        }
        None
    }

    fn status(&self) -> String {
        self.notice.clone().unwrap_or_else(|| {
            if self.complete {
                format!("{} items", self.count)
            } else {
                format!("Reading folder · {} items", self.count)
            }
        })
    }
}

struct Index {
    ticket: Ticket,
    scope: Scope,
    cancel: Cancellation,
    ready: Option<(SearchIndex, u64)>,
}

impl Drop for Index {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct Search {
    text: String,
    filters: Filters,
    regex: bool,
    index: Option<Index>,
    pending: Option<(Ticket, Cancellation)>,
    results: Option<ItemPage>,
    offset: u32,
}

impl Search {
    fn active(&self) -> bool {
        !self.text.trim().is_empty()
    }

    fn cancel_query(&mut self) {
        if let Some((_, cancel)) = self.pending.take() {
            cancel.cancel();
        }
    }
}

struct Clipboard {
    item: ItemId,
    cut: bool,
}

struct Model {
    folder: Option<Folder>,
    history: History,
    navigation: Option<Navigation>,
    search: Search,
    settings: Settings,
    save_settings: bool,
    clipboard: Option<Clipboard>,
    operating: bool,
    consume_cut: bool,
    ticket: Ticket,
}

impl Model {
    fn ticket(&mut self) -> Ticket {
        self.ticket += 1;
        self.ticket
    }

    fn item(&mut self, row: u32, workers: &Workers) -> Option<&Item> {
        if self.search.active() {
            return self.search.results.as_ref()?.items.get(row as usize);
        }
        self.folder.as_mut()?.item(row, workers)
    }
}

struct App {
    controls: Cell<Option<Controls>>,
    model: RefCell<Model>,
    workers: Workers,
    data: PathBuf,
    suppress_search: Cell<bool>,
    measurement: Option<Measurement>,
}

impl App {
    fn status(&self, text: impl AsRef<OsStr>) {
        if let Some(controls) = self.controls.get() {
            set_text(controls.status, text);
        }
    }

    fn error(&self, error: &anyhow::Error) {
        self.status(format!("{error:#}"));
    }

    fn layout(&self, window: HWND, controls: Controls) {
        let (pins, searching) = {
            let model = self.model.borrow();
            (!model.settings.pins.is_empty(), model.search.active())
        };
        controls.layout(window, pins, searching);
    }

    fn navigate(&self, path: PathBuf, visit: Visit) -> Result<()> {
        let mut model = self.model.borrow_mut();
        if let Some(previous) = model.navigation.take() {
            previous.cancel.cancel();
        }
        let ticket = model.ticket();
        let cancel = Cancellation::default();
        self.workers
            .listing
            .send(ListJob::Open(ticket, path, cancel.clone()))?;
        model.navigation = Some(Navigation {
            ticket,
            cancel,
            visit,
        });
        drop(model);
        self.status("Opening folder…");
        Ok(())
    }

    fn commit_visit(
        &self,
        window: HWND,
        controls: Controls,
        ticket: Ticket,
        snapshot: Snapshot,
        visit: Visit,
    ) {
        if let Some(probe) = &self.measurement {
            probe.mark("directory page received");
        }
        let path = snapshot.path;
        let count = snapshot.count;
        let refresh = matches!(visit, Visit::Refresh(_));
        let changed_settings = {
            let mut model = self.model.borrow_mut();
            let history = &mut model.history;
            let notice = match visit {
                Visit::History(position) => {
                    history.position = position;
                    None
                }
                Visit::Refresh(notice) => notice,
                Visit::Startup | Visit::New => {
                    if history.folders.get(history.position) != Some(&path) {
                        history.folders.truncate(history.position + 1);
                        history.folders.push(path.clone());
                        history.position = history.folders.len() - 1;
                    }
                    None
                }
            };
            model.folder = Some(Folder {
                ticket,
                path: path.clone(),
                id: snapshot.id,
                count,
                complete: snapshot.complete,
                cache: VecDeque::from([snapshot.page]),
                requested: HashSet::new(),
                notice,
            });
            if !refresh {
                model.search.cancel_query();
                model.search.index = None;
                model.search.results = None;
                model.search.text.clear();
                model.search.offset = 0;
            }
            let changed = model.settings.last_folder.as_ref() != Some(&path);
            model.settings.last_folder = Some(path.clone());
            changed
        };
        set_text(controls.address, display_path(&path));
        if !refresh {
            self.suppress_search.set(true);
            set_text(controls.search, "");
            self.suppress_search.set(false);
        }
        self.navigation_buttons(controls);
        self.search_chrome(controls);
        self.layout(window, controls);
        if !self.model.borrow().search.active() {
            reset_list(controls.list, count);
            self.folder_status();
        }
        if changed_settings && let Err(error) = self.save() {
            self.error(&error);
        }
    }

    fn folder_status(&self) {
        let status = self.model.borrow().folder.as_ref().map(Folder::status);
        if let Some(status) = status {
            self.status(status);
        }
    }

    fn navigation_buttons(&self, controls: Controls) {
        let (back, forward, up, pinned) = {
            let model = self.model.borrow();
            let current = model.folder.as_ref().map(|folder| &folder.path);
            (
                model.history.position > 0,
                model.history.position + 1 < model.history.folders.len(),
                current.is_some_and(|path| path.parent().is_some()),
                current.is_some_and(|path| model.settings.pins.contains(path)),
            )
        };
        unsafe {
            let _ = EnableWindow(controls.back, back);
            let _ = EnableWindow(controls.forward, forward);
            let _ = EnableWindow(controls.up, up);
        }
        set_text(
            controls.pin,
            if pinned { "Unpin folder" } else { "Pin folder" },
        );
    }

    fn save(&self) -> Result<()> {
        let model = self.model.borrow();
        if model.save_settings {
            model.settings.save(&self.data)?;
        }
        Ok(())
    }

    fn pin_current(&self, window: HWND, controls: Controls) -> Result<()> {
        {
            let mut model = self.model.borrow_mut();
            let path = model
                .folder
                .as_ref()
                .context("Open a folder first")?
                .path
                .clone();
            if let Some(position) = model.settings.pins.iter().position(|pin| pin == &path) {
                model.settings.pins.remove(position);
            } else {
                model.settings.pins.push(path);
            }
        }
        self.pins(window, controls);
        self.navigation_buttons(controls);
        self.save()
    }

    fn pins(&self, window: HWND, controls: Controls) {
        let pins = self.model.borrow().settings.pins.clone();
        send(controls.pins, LB_RESETCONTENT, 0, 0);
        for pin in pins {
            let text = wide(&display_path(&pin));
            send(controls.pins, LB_ADDSTRING, 0, text.as_ptr() as isize);
        }
        self.layout(window, controls);
    }

    fn pin_open(&self, controls: Controls) -> Result<()> {
        let row = send(controls.pins, LB_GETCURSEL, 0, 0);
        let path = self.model.borrow().settings.pins.get(row as usize).cloned();
        if let Some(path) = path {
            self.navigate(path, Visit::New)?;
        }
        Ok(())
    }

    fn remove_selected_pin(&self, window: HWND, controls: Controls) -> Result<()> {
        let row = send(controls.pins, LB_GETCURSEL, 0, 0) as usize;
        {
            let mut model = self.model.borrow_mut();
            if row >= model.settings.pins.len() {
                return Ok(());
            }
            model.settings.pins.remove(row);
        }
        self.pins(window, controls);
        self.navigation_buttons(controls);
        self.save()
    }

    fn search_chrome(&self, controls: Controls) {
        let (drive, previous, next) = {
            let model = self.model.borrow();
            let search = &model.search;
            let drive = search.index.as_ref().and_then(|index| match index.scope {
                Scope::Volume(drive) => Some(drive),
                Scope::Folder(_) => None,
            });
            (
                drive,
                search.results.is_some() && search.offset > 0,
                search.results.as_ref().is_some_and(|page| page.has_more),
            )
        };
        set_text(
            controls.volume,
            drive.map_or_else(
                || "NTFS search…".into(),
                |drive| format!("Stop {drive}: search"),
            ),
        );
        cue(
            controls.search,
            if drive.is_some() {
                w!("Search the selected NTFS volume")
            } else {
                w!("Search this folder and its subfolders")
            },
        );
        unsafe {
            let _ = EnableWindow(controls.previous, previous);
            let _ = EnableWindow(controls.next, next);
        }
    }

    fn start_index(&self, scope: Scope) {
        let (ticket, cancel) = {
            let mut model = self.model.borrow_mut();
            model.search.cancel_query();
            let ticket = model.ticket();
            let cancel = Cancellation::default();
            model.search.index = Some(Index {
                ticket,
                scope: scope.clone(),
                cancel: cancel.clone(),
                ready: None,
            });
            (ticket, cancel)
        };
        self.status(match &scope {
            Scope::Folder(_) => "Indexing the selected folder…".into(),
            Scope::Volume(drive) => format!("Waiting for UAC and indexing {drive}: …"),
        });
        self.workers.index(ticket, scope, self.data.clone(), cancel);
    }

    fn search_input(&self, window: HWND, controls: Controls) -> Result<()> {
        if self.suppress_search.get() {
            return Ok(());
        }
        let (active, mode_changed, count, scope) = {
            let mut model = self.model.borrow_mut();
            model.search.cancel_query();
            let was_active = model.search.active();
            model.search.text = get_text(controls.search).to_string_lossy().into_owned();
            model.search.regex = send(controls.regex, BM_GETCHECK, 0, 0) == BST_CHECKED.0 as isize;
            model.search.results = None;
            model.search.offset = 0;
            let active = model.search.active();
            if !active
                && model.search.index.as_ref().is_some_and(|index| {
                    index.ready.is_none() && matches!(index.scope, Scope::Folder(_))
                })
            {
                model.search.index = None;
            }
            let count = model.folder.as_ref().map_or(0, |folder| folder.count);
            let scope = model
                .folder
                .as_ref()
                .map(|folder| Scope::Folder(folder.path.clone()));
            (active, active != was_active, count, scope)
        };
        if mode_changed {
            self.layout(window, controls);
        }
        self.search_chrome(controls);
        if active {
            reset_list(controls.list, 0);
            if self.model.borrow().search.index.is_none() {
                if let Some(scope) = scope {
                    self.start_index(scope);
                }
            } else {
                self.query()?;
            }
        } else {
            reset_list(controls.list, count);
            let volume_status = self.model.borrow().search.index.as_ref().and_then(|index| {
                let Scope::Volume(drive) = index.scope else {
                    return None;
                };
                Some(index.ready.as_ref().map_or_else(
                    || format!("Waiting for UAC and indexing {drive}: …"),
                    |(_, count)| format!("NTFS index ready · {count} items"),
                ))
            });
            if let Some(status) = volume_status {
                self.status(status);
            } else {
                self.folder_status();
            }
        }
        Ok(())
    }

    fn query(&self) -> Result<()> {
        let mut model = self.model.borrow_mut();
        let Some((index, _)) = model
            .search
            .index
            .as_ref()
            .and_then(|index| index.ready.clone())
        else {
            return Ok(());
        };
        if !model.search.active() {
            return Ok(());
        }
        model.search.cancel_query();
        let ticket = model.ticket();
        let cancel = Cancellation::default();
        let query = SearchQuery {
            text: model.search.text.clone(),
            regex: model.search.regex,
            filters: model.search.filters.clone(),
            page: page(model.search.offset),
        };
        self.workers
            .search
            .send(QueryJob(ticket, index, query, cancel.clone()))?;
        model.search.pending = Some((ticket, cancel));
        Ok(())
    }

    fn search_page(&self, controls: Controls, forward: bool) -> Result<()> {
        {
            let mut model = self.model.borrow_mut();
            let search = &mut model.search;
            let Some(page) = &search.results else {
                return Ok(());
            };
            if forward && !page.has_more {
                return Ok(());
            }
            search.offset = if forward {
                page.offset.saturating_add(PAGE_SIZE)
            } else {
                page.offset.saturating_sub(PAGE_SIZE)
            };
            search.results = None;
        }
        reset_list(controls.list, 0);
        self.search_chrome(controls);
        self.query()
    }

    fn volume(&self, window: HWND, controls: Controls) -> Result<()> {
        let (volume, folder) = {
            let model = self.model.borrow();
            let volume = model
                .search
                .index
                .as_ref()
                .is_some_and(|index| matches!(index.scope, Scope::Volume(_)));
            let folder = model
                .folder
                .as_ref()
                .context("Open a folder first")?
                .path
                .clone();
            (volume, folder)
        };
        if volume {
            self.model.borrow_mut().search.index = None;
            return self.search_input(window, controls);
        }
        let drive = volume::drive_of(&folder)
            .context("Volume search requires a local NTFS drive letter")?;
        let message = wide(OsStr::new(&format!(
            "Index accessible items on {drive}: for this session? Windows will request administrator approval for the read-only index helper."
        )));
        if unsafe {
            MessageBoxW(
                Some(window),
                PCWSTR(message.as_ptr()),
                w!("Enable NTFS search"),
                MB_OKCANCEL | MB_ICONINFORMATION | MB_DEFBUTTON2,
            )
        } != IDOK
        {
            return Ok(());
        }
        self.start_index(Scope::Volume(drive));
        self.search_input(window, controls)?;
        self.search_chrome(controls);
        Ok(())
    }

    fn events(&self, window: HWND, controls: Controls) -> Result<()> {
        for _ in 0..40 {
            let Ok(event) = self.workers.events.try_recv() else {
                break;
            };
            match event {
                Event::Navigated(ticket, result) => {
                    let visit = {
                        let mut model = self.model.borrow_mut();
                        if !model
                            .navigation
                            .as_ref()
                            .is_some_and(|navigation| navigation.ticket == ticket)
                        {
                            continue;
                        }
                        model.navigation.take().map(|navigation| navigation.visit)
                    };
                    let Some(visit) = visit else { continue };
                    match result {
                        Ok(snapshot) => {
                            self.commit_visit(window, controls, ticket, snapshot, visit)
                        }
                        Err(error) => {
                            let current = self.current_path().ok();
                            if let Some(path) = current {
                                set_text(controls.address, display_path(&path));
                                self.error(&error);
                            } else if matches!(visit, Visit::Startup) {
                                self.navigate(home()?, Visit::New)?;
                            } else {
                                self.error(&error);
                            }
                        }
                    }
                }
                Event::Count(ticket, count, complete) => {
                    let browsing = {
                        let mut model = self.model.borrow_mut();
                        let Some(folder) = model
                            .folder
                            .as_mut()
                            .filter(|folder| folder.ticket == ticket)
                        else {
                            continue;
                        };
                        folder.count = count;
                        folder.complete = complete;
                        !model.search.active()
                    };
                    if browsing {
                        send(
                            controls.list,
                            LVM_SETITEMCOUNT,
                            count as usize,
                            (LVSICF_NOINVALIDATEALL | LVSICF_NOSCROLL) as isize,
                        );
                        self.folder_status();
                    }
                }
                Event::ListPage(ticket, offset, result) => {
                    let last = {
                        let mut model = self.model.borrow_mut();
                        let browsing = !model.search.active();
                        let Some(folder) = model
                            .folder
                            .as_mut()
                            .filter(|folder| folder.ticket == ticket)
                        else {
                            continue;
                        };
                        folder.requested.remove(&offset);
                        match result {
                            Ok(page) => {
                                let last = offset + page.items.len() as u32;
                                folder.remember(page);
                                browsing.then_some(last)
                            }
                            Err(error) => {
                                drop(model);
                                self.error(&error);
                                None
                            }
                        }
                    };
                    if let Some(last) = last
                        && last > offset
                    {
                        send(
                            controls.list,
                            LVM_REDRAWITEMS,
                            offset as usize,
                            last as isize - 1,
                        );
                    }
                }
                Event::ListError(ticket, error) => {
                    if self
                        .model
                        .borrow()
                        .folder
                        .as_ref()
                        .is_some_and(|folder| folder.ticket == ticket)
                    {
                        self.error(&error);
                    }
                }
                Event::Indexed(ticket, index, count) => {
                    let (active, volume) = {
                        let mut model = self.model.borrow_mut();
                        let Some(current) = model
                            .search
                            .index
                            .as_mut()
                            .filter(|index| index.ticket == ticket)
                        else {
                            continue;
                        };
                        current.ready = Some((index, count));
                        let volume = matches!(current.scope, Scope::Volume(_));
                        (model.search.active(), volume)
                    };
                    if active {
                        self.query()?;
                    } else if volume {
                        self.status(format!("NTFS index ready · {count} items"));
                    }
                }
                Event::IndexError(ticket, error) => {
                    {
                        let mut model = self.model.borrow_mut();
                        if !model
                            .search
                            .index
                            .as_ref()
                            .is_some_and(|index| index.ticket == ticket)
                        {
                            continue;
                        }
                        model.search.cancel_query();
                        model.search.index = None;
                        model.search.results = None;
                    }
                    let count = {
                        let model = self.model.borrow();
                        if model.search.active() {
                            0
                        } else {
                            model.folder.as_ref().map_or(0, |folder| folder.count)
                        }
                    };
                    reset_list(controls.list, count);
                    self.search_chrome(controls);
                    self.error(&error);
                }
                Event::Results(ticket, result) => {
                    let mut model = self.model.borrow_mut();
                    if !model
                        .search
                        .pending
                        .as_ref()
                        .is_some_and(|(pending, _)| *pending == ticket)
                    {
                        continue;
                    }
                    model.search.pending = None;
                    match result {
                        Ok(page) => {
                            let count = page.items.len() as u32;
                            let status =
                                format!("{count} results on page {}", page.offset / PAGE_SIZE + 1);
                            model.search.results = Some(page);
                            drop(model);
                            reset_list(controls.list, count);
                            self.search_chrome(controls);
                            self.status(status);
                        }
                        Err(error) => {
                            model.search.results = None;
                            drop(model);
                            reset_list(controls.list, 0);
                            self.search_chrome(controls);
                            self.error(&error);
                        }
                    }
                }
                Event::Operation(result) => {
                    let active = {
                        let mut model = self.model.borrow_mut();
                        model.operating = false;
                        if result.is_ok() && model.consume_cut {
                            model.clipboard = None;
                        }
                        model.consume_cut = false;
                        model.search.active()
                    };
                    match result {
                        Ok(result) if !active && self.model.borrow().navigation.is_none() => {
                            self.navigate(
                                self.current_path()?,
                                Visit::Refresh(Some(result.message)),
                            )?;
                        }
                        Ok(result) => self.status(result.message),
                        Err(error) => self.error(&error),
                    }
                }
            }
        }
        Ok(())
    }

    fn current_path(&self) -> Result<PathBuf> {
        self.model
            .borrow()
            .folder
            .as_ref()
            .map(|folder| folder.path.clone())
            .context("Open a folder first")
    }

    fn open_address(&self, controls: Controls) -> Result<()> {
        let text = get_text(controls.address);
        let path = PathBuf::from(text);
        let path = if path.is_absolute() {
            path
        } else {
            ensure!(
                !path.as_os_str().is_empty()
                    && !path.has_root()
                    && !matches!(
                        path.components().next(),
                        Some(std::path::Component::Prefix(_))
                    ),
                "Enter an absolute path or a folder relative to the current one"
            );
            let current = self.current_path()?;
            PathBuf::from(display_path(&current)).join(path)
        };
        self.navigate(path, Visit::New)
    }

    fn history(&self, forward: bool) -> Result<()> {
        let target = {
            let model = self.model.borrow();
            let history = &model.history;
            let position = if forward {
                history.position.checked_add(1)
            } else {
                history.position.checked_sub(1)
            };
            position.and_then(|position| {
                history
                    .folders
                    .get(position)
                    .map(|path| (position, path.clone()))
            })
        };
        if let Some((position, path)) = target {
            self.navigate(path, Visit::History(position))?;
        }
        Ok(())
    }

    fn selection(&self, controls: Controls) -> Option<Item> {
        let row = send(
            controls.list,
            LVM_GETNEXTITEM,
            usize::MAX,
            LVNI_SELECTED as isize,
        );
        if row < 0 {
            return None;
        }
        self.model
            .borrow_mut()
            .item(row as u32, &self.workers)
            .cloned()
    }

    fn open_item(&self, window: HWND, item: &Item, parent: bool) -> Result<()> {
        let id = item
            .id
            .as_ref()
            .context("The selected item's identity is unavailable")?;
        id.verify()?;
        if parent || item.directory {
            let path = if parent {
                id.path()
                    .parent()
                    .context("This item has no containing folder")?
            } else {
                id.path()
            };
            return self.navigate(path.to_owned(), Visit::New);
        }
        let path = wide(&display_path(id.path()));
        let status = unsafe {
            ShellExecuteW(
                Some(window),
                w!("open"),
                PCWSTR(path.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        }
        .0 as isize;
        ensure!(
            status > 32,
            "Windows could not open the selected item (code {status})"
        );
        Ok(())
    }

    fn submit(&self, job: FileJob, consume_cut: bool) -> Result<()> {
        let mut model = self.model.borrow_mut();
        ensure!(!model.operating, "A file operation is already running");
        self.workers.files.send(job)?;
        model.operating = true;
        model.consume_cut = consume_cut;
        drop(model);
        self.status("Working…");
        Ok(())
    }

    fn action(
        &self,
        window: HWND,
        controls: Controls,
        action: usize,
        item: Option<Item>,
    ) -> Result<()> {
        if action == UNDO {
            return self.submit(FileJob::Undo, false);
        }
        if action == REFRESH {
            if self.model.borrow().search.active() {
                let index_ready = self
                    .model
                    .borrow()
                    .search
                    .index
                    .as_ref()
                    .is_some_and(|index| index.ready.is_some());
                if index_ready {
                    return self.query();
                }
                return self.search_input(window, controls);
            }
            return self.navigate(self.current_path()?, Visit::Refresh(None));
        }
        if action == PASTE {
            let (operation, cut) = {
                let model = self.model.borrow();
                let clipboard = model
                    .clipboard
                    .as_ref()
                    .context("Copy or cut an item in Firewol first")?;
                let into = model
                    .folder
                    .as_ref()
                    .and_then(|folder| folder.id.clone())
                    .context("The current folder's identity is unavailable")?;
                let item = clipboard.item.clone();
                (
                    if clipboard.cut {
                        Operation::Move { item, into }
                    } else {
                        Operation::Copy { item, into }
                    },
                    clipboard.cut,
                )
            };
            return self.submit(FileJob::Run(operation), cut);
        }
        let item = item.context("Select an item first")?;
        match action {
            OPEN | CONTAINING => self.open_item(window, &item, action == CONTAINING),
            COPY | CUT => {
                let id = item
                    .id
                    .context("The selected item's identity is unavailable")?;
                let mut model = self.model.borrow_mut();
                ensure!(!model.operating, "Wait for the current operation to finish");
                model.clipboard = Some(Clipboard {
                    item: id,
                    cut: action == CUT,
                });
                drop(model);
                self.status(if action == CUT {
                    "Cut · navigate to a folder and paste"
                } else {
                    "Copied to Firewol clipboard · navigate to a folder and paste"
                });
                Ok(())
            }
            RENAME => {
                let name = item.name().to_owned();
                let id = item
                    .id
                    .context("The selected item's identity is unavailable")?;
                if let Some(name) = form::rename(window, name)? {
                    self.submit(FileJob::Run(Operation::Rename { item: id, name }), false)?;
                }
                Ok(())
            }
            RECYCLE => {
                let name = wide(OsStr::new(&format!(
                    "Move “{}” to the Recycle Bin?",
                    item.name().to_string_lossy()
                )));
                let id = item
                    .id
                    .context("The selected item's identity is unavailable")?;
                if unsafe {
                    MessageBoxW(
                        Some(window),
                        PCWSTR(name.as_ptr()),
                        w!("Recycle item"),
                        MB_OKCANCEL | MB_ICONQUESTION | MB_DEFBUTTON2,
                    )
                } == IDOK
                {
                    self.submit(FileJob::Run(Operation::Recycle { item: id }), false)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn command(&self, window: HWND, controls: Controls, command: usize, code: u32) -> Result<()> {
        match command {
            SEARCH if code == EN_CHANGE => self.search_input(window, controls),
            GO => self.open_address(controls),
            BACK | FORWARD => self.history(command == FORWARD),
            UP => {
                if let Some(path) = self.current_path()?.parent() {
                    self.navigate(path.to_owned(), Visit::New)?;
                }
                Ok(())
            }
            PIN => self.pin_current(window, controls),
            PINS if code == LBN_DBLCLK => self.pin_open(controls),
            REGEX => self.search_input(window, controls),
            FILTERS => {
                let previous = self.model.borrow().search.filters.clone();
                if let Some(filters) = form::filters(window, &previous)? {
                    self.model.borrow_mut().search.filters = filters;
                    self.search_input(window, controls)?;
                }
                Ok(())
            }
            VOLUME => self.volume(window, controls),
            PREVIOUS | NEXT => self.search_page(controls, command == NEXT),
            _ => Ok(()),
        }
    }
}

const OPEN: usize = 501;
const CONTAINING: usize = 502;
const COPY: usize = 503;
const CUT: usize = 504;
const PASTE: usize = 505;
const RENAME: usize = 506;
const RECYCLE: usize = 507;
const UNDO: usize = 508;
const REFRESH: usize = 509;

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

fn context_menu(app: &App, window: HWND, controls: Controls, mouse: bool) -> Result<()> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point)? };
    let item = if mouse {
        let mut hit = LVHITTESTINFO {
            pt: point,
            ..Default::default()
        };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(controls.list, &mut hit.pt);
        }
        let row = send(
            controls.list,
            LVM_HITTEST,
            0,
            std::ptr::from_mut(&mut hit) as isize,
        );
        (row >= 0)
            .then(|| {
                app.model
                    .borrow_mut()
                    .item(row as u32, &app.workers)
                    .cloned()
            })
            .flatten()
    } else {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(controls.list, &mut rect)? };
        point = POINT {
            x: rect.left + 25,
            y: rect.top + 32,
        };
        app.selection(controls)
    };
    let (selected, searching, clipboard, idle) = {
        let model = app.model.borrow();
        (
            item.as_ref().is_some_and(|item| item.id.is_some()),
            model.search.active(),
            model.clipboard.is_some(),
            !model.operating,
        )
    };
    let menu = Menu(unsafe { CreatePopupMenu()? });
    let add = |id, text, enabled| unsafe {
        AppendMenuW(
            menu.0,
            MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED },
            id,
            text,
        )
    };
    let separator = || unsafe { AppendMenuW(menu.0, MF_SEPARATOR, 0, None) };
    add(OPEN, w!("↵  Open\tEnter"), selected)?;
    if searching {
        add(CONTAINING, w!("    Open containing folder"), selected)?;
    }
    separator()?;
    add(COPY, w!("    Copy\tCtrl+C"), selected && idle)?;
    add(CUT, w!("    Cut\tCtrl+X"), selected && idle)?;
    add(
        PASTE,
        w!("    Paste in current folder\tCtrl+V"),
        clipboard && idle,
    )?;
    add(RENAME, w!("    Rename…\tF2"), selected && idle)?;
    add(
        RECYCLE,
        w!("    Move to Recycle Bin…\tDel"),
        selected && idle,
    )?;
    separator()?;
    add(UNDO, w!("    Undo\tCtrl+Z"), idle)?;
    add(REFRESH, w!("    Refresh\tF5"), true)?;
    let command = unsafe {
        TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            window,
            None,
        )
        .0
    } as usize;
    if command != 0 {
        app.action(window, controls, command, item)?;
    }
    Ok(())
}

fn keyboard(app: &App, window: HWND, controls: Controls, message: &MSG) -> Result<bool> {
    if message.message != WM_KEYDOWN && message.message != WM_SYSKEYDOWN {
        return Ok(false);
    }
    let control = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
    let alt = unsafe { GetKeyState(i32::from(VK_MENU.0)) } < 0;
    let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
    let key = VIRTUAL_KEY(message.wParam.0 as u16);
    let list = message.hwnd == controls.list;
    match (key, control, alt, list) {
        (VK_L, true, false, _) | (VK_F, true, false, _) => {
            let focus = if key == VK_L {
                controls.address
            } else {
                controls.search
            };
            let _ = unsafe { SetFocus(Some(focus)) };
            send(focus, EM_SETSEL, 0, -1);
        }
        (VK_LEFT, false, true, _) => app.history(false)?,
        (VK_RIGHT, false, true, _) => app.history(true)?,
        (VK_UP, false, true, _) => app.command(window, controls, UP, 0)?,
        (VK_RETURN, false, false, _) if message.hwnd == controls.address => {
            app.open_address(controls)?
        }
        (VK_RETURN, false, false, _) if message.hwnd == controls.pins => app.pin_open(controls)?,
        (VK_DELETE, false, false, _) if message.hwnd == controls.pins => {
            app.remove_selected_pin(window, controls)?
        }
        (VK_RETURN, false, false, true) => {
            app.action(window, controls, OPEN, app.selection(controls))?
        }
        (VK_ESCAPE, false, false, _) if app.model.borrow().search.active() => {
            set_text(controls.search, "")
        }
        (VK_F5, false, false, _) => app.action(window, controls, REFRESH, None)?,
        (VK_F2, false, false, true) => {
            app.action(window, controls, RENAME, app.selection(controls))?
        }
        (VK_DELETE, false, false, true) => {
            app.action(window, controls, RECYCLE, app.selection(controls))?
        }
        (VK_C, true, false, true) => app.action(window, controls, COPY, app.selection(controls))?,
        (VK_X, true, false, true) => app.action(window, controls, CUT, app.selection(controls))?,
        (VK_V, true, false, true) => app.action(window, controls, PASTE, None)?,
        (VK_Z, true, false, true) => app.action(window, controls, UNDO, None)?,
        (VK_APPS, false, false, true) => context_menu(app, window, controls, false)?,
        (VK_F10, false, false, true) if shift => context_menu(app, window, controls, false)?,
        _ => return Ok(false),
    }
    Ok(true)
}

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const App;
    let Some(app) = (unsafe { data.as_ref() }) else {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    match message {
        WM_GETMINMAXINFO => {
            let bounds = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            bounds.ptMinTrackSize.x = 760;
            bounds.ptMinTrackSize.y = 400;
            return LRESULT(0);
        }
        WM_CLOSE if app.model.borrow().operating => {
            app.status("Wait for the current file operation to finish");
            return LRESULT(0);
        }
        WM_DESTROY => {
            app.workers.bind(HWND::default());
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    let result = match app.controls.get() {
        Some(controls) => match message {
            WM_SIZE => {
                app.layout(window, controls);
                Ok(())
            }
            WAKE => app.events(window, controls),
            WM_TIMER if wparam.0 == EVENT_TIMER => app.events(window, controls).and_then(|()| {
                app.measurement
                    .as_ref()
                    .map_or(Ok(()), |probe| probe.tick(app, window, controls))
            }),
            WM_COMMAND => app.command(window, controls, wparam.0 & 0xffff, (wparam.0 >> 16) as u32),
            WM_NOTIFY if lparam.0 != 0 => {
                let header = unsafe { &*(lparam.0 as *const NMHDR) };
                if header.hwndFrom != controls.list {
                    return unsafe { DefWindowProcW(window, message, wparam, lparam) };
                }
                match header.code {
                    LVN_GETDISPINFOW => {
                        let info = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                        let info = &mut info.item;
                        let text_requested = info.mask.contains(LVIF_TEXT)
                            && info.cchTextMax > 0
                            && !info.pszText.is_null();
                        let icon_requested = info.mask.contains(LVIF_IMAGE) && info.iSubItem == 0;
                        if info.iItem >= 0 && (text_requested || icon_requested) {
                            let mut model = app.model.borrow_mut();
                            let item = model.item(info.iItem as u32, &app.workers);
                            if icon_requested {
                                info.iImage =
                                    item.map_or(I_IMAGENONE, |item| i32::from(item.directory));
                            }
                            if !text_requested {
                                return LRESULT(0);
                            }
                            let buffer = unsafe {
                                std::slice::from_raw_parts_mut(
                                    info.pszText.0,
                                    info.cchTextMax as usize,
                                )
                            };
                            match (info.iSubItem, item) {
                                (0, Some(item)) => write_cell(buffer, item.name()),
                                (1, Some(item)) => write_cell(
                                    buffer,
                                    OsStr::new(if item.directory { "Folder" } else { "File" }),
                                ),
                                (2, Some(item)) => write_cell(
                                    buffer,
                                    &display_path(item.path.parent().unwrap_or(Path::new(""))),
                                ),
                                (0, None) => write_cell(buffer, OsStr::new("Loading…")),
                                _ => write_cell(buffer, OsStr::new("")),
                            }
                        }
                        return LRESULT(0);
                    }
                    NM_DBLCLK => {
                        let activation = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
                        if activation.iItem >= 0 {
                            app.action(window, controls, OPEN, app.selection(controls))
                        } else {
                            Ok(())
                        }
                    }
                    NM_RCLICK => context_menu(app, window, controls, true),
                    _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
                }
            }
            _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
        },
        None => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    };
    if let Err(error) = result {
        app.error(&error);
    }
    LRESULT(0)
}

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

pub fn run(path: Option<PathBuf>) -> Result<()> {
    let measurement = Measurement::requested();
    let data = data_dir()?;
    let loaded = Settings::load(&data);
    let (settings, settings_error) = match loaded {
        Ok(settings) => (settings, None),
        Err(error) => (Settings::default(), Some(error)),
    };
    let (first, visit) = match path {
        Some(path) => (path, Visit::New),
        None => (
            settings.last_folder.clone().unwrap_or(home()?),
            Visit::Startup,
        ),
    };
    if let Some(probe) = &measurement {
        probe.mark("settings loaded");
    }
    let app = Box::new(App {
        controls: Cell::new(None),
        workers: Workers::start(),
        data,
        suppress_search: Cell::new(false),
        measurement,
        model: RefCell::new(Model {
            folder: None,
            history: History::default(),
            navigation: None,
            search: Search {
                text: String::new(),
                filters: Filters::default(),
                regex: false,
                index: None,
                pending: None,
                results: None,
                offset: 0,
            },
            settings,
            save_settings: settings_error.is_none(),
            clipboard: None,
            operating: false,
            consume_cut: false,
            ticket: 0,
        }),
    });
    if let Some(probe) = &app.measurement {
        probe.mark("workers ready");
    }
    app.navigate(first, visit)?;
    let common = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES,
    };
    unsafe {
        ensure!(
            InitCommonControlsEx(&common).as_bool(),
            "Initialize Windows ListView"
        );
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        form::register(instance)?;
        let class = WNDCLASSW {
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpfnWndProc: Some(procedure),
            lpszClassName: CLASS,
            hbrBackground: HBRUSH((COLOR_3DFACE.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        ensure!(RegisterClassW(&class) != 0, "Register Firewol window class");
        let window = Window(CreateWindowExW(
            WS_EX_CONTROLPARENT,
            CLASS,
            w!("Firewol"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            970,
            665,
            None,
            None,
            Some(instance),
            Some(std::ptr::from_ref(app.as_ref()).cast()),
        )?);
        let controls = Controls::create(window.0, instance)?;
        if let Some(probe) = &app.measurement {
            probe.mark("controls created");
        }
        app.controls.set(Some(controls));
        app.workers.bind(window.0);
        app.pins(window.0, controls);
        app.navigation_buttons(controls);
        app.search_chrome(controls);
        if app.measurement.is_some() {
            ensure!(
                SetTimer(Some(window.0), EVENT_TIMER, 15, None) != 0,
                "Start the Firewol measurement timer"
            );
        }
        app.status("Opening folder…");
        app.events(window.0, controls)?;
        show(window.0, true);
        let _ = UpdateWindow(window.0);
        if let Some(probe) = &app.measurement {
            probe.mark("window shown");
        }
        if let Some(error) = settings_error {
            app.error(&error);
        }
        let _ = SetFocus(Some(controls.list));
        let mut message = MSG::default();
        loop {
            let next = GetMessageW(&mut message, None, 0, 0).0;
            ensure!(next >= 0, "Read the Win32 message queue");
            if next == 0 {
                break;
            }
            match keyboard(&app, window.0, controls, &message) {
                Ok(false) if !IsDialogMessageW(window.0, &message).as_bool() => {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                Err(error) => app.error(&error),
                _ => {}
            }
        }
    }
    let App { model, workers, .. } = *app;
    let mut model = model.into_inner();
    model.search.cancel_query();
    if let Some(navigation) = &model.navigation {
        navigation.cancel.cancel();
    }
    drop(model);
    workers.shutdown();
    Ok(())
}
