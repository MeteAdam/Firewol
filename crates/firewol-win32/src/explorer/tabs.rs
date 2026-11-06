use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{display_path, home};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PWSTR;

use super::controls::{Controls, reset_list, send, set_text, wide};
use super::workers::ListJob;
use super::{App, Drag, Frame, Search, Tab, Visit, pane_at, point, screen_point, set_cursor};

impl Tab {
    fn path(&self) -> Option<&Path> {
        self.history
            .folders
            .get(self.history.position)
            .or(self.start.as_ref())
            .map(PathBuf::as_path)
    }
}

pub(super) fn install(tabs: HWND) -> Result<()> {
    ensure!(
        unsafe { SetWindowSubclass(tabs, Some(tab_procedure), 0, 0) }.as_bool(),
        "Enable tab dragging"
    );
    Ok(())
}

fn paint_tabs(app: &App, tabs: HWND, dc: HDC) {
    let theme = app.session.theme.borrow();
    theme.erase(dc, tabs);
    let mut bounds = RECT::default();
    if unsafe { GetClientRect(tabs, &mut bounds) }.is_err() {
        return;
    }
    let model = app.model.borrow();
    for (index, tab) in model.tabs.iter().enumerate() {
        let mut rect = RECT::default();
        if send(
            tabs,
            TCM_GETITEMRECT,
            index,
            std::ptr::from_mut(&mut rect) as isize,
        ) == 0
            || rect.right < 0
            || rect.left >= bounds.right
        {
            continue;
        }
        let selected = model.active == index;
        theme.tab(dc, rect, &app.tab_label(tab), selected);
        if selected && unsafe { GetFocus() } == tabs {
            rect.left += 3;
            rect.top += 3;
            rect.right -= 3;
            rect.bottom -= 4;
            let _ = unsafe { DrawFocusRect(dc, &rect) };
        }
    }
}

impl App {
    fn tab_label(&self, tab: &Tab) -> std::ffi::OsString {
        let settings = self.session.settings.borrow();
        if let Some(style) = tab.key.and_then(|key| settings.folders.get(&key))
            && !style.alias.is_empty()
        {
            return style.alias.as_str().into();
        }
        tab.path().map_or_else(
            || "Folder".into(),
            |path| {
                path.file_name()
                    .map_or_else(|| display_path(path), OsStr::to_owned)
            },
        )
    }

    pub(super) fn sync_tabs(&self, controls: Controls) {
        let (labels, active) = {
            let model = self.model.borrow();
            (
                model
                    .tabs
                    .iter()
                    .map(|tab| self.tab_label(tab))
                    .collect::<Vec<_>>(),
                model.active,
            )
        };
        send(controls.tabs, TCM_DELETEALLITEMS, 0, 0);
        for (index, label) in labels.iter().enumerate() {
            let mut label = wide(label);
            let item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(label.as_mut_ptr()),
                ..Default::default()
            };
            send(
                controls.tabs,
                TCM_INSERTITEMW,
                index,
                std::ptr::from_ref(&item) as isize,
            );
        }
        send(controls.tabs, TCM_SETCURSEL, active, 0);
    }

    pub(super) fn update_tab_label(&self, controls: Controls) {
        let (active, label) = {
            let model = self.model.borrow();
            (model.active, self.tab_label(model.tab()))
        };
        let mut label = wide(&label);
        let item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(label.as_mut_ptr()),
            ..Default::default()
        };
        send(
            controls.tabs,
            TCM_SETITEMW,
            active,
            std::ptr::from_ref(&item) as isize,
        );
    }

    pub(super) fn append_tabs(&self, tabs: Vec<Tab>) {
        self.model.borrow_mut().tabs.extend(tabs);
        if let Some(controls) = self.controls.get() {
            self.sync_tabs(controls);
        }
    }

    pub(super) fn tabs_for_merge(&self) -> Vec<Tab> {
        let mut model = self.model.borrow_mut();
        model.save_draft();
        model.tabs.clone()
    }

    fn load_tab(&self, index: usize, start: PathBuf, visit: Visit) -> Result<()> {
        let mut model = self.model.borrow_mut();
        model.cancel();
        model.active = index;
        if model.tab().history.folders.is_empty() {
            model.tabs[index].start = Some(start.clone());
        }
        model.folder = None;
        model.search = Search::default();
        drop(model);
        self.workers.listing.send(ListJob::Clear)?;
        self.deferred_refresh.set(false);
        if let Some(controls) = self.controls.get() {
            send(controls.tabs, TCM_SETCURSEL, index, 0);
            set_text(controls.address, display_path(&start));
            self.suppress_search.set(true);
            set_text(controls.search, "");
            self.suppress_search.set(false);
            reset_list(controls.list, 0);
            self.search_chrome(controls);
            self.navigation_buttons(controls);
            self.layout(self.window.get(), controls);
        }
        self.navigate(start, visit)
    }

    fn tab_path(&self, index: usize) -> Result<PathBuf> {
        let model = self.model.borrow();
        let tab = model.tabs.get(index).context("Select a tab")?;
        tab.path().map(Path::to_owned).map_or_else(home, Ok)
    }

    pub(super) fn select_tab(&self, index: usize) -> Result<()> {
        if self.model.borrow().active != index {
            let start = self.tab_path(index)?;
            self.model.borrow_mut().save_draft();
            self.load_tab(index, start, Visit::Restore)?;
        }
        Ok(())
    }

    pub(super) fn cycle_tab(&self, backward: bool) -> Result<()> {
        let index = {
            let model = self.model.borrow();
            let step = if backward { model.tabs.len() - 1 } else { 1 };
            (model.active + step) % model.tabs.len()
        };
        self.select_tab(index)
    }

    pub(super) fn new_tab(&self) -> Result<()> {
        let active = self.model.borrow().active;
        let start = self.current_path().or_else(|_| self.tab_path(active))?;
        let index = {
            let mut model = self.model.borrow_mut();
            model.save_draft();
            let index = model.tabs.len();
            model.tabs.push(Tab::default());
            index
        };
        if let Some(controls) = self.controls.get() {
            self.sync_tabs(controls);
        }
        self.load_tab(index, start, Visit::New)?;
        self.focus();
        Ok(())
    }

    pub(super) fn close_tab(&self) -> Result<()> {
        let active = self.model.borrow().active;
        self.remove_tab(active)
    }

    fn remove_tab(&self, index: usize) -> Result<()> {
        if self.model.borrow().tabs.len() == 1 {
            return Frame::at(self.frame)
                .context("Window closed")?
                .close_pane(self.window.get());
        }
        let next = {
            let mut model = self.model.borrow_mut();
            model.tabs.remove(index);
            if index == model.active {
                model.active = index.min(model.tabs.len() - 1);
                Some(model.active)
            } else {
                model.active -= usize::from(index < model.active);
                None
            }
        };
        if let Some(controls) = self.controls.get() {
            self.sync_tabs(controls);
        }
        if let Some(index) = next {
            self.load_tab(index, self.tab_path(index)?, Visit::Restore)?;
        }
        Ok(())
    }

    fn reorder_tab(&self, from: usize, to: usize) {
        if from == to {
            return;
        }
        let mut model = self.model.borrow_mut();
        if from >= model.tabs.len() || to >= model.tabs.len() {
            return;
        }
        let tab = model.tabs.remove(from);
        model.tabs.insert(to, tab);
        let active = model.active;
        model.active = if active == from {
            to
        } else {
            let shifted = active - usize::from(from < active);
            shifted + usize::from(to <= shifted)
        };
        drop(model);
        if let Some(controls) = self.controls.get() {
            self.sync_tabs(controls);
        }
    }

    pub(super) fn detach_tab(&self, index: usize, point: Option<POINT>) -> Result<()> {
        let frame = Frame::at(self.frame).context("Window closed")?;
        let tab = {
            let mut model = self.model.borrow_mut();
            ensure!(
                model.tabs.len() > 1 || frame.other(self.window.get()).is_some(),
                "Open a second tab before detaching this one"
            );
            if model.active == index {
                model.save_draft();
            }
            model.tabs.get(index).context("Select a tab")?.clone()
        };
        Frame::open(
            self.session.clone(),
            tab,
            self.tab_path(index)?,
            Visit::Restore,
            point,
            None,
        )?;
        self.remove_tab(index)
    }
}

fn hit_tab(tabs: HWND, point: POINT) -> Option<usize> {
    let mut hit = TCHITTESTINFO {
        pt: point,
        ..Default::default()
    };
    let row = send(tabs, TCM_HITTEST, 0, std::ptr::from_mut(&mut hit) as isize);
    (row >= 0).then_some(row as usize)
}

fn tab_dragged(start: POINT, end: POINT) -> bool {
    (end.x - start.x).abs() >= unsafe { GetSystemMetrics(SM_CXDRAG) }
        || (end.y - start.y).abs() >= unsafe { GetSystemMetrics(SM_CYDRAG) }
}

unsafe extern "system" fn tab_procedure(
    tabs: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    let Some(app) = (unsafe { GetParent(tabs) }).ok().and_then(pane_at) else {
        return unsafe { DefSubclassProc(tabs, message, wparam, lparam) };
    };
    match message {
        WM_ERASEBKGND => {
            app.session
                .theme
                .borrow()
                .erase(HDC(wparam.0 as *mut _), tabs);
            return LRESULT(1);
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(tabs, &mut paint) };
            paint_tabs(&app, tabs, dc);
            let _ = unsafe { EndPaint(tabs, &paint) };
            return LRESULT(0);
        }
        WM_PRINTCLIENT => {
            paint_tabs(&app, tabs, HDC(wparam.0 as *mut _));
            return LRESULT(0);
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let _ = unsafe { InvalidateRect(Some(tabs), None, false) };
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if let Some(index) = hit_tab(tabs, point(lparam)) {
                if let Some(frame) = Frame::at(app.frame) {
                    frame.activate(app.window.get());
                }
                unsafe {
                    let _ = SetFocus(Some(tabs));
                    SetCapture(tabs);
                }
                app.drag.replace(Some(Drag::Tab {
                    index,
                    start: screen_point(tabs, lparam),
                }));
                return LRESULT(0);
            }
        }
        WM_MOUSEMOVE => {
            if let Some(Drag::Tab { start, .. }) = app.drag.borrow().as_ref()
                && tab_dragged(*start, screen_point(tabs, lparam))
            {
                set_cursor(IDC_SIZEALL);
                return LRESULT(0);
            }
        }
        WM_LBUTTONUP => {
            let drag = app.drag.take();
            if let Some(Drag::Tab { index, start }) = drag {
                let _ = unsafe { ReleaseCapture() };
                let end = screen_point(tabs, lparam);
                let mut frame = RECT::default();
                let result = if !tab_dragged(start, end) {
                    app.select_tab(index)
                } else if unsafe { GetWindowRect(app.frame, &mut frame) }.is_ok()
                    && !unsafe { PtInRect(&frame, end) }.as_bool()
                {
                    app.detach_tab(index, Some(end))
                } else {
                    let to = hit_tab(tabs, point(lparam)).or_else(|| {
                        let mut rect = RECT::default();
                        (unsafe { GetClientRect(tabs, &mut rect) }.is_ok()
                            && unsafe { PtInRect(&rect, point(lparam)) }.as_bool())
                        .then(|| app.model.borrow().tabs.len() - 1)
                    });
                    if let Some(to) = to {
                        app.reorder_tab(index, to);
                    }
                    Ok(())
                };
                if let Err(error) = result {
                    app.error(&error);
                }
                return LRESULT(0);
            }
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            if matches!(app.drag.borrow().as_ref(), Some(Drag::Tab { .. })) {
                app.drag.replace(None);
            }
        }
        _ => {}
    }
    unsafe { DefSubclassProc(tabs, message, wparam, lparam) }
}
