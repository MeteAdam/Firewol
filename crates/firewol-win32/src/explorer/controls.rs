use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use anyhow::{Result, ensure};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHGSI_FLAGS, SHGSI_ICON, SHGSI_SMALLICON, SHGetStockIconInfo, SHSTOCKICONINFO,
    SIID_DOCNOASSOC, SUBCLASSPROC, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, w};

use super::theme::{draw_text, fill, mix};

pub(super) const BACK: usize = 101;
pub(super) const FORWARD: usize = 102;
pub(super) const UP: usize = 103;
pub(super) const ADDRESS: usize = 104;
pub(super) const GO: usize = 105;
pub(super) const PIN: usize = 106;
pub(super) const SEARCH: usize = 107;
pub(super) const REGEX: usize = 108;
pub(super) const FILTERS: usize = 109;
pub(super) const VOLUME: usize = 110;
pub(super) const TABS: usize = 111;
pub(super) const LIST: usize = 112;
pub(super) const PREVIOUS: usize = 113;
pub(super) const NEXT: usize = 114;
pub(super) const STATUS: usize = 115;
pub(super) const NEW_TAB: usize = 116;
pub(super) const CLOSE_TAB: usize = 117;
pub(super) const COLUMN_NAMES: [&str; 4] = ["Name", "Type", "Location", "Tags"];

#[derive(Clone, Copy)]
pub(super) struct Controls {
    pub back: HWND,
    pub forward: HWND,
    pub up: HWND,
    pub address: HWND,
    pub go: HWND,
    pub pin: HWND,
    pub search: HWND,
    pub regex: HWND,
    pub filters: HWND,
    pub volume: HWND,
    pub tabs: HWND,
    pub new_tab: HWND,
    pub close_tab: HWND,
    pub list: HWND,
    pub previous: HWND,
    pub next: HWND,
    pub status: HWND,
}

impl Controls {
    pub fn create(window: HWND, instance: HINSTANCE) -> Result<Self> {
        let add_button = |id, title| painted_button(window, instance, id, title);
        let controls = Self {
            back: add_button(BACK, w!("Back"))?,
            forward: add_button(FORWARD, w!("Forward"))?,
            up: add_button(UP, w!("Up"))?,
            address: edit(window, instance, ADDRESS, 32767)?,
            go: add_button(GO, w!("Go"))?,
            pin: add_button(PIN, w!("Pin"))?,
            search: edit(window, instance, SEARCH, 1024)?,
            regex: add_button(REGEX, w!("Regex"))?,
            filters: add_button(FILTERS, w!("Filters"))?,
            volume: add_button(VOLUME, w!("NTFS search…"))?,
            tabs: child(
                window,
                instance,
                TABS,
                WC_TABCONTROLW,
                w!("Folder tabs"),
                WS_TABSTOP
                    | WS_CLIPCHILDREN
                    | WINDOW_STYLE(TCS_BUTTONS | TCS_FLATBUTTONS | TCS_FIXEDWIDTH),
                WINDOW_EX_STYLE::default(),
            )?,
            new_tab: add_button(NEW_TAB, w!("New tab"))?,
            close_tab: add_button(CLOSE_TAB, w!("Close tab"))?,
            list: child(
                window,
                instance,
                LIST,
                WC_LISTVIEWW,
                w!(""),
                WS_TABSTOP
                    | WINDOW_STYLE(
                        LVS_REPORT | LVS_OWNERDATA | LVS_NOSORTHEADER | LVS_SHOWSELALWAYS,
                    ),
                WS_EX_CLIENTEDGE,
            )?,
            previous: add_button(PREVIOUS, w!("Previous"))?,
            next: add_button(NEXT, w!("Next"))?,
            status: child(
                window,
                instance,
                STATUS,
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE::default(),
                WINDOW_EX_STYLE::default(),
            )?,
        };
        send(controls.tabs, TCM_SETITEMSIZE, 0, 24 << 16 | 116);
        cue(controls.address, w!("Folder path"));
        cue(controls.search, w!("Search this folder and its subfolders"));
        send(
            controls.list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
        );
        for (index, (title, width)) in COLUMN_NAMES.into_iter().zip([440, 70, 0, 0]).enumerate() {
            let mut text = wide(OsStr::new(title));
            let column = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH,
                cx: width,
                pszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            ensure!(
                send(
                    controls.list,
                    LVM_INSERTCOLUMNW,
                    index,
                    std::ptr::from_ref(&column) as isize
                ) != -1,
                "Insert virtual list column"
            );
        }
        set_stock_icons(controls.list);
        Ok(controls)
    }

    pub fn install_theme(self, pane: HWND) -> Result<()> {
        let header = HWND(send(self.list, LVM_GETHEADER, 0, 0) as *mut _);
        let controls: [(HWND, SUBCLASSPROC); 3] = [
            (header, Some(header_procedure)),
            (self.address, Some(edit_procedure)),
            (self.search, Some(edit_procedure)),
        ];
        for (window, procedure) in controls {
            ensure!(
                unsafe { SetWindowSubclass(window, procedure, 0, pane.0 as usize) }.as_bool(),
                "Theme the native explorer controls"
            );
        }
        Ok(())
    }

    pub fn layout(self, window: HWND, searching: bool, tags: bool) {
        let mut rect = RECT::default();
        if unsafe { GetClientRect(window, &mut rect) }.is_err() {
            return;
        }
        let right = (rect.right - 8).max(270);
        let bottom = (rect.bottom - 6).max(146);
        place(self.tabs, 8, 4, right - 72, 29);
        place(self.new_tab, right - 60, 4, 28, 27);
        place(self.close_tab, right - 28, 4, 28, 27);
        place(self.back, 8, 39, 26, 27);
        place(self.forward, 37, 39, 26, 27);
        place(self.up, 66, 39, 26, 27);
        place(self.address, 97, 39, right - 183, 27);
        place(self.go, right - 82, 39, 31, 27);
        place(self.pin, right - 47, 39, 47, 27);
        place(self.search, 8, 71, right - 210, 27);
        place(self.regex, right - 198, 71, 48, 27);
        place(self.filters, right - 146, 71, 52, 27);
        place(self.volume, right - 90, 71, 90, 27);
        place(self.list, 8, 104, right - 8, bottom - 136);
        place(
            self.status,
            8,
            bottom - 22,
            right - if searching { 135 } else { 8 },
            21,
        );
        place(self.previous, right - 126, bottom - 27, 65, 25);
        place(self.next, right - 57, bottom - 27, 57, 25);
        show(self.previous, searching);
        show(self.next, searching);
        let tag_width = if tags {
            if right > 520 { 118 } else { 72 }
        } else {
            0
        };
        let width = (right - 8 - 62 - 21 - tag_width).max(80);
        let location = if searching { width * 40 / 100 } else { 0 };
        send(
            self.list,
            LVM_SETCOLUMNWIDTH,
            0,
            (width - location) as isize,
        );
        send(self.list, LVM_SETCOLUMNWIDTH, 1, 62);
        send(self.list, LVM_SETCOLUMNWIDTH, 2, location as isize);
        send(self.list, LVM_SETCOLUMNWIDTH, 3, tag_width as isize);
    }
}

unsafe extern "system" fn header_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    pane: usize,
) -> LRESULT {
    if matches!(message, WM_PAINT | WM_PRINTCLIENT)
        && let Some(app) = super::pane_at(HWND(pane as *mut _))
    {
        let mut paint = PAINTSTRUCT::default();
        let dc = if message == WM_PAINT {
            unsafe { BeginPaint(window, &mut paint) }
        } else {
            HDC(wparam.0 as *mut _)
        };
        let theme = app.session.theme.borrow();
        let mut client = RECT::default();
        if unsafe { GetClientRect(window, &mut client) }.is_ok() {
            fill(dc, client, mix(theme.surface, theme.background, 40));
            fill(
                dc,
                RECT {
                    top: client.bottom - 1,
                    ..client
                },
                theme.border,
            );
            for (index, name) in COLUMN_NAMES.iter().enumerate() {
                let mut cell = RECT::default();
                if send(
                    window,
                    HDM_GETITEMRECT,
                    index,
                    std::ptr::from_mut(&mut cell) as isize,
                ) != 0
                    && cell.right - cell.left > 10
                {
                    fill(
                        dc,
                        RECT {
                            left: cell.right - 1,
                            ..cell
                        },
                        theme.border,
                    );
                    cell.left += 8;
                    cell.right -= 4;
                    draw_text(dc, cell, OsStr::new(name), theme.text, DT_LEFT);
                }
            }
        }
        if message == WM_PAINT {
            let _ = unsafe { EndPaint(window, &paint) };
        }
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

unsafe extern "system" fn edit_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    pane: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
    if matches!(message, WM_PAINT | WM_PRINTCLIENT)
        && unsafe { GetFocus() } != window
        && unsafe { GetWindowTextLengthW(window) } == 0
        && let Some(app) = super::pane_at(HWND(pane as *mut _))
    {
        let mut text = [0u16; 96];
        if send(
            window,
            EM_GETCUEBANNER,
            text.as_mut_ptr() as usize,
            text.len() as isize,
        ) != 0
        {
            let len = text
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(text.len());
            let theme = app.session.theme.borrow();
            let dc = if message == WM_PAINT {
                unsafe { GetDC(Some(window)) }
            } else {
                HDC(wparam.0 as *mut _)
            };
            let mut rect = RECT::default();
            if unsafe { GetClientRect(window, &mut rect) }.is_ok() {
                fill(dc, rect, theme.surface);
                rect.left += 3;
                draw_text(
                    dc,
                    rect,
                    &OsString::from_wide(&text[..len]),
                    theme.muted,
                    DT_LEFT,
                );
            }
            if message == WM_PAINT {
                unsafe { ReleaseDC(Some(window), dc) };
            }
        }
    }
    result
}

fn set_stock_icons(list: HWND) {
    let images = unsafe { ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 2, 0) };
    if images.0 == 0 {
        return;
    }
    let mut icon = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    if unsafe {
        SHGetStockIconInfo(
            SIID_DOCNOASSOC,
            SHGSI_FLAGS(SHGSI_ICON.0 | SHGSI_SMALLICON.0),
            &mut icon,
        )
    }
    .is_err()
    {
        let _ = unsafe { ImageList_Destroy(Some(images)) };
        return;
    }
    let first = unsafe { ImageList_ReplaceIcon(images, -1, icon.hIcon) };
    let _ = unsafe { DestroyIcon(icon.hIcon) };
    // The masked bitmap reserves the icon cell without painting a stock folder behind it.
    let mask = [0xffffu16; 16];
    let bitmap = unsafe { CreateBitmap(16, 16, 1, 1, Some(mask.as_ptr().cast())) };
    let second = if bitmap.is_invalid() {
        -1
    } else {
        let second = unsafe { ImageList_Add(images, bitmap, Some(bitmap)) };
        let _ = unsafe { DeleteObject(bitmap.into()) };
        second
    };
    if first != 0 || second != 1 {
        let _ = unsafe { ImageList_Destroy(Some(images)) };
        return;
    }
    // The ListView owns the image list because LVS_SHAREIMAGELISTS is disabled.
    send(list, LVM_SETIMAGELIST, LVSIL_SMALL as usize, images.0);
}

pub(super) fn child(
    window: HWND,
    instance: HINSTANCE,
    id: usize,
    class: PCWSTR,
    title: PCWSTR,
    style: WINDOW_STYLE,
    frame: WINDOW_EX_STYLE,
) -> Result<HWND> {
    let control = unsafe {
        CreateWindowExW(
            frame,
            class,
            title,
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            1,
            1,
            Some(window),
            Some(HMENU(id as *mut _)),
            Some(instance),
            None,
        )?
    };
    send(
        control,
        WM_SETFONT,
        unsafe { GetStockObject(DEFAULT_GUI_FONT) }.0 as usize,
        1,
    );
    Ok(control)
}

pub(super) fn button(window: HWND, instance: HINSTANCE, id: usize, title: PCWSTR) -> Result<HWND> {
    child(
        window,
        instance,
        id,
        w!("BUTTON"),
        title,
        WS_TABSTOP,
        WINDOW_EX_STYLE::default(),
    )
}

pub(super) fn painted_button(
    window: HWND,
    instance: HINSTANCE,
    id: usize,
    title: PCWSTR,
) -> Result<HWND> {
    let control = child(
        window,
        instance,
        id,
        w!("BUTTON"),
        title,
        WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
        WINDOW_EX_STYLE::default(),
    )?;
    super::theme::track_hover(control);
    Ok(control)
}

pub(super) fn edit(window: HWND, instance: HINSTANCE, id: usize, limit: usize) -> Result<HWND> {
    let control = child(
        window,
        instance,
        id,
        w!("EDIT"),
        w!(""),
        WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        WS_EX_CLIENTEDGE,
    )?;
    send(control, EM_SETLIMITTEXT, limit, 0);
    Ok(control)
}

pub(super) fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

pub(super) fn set_text(control: HWND, text: impl AsRef<OsStr>) {
    let wide = wide(text.as_ref());
    let _ = unsafe { SetWindowTextW(control, PCWSTR(wide.as_ptr())) };
}

pub(super) fn get_text(control: HWND) -> OsString {
    let length = unsafe { GetWindowTextLengthW(control) }.max(0) as usize;
    let mut wide = vec![0; length + 1];
    let length = unsafe { GetWindowTextW(control, &mut wide) }.max(0) as usize;
    OsString::from_wide(&wide[..length])
}

pub(super) fn cue(control: HWND, text: PCWSTR) {
    send(control, EM_SETCUEBANNER, 0, text.0 as isize);
}

pub(super) fn send(control: HWND, message: u32, w: usize, l: isize) -> isize {
    unsafe { SendMessageW(control, message, Some(WPARAM(w)), Some(LPARAM(l))).0 }
}

pub(super) fn place(control: HWND, x: i32, y: i32, width: i32, height: i32) {
    let _ = unsafe { MoveWindow(control, x, y, width.max(1), height.max(1), true) };
}

pub(super) fn show(control: HWND, visible: bool) {
    unsafe {
        let _ = ShowWindow(control, if visible { SW_SHOW } else { SW_HIDE });
    }
}

pub(super) fn reset_list(list: HWND, count: u32) {
    let state = LVITEMW {
        stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
        ..Default::default()
    };
    send(
        list,
        LVM_SETITEMSTATE,
        usize::MAX,
        std::ptr::from_ref(&state) as isize,
    );
    send(list, LVM_SETITEMCOUNT, count as usize, 0);
    if count > 0 {
        send(list, LVM_ENSUREVISIBLE, 0, 0);
    }
    let _ = unsafe { InvalidateRect(Some(list), None, true) };
}

pub(super) fn write_cell(buffer: &mut [u16], text: &OsStr) {
    let mut length = 0;
    let capacity = buffer.len().saturating_sub(1);
    for unit in text.encode_wide().take(capacity) {
        buffer[length] = unit;
        length += 1;
    }
    if length == capacity && length > 0 && (0xD800..=0xDBFF).contains(&buffer[length - 1]) {
        length -= 1;
    }
    buffer[length] = 0;
}
