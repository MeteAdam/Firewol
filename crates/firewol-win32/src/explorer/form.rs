use std::cell::{Cell, RefCell};
use std::ffi::OsString;

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::{Filters, Rgb};
use time::{Date, Month, Time};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, IsWindowEnabled, SetActiveWindow, SetFocus, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::color;
use super::controls::{button, child, edit, get_text, place, send, set_text, show, wide};

const CLASS: PCWSTR = w!("FirewolInput");

type Handler<'a> = dyn Fn(HWND, u32, WPARAM, LPARAM) -> Option<LRESULT> + 'a;

struct Callback<'a>(&'a Handler<'a>);

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CTLCOLORSTATIC {
        unsafe {
            SetBkMode(HDC(wparam.0 as *mut _), TRANSPARENT);
            return LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize);
        }
    }
    let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Callback<'_>;
    if let Some(callback) = unsafe { data.as_ref() }
        && let Some(result) = (callback.0)(window, message, wparam, lparam)
    {
        return result;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

pub(super) fn register(instance: HINSTANCE) -> Result<()> {
    let class = WNDCLASSW {
        hInstance: instance,
        lpszClassName: CLASS,
        lpfnWndProc: Some(procedure),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
        ..Default::default()
    };
    ensure!(
        unsafe { RegisterClassW(&class) } != 0,
        "Register Firewol input window"
    );
    Ok(())
}

pub(super) struct Dialog {
    pub window: HWND,
    pub instance: HINSTANCE,
    owner: HWND,
}

impl Drop for Dialog {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.window) };
    }
}

pub(super) struct Reply<T> {
    result: RefCell<Option<T>>,
    done: Cell<bool>,
}

impl<T> Reply<T> {
    pub fn accept(&self, result: T) {
        self.result.replace(Some(result));
        self.done.set(true);
    }
}

impl Dialog {
    pub fn create(owner: HWND, title: PCWSTR, width: i32, height: i32) -> Result<Self> {
        let instance = HINSTANCE(unsafe { GetWindowLongPtrW(owner, GWLP_HINSTANCE) } as *mut _);
        let mut bounds = RECT::default();
        unsafe { GetWindowRect(owner, &mut bounds)? };
        let mut x = bounds.left + (bounds.right - bounds.left - width) / 2;
        let mut y = bounds.top + (bounds.bottom - bounds.top - height) / 2;
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        }
        .as_bool()
        {
            let work = monitor.rcWork;
            x = x.clamp(work.left, (work.right - width).max(work.left));
            y = y.clamp(work.top, (work.bottom - height).max(work.top));
        }
        let window = unsafe {
            CreateWindowExW(
                WS_EX_DLGMODALFRAME,
                CLASS,
                title,
                WS_POPUP | WS_CAPTION | WS_SYSMENU,
                x,
                y,
                width,
                height,
                Some(owner),
                None,
                Some(instance),
                None,
            )?
        };
        Ok(Self {
            window,
            instance,
            owner,
        })
    }

    pub fn button(&self, id: usize, title: PCWSTR, x: i32, y: i32, width: i32) -> Result<HWND> {
        let control = button(self.window, self.instance, id, title)?;
        place(control, x, y, width, 26);
        Ok(control)
    }

    pub fn text(&self, id: usize, text: &str, x: i32, y: i32, width: i32) -> Result<HWND> {
        let text = wide(text.as_ref());
        let control = child(
            self.window,
            self.instance,
            id,
            w!("STATIC"),
            PCWSTR(text.as_ptr()),
            WINDOW_STYLE::default(),
            WINDOW_EX_STYLE::default(),
        )?;
        place(control, x, y, width, 23);
        Ok(control)
    }

    pub fn run<T>(
        &self,
        focus: HWND,
        enter_fields: &[HWND],
        handle: impl Fn(&Reply<T>, u32, WPARAM, LPARAM) -> Option<LRESULT>,
    ) -> Result<Option<T>> {
        let reply = Reply {
            result: RefCell::default(),
            done: Cell::new(false),
        };
        let dispatch = |_, message, wparam: WPARAM, lparam| match message {
            WM_CLOSE => {
                reply.done.set(true);
                Some(LRESULT(0))
            }
            WM_COMMAND if wparam.0 & 0xffff == 2 => {
                reply.done.set(true);
                Some(LRESULT(0))
            }
            _ => handle(&reply, message, wparam, lparam),
        };
        let callback = Callback(&dispatch);
        let previously_enabled = unsafe { IsWindowEnabled(self.owner) }.as_bool();
        let mut next = 1;
        unsafe {
            SetWindowLongPtrW(
                self.window,
                GWLP_USERDATA,
                std::ptr::from_ref(&callback) as isize,
            );
            let _ = EnableWindow(self.owner, false);
            show(self.window, true);
            let _ = SetFocus(Some(focus));
            if enter_fields.contains(&focus) {
                send(focus, EM_SETSEL, 0, -1);
            }
            let mut message = MSG::default();
            while !reply.done.get() {
                next = GetMessageW(&mut message, None, 0, 0).0;
                if next <= 0 {
                    if next == 0 {
                        PostQuitMessage(message.wParam.0 as i32);
                    }
                    break;
                }
                if message.message == WM_KEYDOWN && message.wParam.0 == VK_ESCAPE.0 as usize {
                    break;
                }
                if message.message == WM_KEYDOWN
                    && message.wParam.0 == VK_RETURN.0 as usize
                    && enter_fields.contains(&message.hwnd)
                {
                    send(self.window, WM_COMMAND, 1, 0);
                } else if !IsDialogMessageW(self.window, &message).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            show(self.window, false);
            if previously_enabled {
                let _ = EnableWindow(self.owner, true);
                let _ = SetActiveWindow(self.owner);
            }
        }
        ensure!(next >= 0, "Read the Firewol dialog message queue");
        Ok(reply.result.into_inner())
    }
}

pub(super) enum Field<'a> {
    Text(&'a str, OsString, usize),
    Choice(&'a str, &'a [&'a str], usize),
    Color(&'a str, Option<Rgb>),
}

impl Field<'_> {
    fn label(&self) -> &str {
        match self {
            Self::Text(label, ..) | Self::Choice(label, ..) | Self::Color(label, ..) => label,
        }
    }
}

pub(super) fn fields<T>(
    owner: HWND,
    title: PCWSTR,
    entries: &[Field<'_>],
    hint: &str,
    parse: impl Fn(&[OsString]) -> Result<T>,
) -> Result<Option<T>> {
    let dialog = Dialog::create(owner, title, 526, 33 * entries.len() as i32 + 119)?;
    let window = dialog.window;
    let instance = dialog.instance;
    let mut inputs = Vec::new();
    let mut enter = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let y = 12 + 33 * i as i32;
        dialog.text(100 + i, entry.label(), 14, y + 4, 173)?;
        let id = 200 + i;
        let control = match entry {
            Field::Text(_, initial, limit) => {
                let control = edit(window, instance, id, *limit)?;
                place(control, 184, y, 312, 26);
                set_text(control, initial);
                enter.push(control);
                control
            }
            Field::Choice(_, choices, selected) => {
                let control = child(
                    window,
                    instance,
                    id,
                    w!("COMBOBOX"),
                    w!(""),
                    WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                    WINDOW_EX_STYLE::default(),
                )?;
                place(control, 184, y, 312, 220);
                for choice in *choices {
                    let text = wide(choice.as_ref());
                    send(control, CB_ADDSTRING, 0, text.as_ptr() as isize);
                }
                send(control, CB_SETCURSEL, *selected, 0);
                control
            }
            Field::Color(_, initial) => {
                let control = edit(window, instance, id, 7)?;
                place(control, 184, y, 230, 26);
                set_text(control, initial.map(color::hex).unwrap_or_default());
                dialog.button(300 + i, w!("Wheel…"), 424, y, 72)?;
                enter.push(control);
                control
            }
        };
        inputs.push(control);
    }
    let y = 17 + 33 * entries.len() as i32;
    let status = dialog.text(401, hint, 14, y, 482)?;
    dialog.button(1, w!("Apply"), 329, y + 27, 78)?;
    dialog.button(2, w!("Cancel"), 417, y + 27, 79)?;
    dialog.run(inputs[0], &enter, |reply, message, wparam, _| {
        if message != WM_COMMAND {
            return None;
        }
        let id = wparam.0 & 0xffff;
        let result = if id == 1 {
            let values: Vec<_> = entries
                .iter()
                .zip(&inputs)
                .map(|(entry, control)| {
                    if let Field::Choice(_, choices, _) = entry {
                        let selected = send(*control, CB_GETCURSEL, 0, 0);
                        choices
                            .get(selected as usize)
                            .copied()
                            .unwrap_or_default()
                            .into()
                    } else {
                        get_text(*control)
                    }
                })
                .collect();
            parse(&values).map(|value| reply.accept(value))
        } else if let Some(index) = id.checked_sub(300).filter(|index| *index < entries.len())
            && matches!(entries[index], Field::Color(..))
        {
            let input = inputs[index];
            let initial = color::parse(&get_text(input)).ok().flatten();
            color::pick(window, initial).map(|value| {
                if let Some(value) = value {
                    set_text(input, color::hex(value));
                }
            })
        } else {
            return None;
        };
        if let Err(error) = result {
            set_text(status, format!("{error}"));
        }
        Some(LRESULT(0))
    })
}

fn prompt<T>(
    owner: HWND,
    title: PCWSTR,
    entries: &[(&str, OsString)],
    parse: impl Fn(&[OsString]) -> Result<T>,
) -> Result<Option<T>> {
    let entries: Vec<_> = entries
        .iter()
        .map(|(label, value)| Field::Text(label, value.clone(), 255))
        .collect();
    fields(owner, title, &entries, "", parse)
}

pub(super) fn rename(owner: HWND, name: OsString) -> Result<Option<OsString>> {
    prompt(owner, w!("Rename item"), &[("New name", name)], |values| {
        ensure!(!values[0].is_empty(), "Enter a name");
        Ok(values[0].clone())
    })
}

pub(super) fn filters(owner: HWND, previous: &Filters) -> Result<Option<Filters>> {
    let number =
        |value: Option<u64>| value.map_or_else(OsString::new, |value| value.to_string().into());
    let date = |value: Option<i64>| {
        value
            .and_then(|value| time::OffsetDateTime::from_unix_timestamp(value).ok())
            .map_or_else(OsString::new, |value| value.date().to_string().into())
    };
    let entries = [
        (
            "Extension",
            previous.extension.as_deref().unwrap_or("").into(),
        ),
        ("Minimum bytes", number(previous.min_bytes)),
        ("Maximum bytes", number(previous.max_bytes)),
        ("From (YYYY-MM-DD)", date(previous.modified_after)),
        ("Through (YYYY-MM-DD)", date(previous.modified_before)),
    ];
    prompt(
        owner,
        w!("Search filters · UTC dates"),
        &entries,
        |values| {
            let extension = values[0]
                .to_string_lossy()
                .trim()
                .trim_start_matches('.')
                .to_owned();
            let min_bytes = optional_number(&values[1])?;
            let max_bytes = optional_number(&values[2])?;
            let modified_after = optional_date(&values[3])?;
            let modified_before = optional_date(&values[4])?.map(|time| time + 86_399);
            ensure!(
                min_bytes.zip(max_bytes).is_none_or(|(min, max)| min <= max),
                "Minimum bytes exceed maximum bytes"
            );
            ensure!(
                modified_after
                    .zip(modified_before)
                    .is_none_or(|(min, max)| min <= max),
                "The date range is reversed"
            );
            Ok(Filters {
                extension: (!extension.is_empty()).then_some(extension),
                min_bytes,
                max_bytes,
                modified_after,
                modified_before,
            })
        },
    )
}

fn optional_number(value: &OsString) -> Result<Option<u64>> {
    let value = value.to_string_lossy();
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let bytes = value
        .parse::<i64>()
        .context("Enter a whole number of bytes")?;
    Ok(Some(
        u64::try_from(bytes).context("File size must be positive")?,
    ))
}

fn optional_date(value: &OsString) -> Result<Option<i64>> {
    let value = value.to_string_lossy();
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parts: Vec<_> = value.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        anyhow::bail!("Use YYYY-MM-DD for dates")
    };
    let date = Date::from_calendar_date(
        year.parse()?,
        Month::try_from(month.parse::<u8>()?)?,
        day.parse()?,
    )?;
    Ok(Some(
        date.with_time(Time::MIDNIGHT).assume_utc().unix_timestamp(),
    ))
}
