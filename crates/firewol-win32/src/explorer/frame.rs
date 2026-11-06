use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Result, ensure};
use firewol_core::explorer::{ItemId, Preset, Settings, data_dir, display_path, home};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::controls::*;
use super::measure::Measurement;
use super::theme::{self, Theme, draw_text, fill};
use super::transfer_view::{self, TransferView};
use super::workers::{FILE_WAKE, FileJob, FileWorker, Threads, finish};
use super::{App, Tab, Visit, editors, form};

const CLASS: PCWSTR = w!("FirewolExplorer");
const HEADER: i32 = 46;
const SIDEBAR: usize = 201;
const SPLIT: usize = 202;
const APPEARANCE: usize = 203;
const MENU: usize = 204;
const PINS: usize = 205;
const MINIMIZE: usize = 206;
const MAXIMIZE: usize = 207;
const CLOSE: usize = 208;
const TRANSFERS: usize = 209;
const ADMIN: usize = 210;
const TRANSFER_TIMER: usize = 2;
const MAX_WINDOWS: usize = 4;

pub(super) struct Clipboard {
    pub items: Vec<ItemId>,
    pub cut: bool,
}

#[derive(Clone, Copy)]
struct Pending {
    pane: HWND,
    consume_cut: bool,
    transfer: bool,
    elevation: bool,
}

pub(super) struct Session {
    pub instance: HINSTANCE,
    pub data: PathBuf,
    pub settings: RefCell<Settings>,
    pub theme: RefCell<Theme>,
    pub clipboard: RefCell<Option<Clipboard>>,
    pub threads: Threads,
    windows: RefCell<Vec<HWND>>,
    files: FileWorker,
    pending: Cell<Option<Pending>>,
    transfer: RefCell<Option<TransferView>>,
    writable: bool,
}

impl Session {
    pub fn update(&self, apply: impl FnOnce(&mut Settings)) -> Result<()> {
        if self.writable {
            self.settings.replace(Settings::update(&self.data, apply)?);
        } else {
            apply(&mut self.settings.borrow_mut());
        }
        Ok(())
    }

    pub fn settings_changed(&self) {
        self.theme
            .replace(Theme::new(self.settings.borrow().appearance));
        for frame in self.frames() {
            frame.sync();
        }
    }

    fn frames(&self) -> Vec<Rc<Frame>> {
        self.windows
            .borrow()
            .iter()
            .filter_map(|window| Frame::at(*window))
            .collect()
    }

    pub fn busy(&self) -> bool {
        self.pending.get().is_some()
    }

    pub fn submit(&self, frame: HWND, pane: HWND, job: FileJob, consume_cut: bool) -> Result<()> {
        ensure!(!self.busy(), "A file operation is already running");
        let transfer = match &job {
            FileJob::Transfer(request, control) => {
                Some(TransferView::new(control.clone(), request.items.len()))
            }
            FileJob::Undo(control) => Some(TransferView::new(control.clone(), 1)),
            _ => None,
        };
        let elevation = matches!(job, FileJob::Elevate { .. });
        self.files.submit(frame, job)?;
        self.pending.set(Some(Pending {
            pane,
            consume_cut,
            transfer: transfer.is_some(),
            elevation,
        }));
        if let Some(transfer) = transfer {
            self.transfer.replace(Some(transfer));
            unsafe { SetTimer(Some(frame), TRANSFER_TIMER, transfer_view::TICK_MS, None) };
            if let Some(frame) = Frame::at(frame)
                && let Some(toolbar) = frame.toolbar.get()
            {
                set_text(toolbar.transfers, "Transferring…");
            }
        }
        Ok(())
    }

    fn file_finished(&self, frame: &Frame) {
        let Ok(result) = self.files.results.try_recv() else {
            return;
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.transfer {
            let _ = unsafe { KillTimer(Some(frame.window.get()), TRANSFER_TIMER) };
            if let Some(view) = self.transfer.borrow_mut().as_mut() {
                view.finish(match &result {
                    Ok(result) => result.message.clone(),
                    Err(error) => format!("{error:#}"),
                });
            }
            if let Some(toolbar) = frame.toolbar.get() {
                set_text(toolbar.transfers, "Transfers…");
            }
        }
        if pending.consume_cut
            && let Ok(result) = &result
        {
            let mut clipboard = self.clipboard.borrow_mut();
            if let Some(cut) = clipboard.as_mut() {
                cut.items.retain(|item| !result.transferred.contains(item));
                if cut.items.is_empty() {
                    *clipboard = None;
                }
            }
        }
        let origin = frame
            .panes()
            .into_iter()
            .find(|pane| pane.window.get() == pending.pane)
            .or_else(|| frame.active());
        let refresh = !pending.elevation && (result.is_ok() || pending.transfer);
        let message = match result {
            Ok(result) => result.message,
            Err(error) => format!("{error:#}"),
        };
        if refresh {
            for frame in self.frames() {
                for pane in frame.panes() {
                    let notice = origin
                        .as_ref()
                        .filter(|origin| Rc::ptr_eq(origin, &pane))
                        .map(|_| message.clone());
                    pane.refresh_after_operation(notice);
                }
            }
        } else if let Some(pane) = origin {
            pane.status(message);
        }
    }
}

#[derive(Clone, Copy)]
struct Toolbar {
    sidebar: HWND,
    split: HWND,
    appearance: HWND,
    menu: HWND,
    transfers: HWND,
    admin: HWND,
    pins: HWND,
    caption: [HWND; 3],
}

pub(super) struct Frame {
    window: Cell<HWND>,
    pub session: Rc<Session>,
    toolbar: Cell<Option<Toolbar>>,
    panes: RefCell<[Option<Rc<App>>; 2]>,
    active: Cell<HWND>,
    glass: Cell<bool>,
}

impl Frame {
    // Call only for a Firewol frame on the UI thread.
    pub fn at(window: HWND) -> Option<Rc<Self>> {
        let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Self;
        if data.is_null() {
            return None;
        }
        unsafe {
            Rc::increment_strong_count(data);
            Some(Rc::from_raw(data))
        }
    }

    pub fn open(
        session: Rc<Session>,
        tab: Tab,
        start: PathBuf,
        visit: Visit,
        position: Option<POINT>,
        measurement: Option<Measurement>,
    ) -> Result<()> {
        ensure!(
            session.windows.borrow().len() < MAX_WINDOWS,
            "Close a window before detaching another tab"
        );
        let frame = Rc::new(Self {
            window: Cell::new(HWND::default()),
            session,
            toolbar: Cell::new(None),
            panes: RefCell::new([None, None]),
            active: Cell::new(HWND::default()),
            glass: Cell::default(),
        });
        let (x, y) = position.map_or((CW_USEDEFAULT, CW_USEDEFAULT), |point| {
            let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
            let mut bounds = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(monitor, &mut bounds) }.as_bool() {
                let work = bounds.rcWork;
                (
                    (point.x - 100).clamp(work.left, (work.right - 780).max(work.left)),
                    (point.y - 20).clamp(work.top, (work.bottom - 420).max(work.top)),
                )
            } else {
                (CW_USEDEFAULT, CW_USEDEFAULT)
            }
        });
        let instance = frame.session.instance;
        let guard = super::Window(unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_CONTROLPARENT,
                CLASS,
                w!("Firewol"),
                WS_POPUP
                    | WS_THICKFRAME
                    | WS_MINIMIZEBOX
                    | WS_MAXIMIZEBOX
                    | WS_SYSMENU
                    | WS_CLIPCHILDREN,
                x,
                y,
                1000,
                670,
                None,
                None,
                Some(instance),
                Some(std::ptr::from_ref(frame.as_ref()).cast()),
            )?
        });
        let window = guard.0;
        frame.window.set(window);
        let button = |id, label| painted_button(window, instance, id, label);
        frame.toolbar.set(Some(Toolbar {
            sidebar: button(SIDEBAR, w!("Sidebar"))?,
            split: button(SPLIT, w!("Split view"))?,
            appearance: button(APPEARANCE, w!("Appearance…"))?,
            menu: button(MENU, w!("Edit menu…"))?,
            transfers: button(TRANSFERS, w!("Transfers…"))?,
            admin: button(ADMIN, w!("Admin…"))?,
            pins: child(
                window,
                instance,
                PINS,
                w!("LISTBOX"),
                w!("Pinned folders"),
                WS_TABSTOP
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
                WINDOW_EX_STYLE::default(),
            )?,
            caption: [
                button(MINIMIZE, w!("Minimize"))?,
                button(MAXIMIZE, w!("Maximize or restore"))?,
                button(CLOSE, w!("Close window"))?,
            ],
        }));
        let pane = App::open(
            window,
            frame.session.clone(),
            tab,
            start,
            visit,
            measurement,
        )?;
        frame.active.set(pane.window.get());
        frame.panes.borrow_mut()[0] = Some(pane.clone());
        frame.session.windows.borrow_mut().push(window);
        frame.sync();
        show(window, true);
        let _ = unsafe { UpdateWindow(window) };
        pane.focus();
        if let Some(probe) = &pane.measurement {
            probe.mark("window shown");
        }
        std::mem::forget(guard);
        Ok(())
    }

    fn panes(&self) -> Vec<Rc<App>> {
        self.panes.borrow().iter().flatten().cloned().collect()
    }

    pub fn active(&self) -> Option<Rc<App>> {
        let panes = self.panes();
        panes
            .iter()
            .find(|pane| pane.window.get() == self.active.get())
            .cloned()
            .or_else(|| panes.into_iter().next())
    }

    pub fn other(&self, window: HWND) -> Option<Rc<App>> {
        self.panes()
            .into_iter()
            .find(|pane| pane.window.get() != window)
    }

    pub fn activate(&self, window: HWND) {
        if self.active.replace(window) != window {
            let _ = unsafe { InvalidateRect(Some(self.window.get()), None, true) };
        }
    }

    pub fn close_pane(&self, window: HWND) -> Result<()> {
        if self.other(window).is_none() {
            unsafe { PostMessageW(Some(self.window.get()), WM_CLOSE, WPARAM(0), LPARAM(0))? };
            return Ok(());
        }
        let closed = {
            let mut panes = self.panes.borrow_mut();
            let index = usize::from(
                panes[1]
                    .as_ref()
                    .is_some_and(|pane| pane.window.get() == window),
            );
            let closed = panes[index].take();
            if index == 0 {
                panes[0] = panes[1].take();
            }
            closed
        };
        if let Some(pane) = closed {
            unsafe { DestroyWindow(pane.window.get())? };
        }
        if let Some(pane) = self.active() {
            self.activate(pane.window.get());
            pane.focus();
        }
        self.layout();
        Ok(())
    }

    fn toggle_split(&self) -> Result<()> {
        let panes = self.panes();
        let Some(left) = panes.first() else {
            return Ok(());
        };
        if let Some(right) = panes.get(1) {
            left.append_tabs(right.tabs_for_merge());
            return self.close_pane(right.window.get());
        }
        let start = left.current_path().or_else(|_| home())?;
        let right = App::open(
            self.window.get(),
            self.session.clone(),
            Tab::default(),
            start,
            Visit::New,
            None,
        )?;
        self.panes.borrow_mut()[1] = Some(right.clone());
        self.layout();
        self.activate(right.window.get());
        right.focus();
        Ok(())
    }

    fn sidebar_visible(&self) -> bool {
        let settings = self.session.settings.borrow();
        settings.sidebar.unwrap_or(!settings.pins.is_empty())
    }

    fn layout(&self) {
        let Some(toolbar) = self.toolbar.get() else {
            return;
        };
        let window = self.window.get();
        let mut rect = RECT::default();
        if unsafe { GetClientRect(window, &mut rect) }.is_err() {
            return;
        }
        let (width, height) = (rect.right, rect.bottom);
        let caption_left = self.session.settings.borrow().appearance.caption_left;
        let caption_x = if caption_left { 8 } else { width - 116 };
        for (i, button) in toolbar.caption.iter().enumerate() {
            let i = if caption_left { 2 - i } else { i };
            place(*button, caption_x + i as i32 * 36, 8, 34, 29);
        }
        let actions_x = width - if caption_left { 522 } else { 630 };
        place(toolbar.sidebar, actions_x, 7, 68, 29);
        place(toolbar.split, actions_x + 73, 7, 75, 29);
        place(toolbar.appearance, actions_x + 153, 7, 95, 29);
        place(toolbar.menu, actions_x + 253, 7, 79, 29);
        place(toolbar.transfers, actions_x + 337, 7, 101, 29);
        place(toolbar.admin, actions_x + 443, 7, 68, 29);
        let sidebar = self.sidebar_visible();
        let sidebar_width = if sidebar { 164 } else { 0 };
        place(
            toolbar.pins,
            7,
            HEADER + 30,
            sidebar_width - 12,
            height - HEADER - 38,
        );
        show(toolbar.pins, sidebar);
        let panes = self.panes();
        let available = width - sidebar_width;
        let split = panes.len() == 2;
        set_text(
            toolbar.split,
            if split { "Single pane" } else { "Split view" },
        );
        let left_width = if split {
            (available - 7) / 2
        } else {
            available
        };
        if let Some(left) = panes.first() {
            place(
                left.window.get(),
                sidebar_width,
                HEADER + 3,
                left_width,
                height - HEADER - 3,
            );
        }
        if let Some(right) = panes.get(1) {
            place(
                right.window.get(),
                sidebar_width + left_width + 7,
                HEADER + 3,
                available - left_width - 7,
                height - HEADER - 3,
            );
        }
        let _ = unsafe { InvalidateRect(Some(window), None, true) };
    }

    fn sync(&self) {
        self.backdrop();
        if let Some(toolbar) = self.toolbar.get() {
            let selected = send(toolbar.pins, LB_GETCURSEL, 0, 0);
            send(toolbar.pins, LB_RESETCONTENT, 0, 0);
            let mut width = 0;
            for path in &self.session.settings.borrow().pins {
                let text = wide(&display_path(path));
                width = width.max(text.len() * 7);
                send(toolbar.pins, LB_ADDSTRING, 0, text.as_ptr() as isize);
            }
            send(toolbar.pins, LB_SETHORIZONTALEXTENT, width, 0);
            send(toolbar.pins, LB_SETCURSEL, selected as usize, 0);
        }
        for pane in self.panes() {
            pane.sync();
        }
        self.layout();
        unsafe {
            let _ = RedrawWindow(
                Some(self.window.get()),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }

    fn selected_pin(&self) -> Option<PathBuf> {
        let toolbar = self.toolbar.get()?;
        let selected = send(toolbar.pins, LB_GETCURSEL, 0, 0);
        self.session
            .settings
            .borrow()
            .pins
            .get(selected as usize)
            .cloned()
    }

    fn open_pin(&self) -> Result<()> {
        if let (Some(path), Some(pane)) = (self.selected_pin(), self.active()) {
            pane.navigate(path, Visit::New)?;
            pane.focus();
        }
        Ok(())
    }

    fn command(&self, id: usize, code: u32) -> Result<()> {
        let window = self.window.get();
        match id {
            SIDEBAR => {
                let visible = !self.sidebar_visible();
                self.session
                    .update(|settings| settings.sidebar = Some(visible))?;
                self.session.settings_changed();
            }
            SPLIT => self.toggle_split()?,
            APPEARANCE => {
                let previous = self.session.settings.borrow().appearance;
                if let Some(appearance) = editors::appearance(window, previous)? {
                    self.session
                        .update(|settings| settings.appearance = appearance)?;
                    self.session.settings_changed();
                }
            }
            MENU => {
                let previous = self.session.settings.borrow().menu.clone();
                if let Some(menu) = editors::menu(window, previous)? {
                    self.session.update(|settings| settings.menu = menu)?;
                    self.session.settings_changed();
                }
            }
            TRANSFERS => transfer_view::show(window, &self.session.transfer)?,
            ADMIN => {
                let previous = self.session.settings.borrow().elevation.clone();
                if let Some(policy) = editors::elevation(window, &previous)? {
                    self.session
                        .update(|settings| settings.elevation = policy)?;
                }
            }
            PINS if code == LBN_DBLCLK => self.open_pin()?,
            MINIMIZE => show_command(window, SW_MINIMIZE),
            MAXIMIZE => toggle_maximize(window),
            CLOSE => unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0))? },
            _ => {}
        }
        Ok(())
    }

    fn backdrop(&self) {
        let appearance = self.session.settings.borrow().appearance;
        let want_glass =
            appearance.preset == Preset::Glassmorphism && appearance.background.is_none();
        let window = self.window.get();
        let set = |attribute, value: i32| unsafe {
            DwmSetWindowAttribute(
                window,
                attribute,
                std::ptr::from_ref(&value).cast(),
                size_of::<i32>() as u32,
            )
        };
        if !want_glass && !self.glass.get() {
            return;
        }
        let margins = MARGINS {
            cyTopHeight: HEADER,
            ..Default::default()
        };
        let use_glass = want_glass
            && set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1).is_ok()
            && set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW.0).is_ok()
            && unsafe { DwmExtendFrameIntoClientArea(window, &margins) }.is_ok();
        if !use_glass {
            let _ = set(DWMWA_USE_IMMERSIVE_DARK_MODE, 0);
            let _ = set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE.0);
            let _ = unsafe { DwmExtendFrameIntoClientArea(window, &MARGINS::default()) };
        }
        if self.glass.replace(use_glass) != use_glass {
            // Extending a frame with WS_SYSMENU makes DWM draw a second set of caption buttons.
            let native_caption = (WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0 as isize;
            unsafe {
                let styles = GetWindowLongPtrW(window, GWL_STYLE);
                let styles = if use_glass {
                    styles & !native_caption
                } else {
                    styles | native_caption
                };
                SetWindowLongPtrW(window, GWL_STYLE, styles);
                let _ = SetWindowPos(
                    window,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
        }
    }

    fn paint(&self, dc: HDC) {
        let window = self.window.get();
        let theme = self.session.theme.borrow();
        let mut rect = RECT::default();
        if unsafe { GetClientRect(window, &mut rect) }.is_err() {
            return;
        }
        let header = RECT {
            bottom: HEADER - 1,
            ..rect
        };
        if !self.glass.get() {
            theme.header(dc, header);
        }
        fill(
            dc,
            RECT {
                top: HEADER - 1,
                bottom: HEADER,
                ..rect
            },
            theme.border,
        );
        let left = if theme.appearance.caption_left {
            124
        } else {
            13
        };
        draw_text(
            dc,
            RECT {
                left,
                right: left + 80,
                ..header
            },
            OsStr::new("Firewol"),
            theme.text,
            DT_LEFT,
        );
        if self.sidebar_visible() {
            fill(
                dc,
                RECT {
                    left: 163,
                    right: 164,
                    top: HEADER,
                    ..rect
                },
                theme.border,
            );
            draw_text(
                dc,
                RECT {
                    left: 12,
                    top: HEADER + 5,
                    right: 160,
                    bottom: HEADER + 27,
                },
                OsStr::new("Pinned folders"),
                theme.muted,
                DT_LEFT,
            );
        }
        if let Some(active) = self.active() {
            let mut pane = RECT::default();
            if unsafe { GetWindowRect(active.window.get(), &mut pane) }.is_ok() {
                let mut position = POINT {
                    x: pane.left,
                    y: pane.top,
                };
                let _ = unsafe { ScreenToClient(window, &mut position) };
                fill(
                    dc,
                    RECT {
                        left: position.x + 8,
                        top: HEADER + 1,
                        right: position.x + pane.right - pane.left - 8,
                        bottom: HEADER + 3,
                    },
                    theme.accent,
                );
            }
        }
    }

    fn keyboard(&self, message: &MSG) -> Result<bool> {
        if matches!(
            message.message,
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN
        ) && let Some(pane) = self.panes().into_iter().find(|pane| {
            pane.window.get() == message.hwnd
                || unsafe { IsChild(pane.window.get(), message.hwnd) }.as_bool()
        }) {
            self.activate(pane.window.get());
        }
        if message.message != WM_KEYDOWN && message.message != WM_SYSKEYDOWN {
            return Ok(false);
        }
        if message.message == WM_SYSKEYDOWN && message.wParam.0 == VK_F4.0 as usize {
            self.command(CLOSE, 0)?;
            return Ok(true);
        }
        if self
            .toolbar
            .get()
            .is_some_and(|toolbar| message.hwnd == toolbar.pins)
        {
            match VIRTUAL_KEY(message.wParam.0 as u16) {
                VK_RETURN => {
                    self.open_pin()?;
                    return Ok(true);
                }
                VK_DELETE => {
                    if let Some(path) = self.selected_pin() {
                        self.session
                            .update(|settings| settings.pins.retain(|pin| pin != &path))?;
                        self.session.settings_changed();
                    }
                    return Ok(true);
                }
                _ => {}
            }
        }
        self.active()
            .map_or(Ok(false), |pane| pane.keyboard(message))
    }
}

fn show_command(window: HWND, command: SHOW_WINDOW_CMD) {
    let _ = unsafe { ShowWindow(window, command) };
}

fn toggle_maximize(window: HWND) {
    let command = if unsafe { IsZoomed(window) }.as_bool() {
        SW_RESTORE
    } else {
        SW_MAXIMIZE
    };
    show_command(window, command);
}

fn resize_frame() -> (i32, i32) {
    unsafe {
        let padding = GetSystemMetrics(SM_CXPADDEDBORDER);
        (
            GetSystemMetrics(SM_CXSIZEFRAME) + padding,
            GetSystemMetrics(SM_CYSIZEFRAME) + padding,
        )
    }
}

fn frame_hit(window: HWND, mut point: POINT) -> LRESULT {
    let mut bounds = RECT::default();
    if !unsafe { IsZoomed(window) }.as_bool()
        && unsafe { GetWindowRect(window, &mut bounds) }.is_ok()
    {
        let (x, y) = resize_frame();
        let left = point.x < bounds.left + x;
        let right = point.x >= bounds.right - x;
        let top = point.y < bounds.top + y;
        let bottom = point.y >= bounds.bottom - y;
        let border = match (left, right, top, bottom) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, _, _, _) => HTLEFT,
            (_, true, _, _) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (_, _, _, true) => HTBOTTOM,
            _ => HTCLIENT,
        };
        if border != HTCLIENT {
            return LRESULT(border as isize);
        }
    }
    let _ = unsafe { ScreenToClient(window, &mut point) };
    if (0..HEADER).contains(&point.y)
        && unsafe { ChildWindowFromPointEx(window, point, CWP_SKIPINVISIBLE) } == window
    {
        LRESULT(HTCAPTION as isize)
    } else {
        LRESULT(HTCLIENT as isize)
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
        let frame = create.lpCreateParams.cast::<Frame>();
        unsafe {
            Rc::increment_strong_count(frame);
            SetWindowLongPtrW(window, GWLP_USERDATA, frame as isize);
        }
    }
    let Some(frame) = Frame::at(window) else {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };
    match message {
        WM_NCDESTROY => {
            let data = unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) } as *const Frame;
            let mut windows = frame.session.windows.borrow_mut();
            if let Some(index) = windows.iter().position(|item| *item == window) {
                windows.remove(index);
                if windows.is_empty() {
                    unsafe { PostQuitMessage(0) };
                }
            }
            drop(unsafe { Rc::from_raw(data) });
        }
        WM_GETMINMAXINFO => {
            let limits = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            limits.ptMinTrackSize.x = 780;
            limits.ptMinTrackSize.y = 420;
            return LRESULT(0);
        }
        WM_NCCALCSIZE => {
            let bounds = if wparam.0 == 0 {
                unsafe { &mut *(lparam.0 as *mut RECT) }
            } else {
                &mut unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) }.rgrc[0]
            };
            if unsafe { IsZoomed(window) }.as_bool() {
                let (x, y) = resize_frame();
                bounds.left += x;
                bounds.right -= x;
                bounds.top += y;
                bounds.bottom -= y;
            }
            return LRESULT(0);
        }
        WM_NCHITTEST => return frame_hit(window, super::point(lparam)),
        WM_NCLBUTTONDBLCLK if wparam.0 == HTCAPTION as usize => {
            toggle_maximize(window);
            return LRESULT(0);
        }
        WM_NCACTIVATE => return unsafe { DefWindowProcW(window, message, wparam, LPARAM(-1)) },
        WM_CLOSE if frame.session.busy() => {
            if let Some(pane) = frame.active() {
                pane.status("Wait for the current file operation to finish");
            }
            return LRESULT(0);
        }
        WM_SIZE => frame.layout(),
        WM_DWMCOMPOSITIONCHANGED => {
            frame.backdrop();
            let _ = unsafe { InvalidateRect(Some(window), None, true) };
        }
        WM_ERASEBKGND => {
            let dc = HDC(wparam.0 as *mut _);
            if frame.glass.get() {
                let mut rect = RECT::default();
                if unsafe { GetClientRect(window, &mut rect) }.is_ok() {
                    frame.session.theme.borrow().erase(dc, window);
                    rect.bottom = HEADER;
                    unsafe { FillRect(dc, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0)) };
                }
            } else {
                frame.session.theme.borrow().erase(dc, window);
            }
            return LRESULT(1);
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(window, &mut paint) };
            frame.paint(dc);
            let _ = unsafe { EndPaint(window, &paint) };
            return LRESULT(0);
        }
        WM_CTLCOLORLISTBOX => {
            return frame
                .session
                .theme
                .borrow()
                .control_color(HDC(wparam.0 as *mut _), true);
        }
        WM_DRAWITEM if lparam.0 != 0 => {
            let info = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            if info.CtlType == ODT_BUTTON {
                let glyph = match info.CtlID as usize {
                    MINIMIZE => Some("─"),
                    MAXIMIZE if unsafe { IsZoomed(window) }.as_bool() => Some("❐"),
                    MAXIMIZE => Some("□"),
                    CLOSE => Some("×"),
                    _ => None,
                };
                let selected = match info.CtlID as usize {
                    SIDEBAR => frame.sidebar_visible(),
                    SPLIT => frame.panes().len() == 2,
                    _ => false,
                };
                let theme = frame.session.theme.borrow();
                let base = if frame.glass.get() {
                    theme::rgb(0)
                } else {
                    theme.background
                };
                theme.button(info, glyph, selected, glyph.is_some(), base);
                return LRESULT(1);
            }
        }
        WM_COMMAND => {
            if let Err(error) = frame.command(wparam.0 & 0xffff, (wparam.0 >> 16) as u32)
                && let Some(pane) = frame.active()
            {
                pane.error(&error);
            }
            return LRESULT(0);
        }
        FILE_WAKE => {
            frame.session.file_finished(&frame);
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TRANSFER_TIMER => {
            if let Some(view) = frame.session.transfer.borrow_mut().as_mut() {
                view.sample();
            }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

pub(super) fn run(path: Option<PathBuf>) -> Result<()> {
    let measurement = Measurement::requested();
    let data = data_dir()?;
    let (settings, settings_error) = match Settings::load(&data) {
        Ok(settings) => (settings, None),
        Err(error) => (Settings::default(), Some(error)),
    };
    let (start, visit) = match path {
        Some(path) => (path, Visit::New),
        None => (
            settings.last_folder.clone().unwrap_or(home()?),
            Visit::Startup,
        ),
    };
    if let Some(probe) = &measurement {
        probe.mark("settings loaded");
    }
    let common = INITCOMMONCONTROLSEX {
        dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES,
    };
    let instance = unsafe {
        ensure!(
            InitCommonControlsEx(&common).as_bool(),
            "Initialize Windows controls"
        );
        HINSTANCE(GetModuleHandleW(None)?.0)
    };
    form::register(instance)?;
    super::register(instance)?;
    let class = WNDCLASSW {
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        lpfnWndProc: Some(procedure),
        lpszClassName: CLASS,
        ..Default::default()
    };
    ensure!(
        unsafe { RegisterClassW(&class) } != 0,
        "Register Firewol window class"
    );
    let threads = Threads::default();
    let session = Rc::new(Session {
        instance,
        data,
        theme: RefCell::new(Theme::new(settings.appearance)),
        settings: RefCell::new(settings),
        writable: settings_error.is_none(),
        windows: RefCell::default(),
        clipboard: RefCell::default(),
        pending: Cell::default(),
        transfer: RefCell::default(),
        files: FileWorker::start(&threads),
        threads: threads.clone(),
    });
    Frame::open(
        session.clone(),
        Tab::default(),
        start,
        visit,
        None,
        measurement,
    )?;
    if let Some(error) = settings_error
        && let Some(pane) = session.frames().first().and_then(|frame| frame.active())
    {
        pane.error(&error.context("Settings are available for this session"));
    }
    let mut message = MSG::default();
    loop {
        let next = unsafe { GetMessageW(&mut message, None, 0, 0).0 };
        ensure!(next >= 0, "Read the Win32 message queue");
        if next == 0 {
            break;
        }
        let window = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
        let known = session.windows.borrow().contains(&window);
        if known && let Some(frame) = Frame::at(window) {
            match frame.keyboard(&message) {
                Ok(true) => continue,
                Err(error) => {
                    if let Some(pane) = frame.active() {
                        pane.error(&error);
                    }
                    continue;
                }
                Ok(false) => {}
            }
            if unsafe { IsDialogMessageW(window, &message) }.as_bool() {
                continue;
            }
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(session);
    finish(threads);
    Ok(())
}
