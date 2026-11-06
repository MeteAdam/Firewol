use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use anyhow::{Result, ensure};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{DEFAULT_GUI_FONT, GetStockObject, InvalidateRect};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{
    SHGSI_FLAGS, SHGSI_ICON, SHGSI_SMALLICON, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_DOCNOASSOC,
    SIID_FOLDER,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, w};

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
pub(super) const PINS: usize = 111;
pub(super) const LIST: usize = 112;
pub(super) const PREVIOUS: usize = 113;
pub(super) const NEXT: usize = 114;
pub(super) const STATUS: usize = 115;

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
    pub pins: HWND,
    pub list: HWND,
    pub previous: HWND,
    pub next: HWND,
    pub status: HWND,
}

impl Controls {
    pub fn create(window: HWND, instance: HINSTANCE) -> Result<Self> {
        let add_button = |id, title| button(window, instance, id, title);
        let controls = Self {
            back: add_button(BACK, w!("‹ Back"))?,
            forward: add_button(FORWARD, w!("Forward ›"))?,
            up: add_button(UP, w!("↑ Up"))?,
            address: edit(window, instance, ADDRESS, 32767)?,
            go: add_button(GO, w!("Go"))?,
            pin: add_button(PIN, w!("Pin folder"))?,
            search: edit(window, instance, SEARCH, 1024)?,
            regex: child(
                window,
                instance,
                REGEX,
                w!("BUTTON"),
                w!("Regex"),
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                WINDOW_EX_STYLE::default(),
            )?,
            filters: add_button(FILTERS, w!("Filters…"))?,
            volume: add_button(VOLUME, w!("NTFS search…"))?,
            pins: child(
                window,
                instance,
                PINS,
                w!("LISTBOX"),
                w!(""),
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32),
                WS_EX_CLIENTEDGE,
            )?,
            list: child(
                window,
                instance,
                LIST,
                WC_LISTVIEWW,
                w!(""),
                WS_TABSTOP
                    | WINDOW_STYLE(
                        LVS_REPORT
                            | LVS_OWNERDATA
                            | LVS_SINGLESEL
                            | LVS_NOSORTHEADER
                            | LVS_SHOWSELALWAYS,
                    ),
                WS_EX_CLIENTEDGE,
            )?,
            previous: add_button(PREVIOUS, w!("‹ Previous"))?,
            next: add_button(NEXT, w!("Next ›"))?,
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
        cue(controls.address, w!("Folder path"));
        cue(controls.search, w!("Search this folder and its subfolders"));
        send(
            controls.list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize,
        );
        for (index, (title, width)) in [("Name", 440), ("Type", 80), ("Location", 300)]
            .into_iter()
            .enumerate()
        {
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

    pub fn layout(self, window: HWND, show_pins: bool, searching: bool) {
        let mut rect = RECT::default();
        if unsafe { GetClientRect(window, &mut rect) }.is_err() {
            return;
        }
        let right = (rect.right - 14).max(700);
        let bottom = (rect.bottom - 12).max(150);
        place(self.back, 14, 12, 61, 28);
        place(self.forward, 80, 12, 75, 28);
        place(self.up, 160, 12, 48, 28);
        place(self.address, 216, 12, right - 358, 28);
        place(self.go, right - 136, 12, 44, 28);
        place(self.pin, right - 86, 12, 86, 28);
        place(self.search, 14, 48, right - 288, 28);
        place(self.regex, right - 266, 48, 61, 28);
        place(self.filters, right - 200, 48, 74, 28);
        place(self.volume, right - 120, 48, 120, 28);
        let left = if show_pins { 190 } else { 14 };
        let list_height = (bottom - 118).max(1);
        place(self.pins, 14, 86, 168, list_height);
        show(self.pins, show_pins);
        place(self.list, left, 86, right - left, list_height);
        place(
            self.status,
            14,
            bottom - 23,
            right - if searching { 180 } else { 14 },
            22,
        );
        place(self.previous, right - 164, bottom - 26, 84, 26);
        place(self.next, right - 74, bottom - 26, 74, 26);
        show(self.previous, searching);
        show(self.next, searching);
        let width = (right - left - 105).max(160);
        let (name, location) = if searching {
            (width * 53 / 100, width * 47 / 100)
        } else {
            (width, 0)
        };
        send(self.list, LVM_SETCOLUMNWIDTH, 0, name as isize);
        send(self.list, LVM_SETCOLUMNWIDTH, 2, location as isize);
    }
}

fn set_stock_icons(list: HWND) {
    let images = unsafe { ImageList_Create(16, 16, ILC_COLOR32 | ILC_MASK, 2, 0) };
    if images.0 == 0 {
        return;
    }
    for (expected, kind) in [SIID_DOCNOASSOC, SIID_FOLDER].into_iter().enumerate() {
        let mut icon = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        let loaded = unsafe {
            SHGetStockIconInfo(
                kind,
                SHGSI_FLAGS(SHGSI_ICON.0 | SHGSI_SMALLICON.0),
                &mut icon,
            )
        };
        if loaded.is_err() {
            let _ = unsafe { ImageList_Destroy(Some(images)) };
            return;
        }
        let index = unsafe { ImageList_ReplaceIcon(images, -1, icon.hIcon) };
        let _ = unsafe { DestroyIcon(icon.hIcon) };
        if index != expected as i32 {
            let _ = unsafe { ImageList_Destroy(Some(images)) };
            return;
        }
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
