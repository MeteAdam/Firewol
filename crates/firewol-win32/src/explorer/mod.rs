mod color;
mod controls;
mod editors;
mod form;
mod frame;
mod measure;
mod menu;
mod tabs;
mod theme;
mod workers;

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{
    Cancellation, Filters, FolderKey, FolderStyle, Item, ItemId, ItemPage, Operation, PAGE_SIZE,
    SearchIndex, SearchQuery, display_path, home, page, volume,
};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use self::controls::*;
use self::frame::{Clipboard, Frame, Session};
use self::measure::Measurement;
use self::workers::{Event, FileJob, ListJob, QueryJob, Scope, Snapshot, Ticket, WAKE, Workers};

const CLASS: PCWSTR = w!("FirewolPane");
const EVENT_TIMER: usize = 1;

#[derive(Clone, Default)]
struct History {
    folders: Vec<PathBuf>,
    position: usize,
}

#[derive(Clone, Default)]
struct Tab {
    history: History,
    key: Option<FolderKey>,
    start: Option<PathBuf>,
    draft: SearchDraft,
}

#[derive(Clone, Default)]
struct SearchDraft {
    text: String,
    filters: Filters,
    regex: bool,
}

enum Visit {
    Startup,
    New,
    History(usize),
    Restore,
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

#[derive(Default)]
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

struct Model {
    folder: Option<Folder>,
    tabs: Vec<Tab>,
    active: usize,
    navigation: Option<Navigation>,
    search: Search,
    ticket: Ticket,
}

impl Model {
    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn save_draft(&mut self) {
        let search = &self.search;
        let folder_scope = search
            .index
            .as_ref()
            .is_none_or(|index| matches!(index.scope, Scope::Folder(_)));
        self.tabs[self.active].draft = SearchDraft {
            text: if folder_scope {
                search.text.clone()
            } else {
                String::new()
            },
            filters: search.filters.clone(),
            regex: search.regex,
        };
    }

    fn cancel(&mut self) {
        if let Some(navigation) = self.navigation.take() {
            navigation.cancel.cancel();
        }
        self.search.cancel_query();
        self.search.index = None;
    }

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

enum Drag {
    Tab { index: usize, start: POINT },
    Item(ItemId),
}

struct App {
    window: Cell<HWND>,
    frame: HWND,
    session: Rc<Session>,
    controls: Cell<Option<Controls>>,
    model: RefCell<Model>,
    workers: Workers,
    suppress_search: Cell<bool>,
    deferred_refresh: Cell<bool>,
    drag: RefCell<Option<Drag>>,
    measurement: Option<Measurement>,
}

impl Drop for App {
    fn drop(&mut self) {
        self.model.get_mut().cancel();
    }
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
        let searching = self.model.borrow().search.active();
        let tags = self
            .session
            .settings
            .borrow()
            .folders
            .values()
            .any(|style| !style.tags.is_empty());
        controls.layout(window, searching, tags);
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
        let draft = {
            let mut model = self.model.borrow_mut();
            let active = model.active;
            let tab = &mut model.tabs[active];
            let history = &mut tab.history;
            tab.key = snapshot.id.as_ref().map(FolderKey::from);
            tab.start = None;
            let draft = matches!(visit, Visit::Restore).then(|| tab.draft.clone());
            let notice = match visit {
                Visit::History(position) => {
                    history.position = position;
                    None
                }
                Visit::Refresh(notice) => notice,
                Visit::Startup | Visit::New | Visit::Restore => {
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
            if let Some(draft) = &draft {
                model.search.regex = draft.regex;
                model.search.filters = draft.filters.clone();
            }
            draft
        };
        set_text(controls.address, display_path(&path));
        if !refresh {
            self.suppress_search.set(true);
            set_text(
                controls.search,
                draft.as_ref().map_or("", |draft| draft.text.as_str()),
            );
            self.suppress_search.set(false);
        }
        self.update_tab_label(controls);
        self.navigation_buttons(controls);
        self.search_chrome(controls);
        self.layout(window, controls);
        if !self.model.borrow().search.active() {
            reset_list(controls.list, count);
            self.folder_status();
        }
        if draft.is_some_and(|draft| !draft.text.trim().is_empty())
            && let Err(error) = self.search_input(window, controls)
        {
            self.error(&error);
        }
        let record = !refresh
            && Frame::at(self.frame)
                .and_then(|frame| frame.active())
                .is_none_or(|pane| pane.window.get() == window)
            && self.session.settings.borrow().last_folder.as_ref() != Some(&path);
        if record
            && let Err(error) = self
                .session
                .update(|settings| settings.last_folder = Some(path))
        {
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
                model.tab().history.position > 0,
                model.tab().history.position + 1 < model.tab().history.folders.len(),
                current.is_some_and(|path| path.parent().is_some()),
                current.is_some_and(|path| self.session.settings.borrow().pins.contains(path)),
            )
        };
        unsafe {
            let _ = EnableWindow(controls.back, back);
            let _ = EnableWindow(controls.forward, forward);
            let _ = EnableWindow(controls.up, up);
        }
        set_text(controls.pin, if pinned { "Unpin" } else { "Pin" });
    }

    fn pin_current(&self) -> Result<()> {
        let path = self.current_path()?;
        self.session.update(|settings| {
            if let Some(index) = settings.pins.iter().position(|pin| pin == &path) {
                settings.pins.remove(index);
            } else {
                settings.pins.push(path);
            }
        })?;
        self.session.settings_changed();
        Ok(())
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
            drive.map_or_else(|| "NTFS search".into(), |drive| format!("Stop {drive}:")),
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
        self.workers
            .index(ticket, scope, self.session.data.clone(), cancel);
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
                Some(self.frame),
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
                    if self.model.borrow().navigation.is_none()
                        && self.deferred_refresh.replace(false)
                    {
                        self.refresh_after_operation(None);
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
            let history = &model.tab().history;
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

    fn open_item(&self, item: &Item, parent: bool) -> Result<()> {
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
                Some(self.frame),
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

    fn refresh_after_operation(&self, message: Option<String>) {
        let (pending, searching) = {
            let model = self.model.borrow();
            (model.navigation.is_some(), model.search.active())
        };
        if pending {
            self.deferred_refresh.set(true);
        } else if let Ok(path) = self.current_path()
            && let Err(error) = self.navigate(path, Visit::Refresh(message.clone()))
        {
            self.error(&error);
            return;
        }
        if (pending || searching)
            && let Some(message) = message
        {
            self.status(message);
        }
    }

    fn submit(&self, job: FileJob, consume_cut: bool) -> Result<()> {
        self.session
            .submit(self.frame, self.window.get(), job, consume_cut)?;
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
                ensure!(
                    model.navigation.is_none(),
                    "Wait for the destination folder to open"
                );
                let clipboard = self.session.clipboard.borrow();
                let clipboard = clipboard
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
            OPEN | CONTAINING => self.open_item(&item, action == CONTAINING),
            COPY | CUT => {
                let id = item
                    .id
                    .context("The selected item's identity is unavailable")?;
                ensure!(
                    !self.session.busy(),
                    "Wait for the current operation to finish"
                );
                self.session.clipboard.replace(Some(Clipboard {
                    item: id,
                    cut: action == CUT,
                }));
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
                if let Some(name) = form::rename(self.frame, name)? {
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
                        Some(self.frame),
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
            PIN => self.pin_current(),
            NEW_TAB => self.new_tab(),
            CLOSE_TAB => self.close_tab(),
            REGEX => {
                let mut model = self.model.borrow_mut();
                model.search.regex = !model.search.regex;
                drop(model);
                let _ = unsafe { InvalidateRect(Some(controls.regex), None, false) };
                self.search_input(window, controls)
            }
            FILTERS => {
                let previous = self.model.borrow().search.filters.clone();
                if let Some(filters) = form::filters(self.frame, &previous)? {
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

fn keyboard(app: &App, window: HWND, controls: Controls, message: &MSG) -> Result<bool> {
    if message.message != WM_KEYDOWN && message.message != WM_SYSKEYDOWN {
        return Ok(false);
    }
    let control = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
    let alt = unsafe { GetKeyState(i32::from(VK_MENU.0)) } < 0;
    let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
    let key = VIRTUAL_KEY(message.wParam.0 as u16);
    if app.drag.borrow().is_some() {
        if key == VK_ESCAPE {
            app.drag.replace(None);
            let _ = unsafe { ReleaseCapture() };
            app.folder_status();
        }
        return Ok(true);
    }
    let list = message.hwnd == controls.list;
    match (key, control, alt, list) {
        (VK_T, true, false, _) => app.new_tab()?,
        (VK_W, true, false, _) => app.close_tab()?,
        (VK_TAB, true, false, _) => app.cycle_tab(shift)?,
        (VK_N, true, false, _) if shift => {
            let active = app.model.borrow().active;
            app.detach_tab(active, None)?;
        }
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
        (VK_APPS, false, false, true) => menu::popup(app, window, controls, false)?,
        (VK_F10, false, false, true) if shift => menu::popup(app, window, controls, false)?,
        _ => return Ok(false),
    }
    Ok(true)
}

impl App {
    fn open(
        frame: HWND,
        session: Rc<Session>,
        mut tab: Tab,
        start: PathBuf,
        visit: Visit,
        measurement: Option<Measurement>,
    ) -> Result<Rc<Self>> {
        if tab.history.folders.is_empty() {
            tab.start = Some(start.clone());
        }
        let app = Rc::new(Self {
            window: Cell::default(),
            frame,
            controls: Cell::default(),
            model: RefCell::new(Model {
                tabs: vec![tab],
                active: 0,
                folder: None,
                navigation: None,
                search: Search::default(),
                ticket: 0,
            }),
            workers: Workers::start(session.threads.clone()),
            session,
            suppress_search: Cell::default(),
            deferred_refresh: Cell::default(),
            drag: RefCell::default(),
            measurement,
        });
        if let Some(probe) = &app.measurement {
            probe.mark("workers ready");
        }
        app.navigate(start.clone(), visit)?;
        let instance = app.session.instance;
        let guard = Window(unsafe {
            CreateWindowExW(
                WS_EX_CONTROLPARENT,
                CLASS,
                w!("Folder pane"),
                WS_CHILD | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(frame),
                None,
                Some(instance),
                Some(std::ptr::from_ref(app.as_ref()).cast()),
            )?
        });
        let window = guard.0;
        app.window.set(window);
        let controls = Controls::create(window, instance)?;
        if let Some(probe) = &app.measurement {
            probe.mark("controls created");
            ensure!(
                unsafe { SetTimer(Some(window), EVENT_TIMER, 15, None) } != 0,
                "Start the Firewol measurement timer"
            );
        }
        app.controls.set(Some(controls));
        app.workers.bind(window);
        tabs::install(controls.tabs)?;
        controls.install_theme(window)?;
        app.sync();
        set_text(controls.address, display_path(&start));
        app.search_chrome(controls);
        app.status("Opening folder…");
        app.events(window, controls)?;
        show(window, true);
        std::mem::forget(guard);
        Ok(app)
    }

    fn focus(&self) {
        if let Some(controls) = self.controls.get() {
            let _ = unsafe { SetFocus(Some(controls.list)) };
        }
    }

    fn keyboard(&self, message: &MSG) -> Result<bool> {
        self.controls.get().map_or(Ok(false), |controls| {
            keyboard(self, self.window.get(), controls, message)
        })
    }

    fn sync(&self) {
        if let Some(controls) = self.controls.get() {
            self.session.theme.borrow().list(controls.list);
            self.sync_tabs(controls);
            self.navigation_buttons(controls);
            self.layout(self.window.get(), controls);
            let _ = unsafe { InvalidateRect(Some(controls.list), None, true) };
        }
    }

    fn transfer(&self, item: ItemId, move_item: bool) -> Result<()> {
        let target = Frame::at(self.frame)
            .and_then(|frame| frame.other(self.window.get()))
            .context("Open split view first")?;
        let into = {
            let model = target.model.borrow();
            ensure!(
                model.navigation.is_none(),
                "Wait for the other pane to open its folder"
            );
            model
                .folder
                .as_ref()
                .and_then(|folder| folder.id.clone())
                .context("The other pane's folder identity is unavailable")?
        };
        let operation = if move_item {
            Operation::Move { item, into }
        } else {
            Operation::Copy { item, into }
        };
        self.submit(FileJob::Run(operation), false)
    }

    fn customize_folder(&self, item: Option<Item>) -> Result<()> {
        let id = if let Some(item) = item {
            ensure!(item.directory, "Select a folder to customize it");
            item.id
        } else {
            self.model
                .borrow()
                .folder
                .as_ref()
                .and_then(|folder| folder.id.clone())
        }
        .context("The folder's identity is unavailable")?;
        id.verify()?;
        let key = FolderKey::from(&id);
        let previous = self
            .session
            .settings
            .borrow()
            .folders
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if let Some(style) = editors::folder(self.frame, &previous)? {
            id.verify()?;
            self.session.update(|settings| {
                if style == FolderStyle::default() {
                    settings.folders.remove(&key);
                } else {
                    settings.folders.insert(key, style);
                }
            })?;
            self.session.settings_changed();
        }
        Ok(())
    }

    fn drop_target(&self, point: POINT) -> bool {
        let Some(target) = Frame::at(self.frame).and_then(|frame| frame.other(self.window.get()))
        else {
            return false;
        };
        let Some(controls) = target.controls.get() else {
            return false;
        };
        unsafe {
            let child = WindowFromPoint(point);
            child == controls.list || IsChild(controls.list, child).as_bool()
        }
    }

    fn row(&self, index: i32) -> Option<Item> {
        if index < 0 {
            return None;
        }
        self.model
            .borrow_mut()
            .item(index as u32, &self.workers)
            .cloned()
    }

    fn list_text(&self, info: &mut LVITEMW) {
        let text_requested =
            info.mask.contains(LVIF_TEXT) && info.cchTextMax > 0 && !info.pszText.is_null();
        let icon_requested = info.mask.contains(LVIF_IMAGE) && info.iSubItem == 0;
        if info.iItem < 0 || !(text_requested || icon_requested) {
            return;
        }
        let mut model = self.model.borrow_mut();
        let item = model.item(info.iItem as u32, &self.workers);
        if icon_requested {
            info.iImage = item.map_or(I_IMAGENONE, |item| i32::from(item.directory));
        }
        if !text_requested {
            return;
        }
        let settings = self.session.settings.borrow();
        let style = folder_style(&settings, item);
        let buffer =
            unsafe { std::slice::from_raw_parts_mut(info.pszText.0, info.cchTextMax as usize) };
        match (info.iSubItem, item) {
            (0, Some(item)) => write_cell(
                buffer,
                style
                    .filter(|style| !style.alias.is_empty())
                    .map_or_else(|| item.name(), |style| OsStr::new(&style.alias)),
            ),
            (1, Some(item)) => write_cell(
                buffer,
                OsStr::new(if item.directory { "Folder" } else { "File" }),
            ),
            (2, Some(item)) => write_cell(
                buffer,
                &display_path(item.path.parent().unwrap_or(Path::new(""))),
            ),
            (3, Some(_)) if style.is_some_and(|style| !style.tags.is_empty()) => {
                let tags = style
                    .map(|style| style.tags.join(" · "))
                    .unwrap_or_default();
                write_cell(buffer, OsStr::new(&tags));
            }
            (0, None) => write_cell(buffer, OsStr::new("Loading…")),
            _ => write_cell(buffer, OsStr::new("")),
        }
    }

    fn list_draw(&self, list: HWND, draw: &mut NMLVCUSTOMDRAW) -> LRESULT {
        let stage = draw.nmcd.dwDrawStage;
        if stage == CDDS_PREPAINT {
            return LRESULT(CDRF_NOTIFYITEMDRAW as isize);
        }
        if stage != CDDS_ITEMPREPAINT && stage != CDDS_ITEMPOSTPAINT {
            return LRESULT(CDRF_DODEFAULT as isize);
        }
        let row = draw.nmcd.dwItemSpec;
        let mut model = self.model.borrow_mut();
        let item = model.item(row as u32, &self.workers);
        if !item.is_some_and(|item| item.directory) {
            return LRESULT(CDRF_DODEFAULT as isize);
        }
        let theme = self.session.theme.borrow();
        if stage == CDDS_ITEMPREPAINT {
            if !draw.nmcd.uItemState.contains(CDIS_SELECTED) {
                draw.clrText = theme::colorref(theme.folder_label);
            }
            return LRESULT(CDRF_NOTIFYPOSTPAINT as isize);
        }
        let mut icon = RECT {
            left: LVIR_ICON as i32,
            ..Default::default()
        };
        if send(
            list,
            LVM_GETITEMRECT,
            row,
            std::ptr::from_mut(&mut icon) as isize,
        ) != 0
        {
            let settings = self.session.settings.borrow();
            theme.folder_icon(draw.nmcd.hdc, icon, folder_style(&settings, item));
        }
        LRESULT(CDRF_DODEFAULT as isize)
    }
}

fn folder_style<'a>(
    settings: &'a firewol_core::explorer::Settings,
    item: Option<&Item>,
) -> Option<&'a FolderStyle> {
    let item = item.filter(|item| item.directory)?;
    settings.folders.get(&FolderKey::from(item.id.as_ref()?))
}

fn point(lparam: LPARAM) -> POINT {
    POINT {
        x: lparam.0 as i16 as i32,
        y: (lparam.0 >> 16) as i16 as i32,
    }
}

fn screen_point(window: HWND, lparam: LPARAM) -> POINT {
    let mut point = point(lparam);
    let _ = unsafe { ClientToScreen(window, &mut point) };
    point
}

fn set_cursor(cursor: PCWSTR) {
    if let Ok(cursor) = unsafe { LoadCursorW(None, cursor) } {
        unsafe { SetCursor(Some(cursor)) };
    }
}

// The parent keeps the pane alive until its children have been destroyed.
fn pane_at(window: HWND) -> Option<Rc<App>> {
    let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const App;
    if data.is_null() {
        return None;
    }
    unsafe {
        Rc::increment_strong_count(data);
        Some(Rc::from_raw(data))
    }
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
    let Some(app) = pane_at(window) else {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    if message == WM_NCDESTROY {
        app.workers.bind(HWND::default());
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
    }
    let Some(controls) = app.controls.get() else {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    let result = match message {
        WM_SIZE => {
            app.layout(window, controls);
            Ok(())
        }
        WM_ERASEBKGND => {
            app.session
                .theme
                .borrow()
                .erase(HDC(wparam.0 as *mut _), window);
            return LRESULT(1);
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            let surface = HWND(lparam.0 as *mut _) == controls.address
                || HWND(lparam.0 as *mut _) == controls.search;
            return app
                .session
                .theme
                .borrow()
                .control_color(HDC(wparam.0 as *mut _), surface);
        }
        WM_MEASUREITEM if lparam.0 != 0 => {
            let info = unsafe { &mut *(lparam.0 as *mut MEASUREITEMSTRUCT) };
            if menu::measure(info) {
                return LRESULT(1);
            }
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        WM_DRAWITEM if lparam.0 != 0 => {
            let info = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            let theme = app.session.theme.borrow();
            if info.CtlType == ODT_BUTTON {
                let glyph = match info.CtlID as usize {
                    BACK => Some("‹"),
                    FORWARD => Some("›"),
                    UP => Some("↑"),
                    NEW_TAB => Some("+"),
                    CLOSE_TAB => Some("×"),
                    _ => None,
                };
                let selected = info.CtlID as usize == REGEX && app.model.borrow().search.regex;
                theme.button(info, glyph, selected, false, theme.background);
            } else if !menu::draw(info, &theme) {
                return unsafe { DefWindowProcW(window, message, wparam, lparam) };
            }
            return LRESULT(1);
        }
        WM_MOUSEMOVE if matches!(app.drag.borrow().as_ref(), Some(Drag::Item(_))) => {
            set_cursor(if app.drop_target(screen_point(window, lparam)) {
                IDC_SIZEALL
            } else {
                IDC_NO
            });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let drag = app.drag.take();
            if let Some(Drag::Item(item)) = drag {
                let _ = unsafe { ReleaseCapture() };
                if app.drop_target(screen_point(window, lparam)) {
                    let move_item = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
                    app.transfer(item, move_item)
                } else {
                    app.folder_status();
                    Ok(())
                }
            } else {
                return unsafe { DefWindowProcW(window, message, wparam, lparam) };
            }
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            app.drag.replace(None);
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        WAKE => app.events(window, controls),
        WM_TIMER if wparam.0 == EVENT_TIMER => app.events(window, controls).and_then(|()| {
            app.measurement
                .as_ref()
                .map_or(Ok(()), |probe| probe.tick(&app, window, controls))
        }),
        WM_COMMAND => app.command(window, controls, wparam.0 & 0xffff, (wparam.0 >> 16) as u32),
        WM_NOTIFY if lparam.0 != 0 => {
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            if header.hwndFrom == controls.tabs && header.code == TCN_SELCHANGE {
                let index = send(controls.tabs, TCM_GETCURSEL, 0, 0);
                if index >= 0 {
                    app.select_tab(index as usize)
                } else {
                    Ok(())
                }
            } else if header.hwndFrom != controls.list {
                return unsafe { DefWindowProcW(window, message, wparam, lparam) };
            } else {
                match header.code {
                    NM_SETFOCUS => {
                        if let Some(frame) = Frame::at(app.frame) {
                            frame.activate(window);
                        }
                        Ok(())
                    }
                    LVN_GETDISPINFOW => {
                        let info = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
                        app.list_text(&mut info.item);
                        Ok(())
                    }
                    NM_CUSTOMDRAW => {
                        let info = unsafe { &mut *(lparam.0 as *mut NMLVCUSTOMDRAW) };
                        return app.list_draw(controls.list, info);
                    }
                    NM_DBLCLK => {
                        let activation = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
                        if activation.iItem >= 0 {
                            app.action(window, controls, OPEN, app.row(activation.iItem))
                        } else {
                            Ok(())
                        }
                    }
                    NM_RCLICK => menu::popup(&app, window, controls, true),
                    LVN_BEGINDRAG => {
                        let drag = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
                        if Frame::at(app.frame)
                            .and_then(|frame| frame.other(window))
                            .is_some()
                            && let Some(item) = app.row(drag.iItem).and_then(|item| item.id)
                        {
                            app.drag.replace(Some(Drag::Item(item)));
                            unsafe { SetCapture(window) };
                            app.status("Drop on the other pane to copy · hold Shift to move");
                        }
                        Ok(())
                    }
                    _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
                }
            }
        }
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    };
    if let Err(error) = result {
        app.error(&error);
    }
    LRESULT(0)
}

fn register(instance: HINSTANCE) -> Result<()> {
    let class = WNDCLASSW {
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpfnWndProc: Some(procedure),
        lpszClassName: CLASS,
        ..Default::default()
    };
    ensure!(
        unsafe { RegisterClassW(&class) } != 0,
        "Register Firewol pane class"
    );
    Ok(())
}

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

pub fn run(path: Option<PathBuf>) -> Result<()> {
    frame::run(path)
}
