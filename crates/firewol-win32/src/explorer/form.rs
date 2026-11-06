use std::cell::{Cell, RefCell};
use std::ffi::OsString;

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::Filters;
use time::{Date, Month, Time};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, SetActiveWindow, SetFocus, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::controls::{button, child, edit, get_text, place, send, set_text, show, wide};

const CLASS: PCWSTR = w!("FirewolInput");

struct Form<'a> {
    fields: Vec<HWND>,
    status: HWND,
    apply: &'a dyn Fn(&[OsString]) -> Result<()>,
    done: Cell<bool>,
}

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let data = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Form<'_>;
    if let Some(form) = unsafe { data.as_ref() } {
        match message {
            WM_CLOSE => form.done.set(true),
            WM_COMMAND if wparam.0 & 0xffff == 1 => {
                let values: Vec<_> = form.fields.iter().map(|field| get_text(*field)).collect();
                match (form.apply)(&values) {
                    Ok(()) => form.done.set(true),
                    Err(error) => set_text(form.status, format!("{error}")),
                }
            }
            WM_COMMAND if wparam.0 & 0xffff == 2 => form.done.set(true),
            _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
        return LRESULT(0);
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

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

fn prompt<T>(
    owner: HWND,
    title: PCWSTR,
    entries: &[(&str, OsString)],
    parse: impl Fn(&[OsString]) -> Result<T>,
) -> Result<Option<T>> {
    let instance = HINSTANCE(unsafe { GetWindowLongPtrW(owner, GWLP_HINSTANCE) } as *mut _);
    let mut bounds = RECT::default();
    unsafe { GetWindowRect(owner, &mut bounds)? };
    let height = 32 * entries.len() as i32 + 123;
    let window = Window(unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            CLASS,
            title,
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            bounds.left + (bounds.right - bounds.left - 510) / 2,
            bounds.top + (bounds.bottom - bounds.top - height) / 2,
            510,
            height,
            Some(owner),
            None,
            Some(instance),
            None,
        )?
    });
    let mut fields = Vec::new();
    for (i, (label, initial)) in entries.iter().enumerate() {
        let y = 14 + 32 * i as i32;
        let label = wide(label.as_ref());
        let label = child(
            window.0,
            instance,
            200 + i * 2,
            w!("STATIC"),
            PCWSTR(label.as_ptr()),
            WINDOW_STYLE::default(),
            WINDOW_EX_STYLE::default(),
        )?;
        place(label, 15, y + 5, 164, 22);
        let field = edit(window.0, instance, 201 + i * 2, 255)?;
        place(field, 181, y, 294, 26);
        set_text(field, initial);
        fields.push(field);
    }
    let y = 20 + 32 * entries.len() as i32;
    let status = child(
        window.0,
        instance,
        230,
        w!("STATIC"),
        w!(""),
        WINDOW_STYLE::default(),
        WINDOW_EX_STYLE::default(),
    )?;
    place(status, 15, y, 460, 26);
    place(
        button(window.0, instance, 1, w!("OK"))?,
        310,
        y + 28,
        78,
        26,
    );
    place(
        button(window.0, instance, 2, w!("Cancel"))?,
        397,
        y + 28,
        78,
        26,
    );
    let result = RefCell::new(None);
    let apply = |values: &[OsString]| {
        *result.borrow_mut() = Some(parse(values)?);
        Ok(())
    };
    let form = Form {
        fields,
        status,
        apply: &apply,
        done: Cell::new(false),
    };
    unsafe {
        SetWindowLongPtrW(window.0, GWLP_USERDATA, std::ptr::from_ref(&form) as isize);
        let _ = EnableWindow(owner, false);
        show(window.0, true);
        if let Some(first) = form.fields.first() {
            let _ = SetFocus(Some(*first));
            send(*first, EM_SETSEL, 0, -1);
        }
        let mut msg = MSG::default();
        while !form.done.get() {
            let received = GetMessageW(&mut msg, None, 0, 0).0;
            if received <= 0 {
                PostQuitMessage(msg.wParam.0 as i32);
                break;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_ESCAPE.0 as usize {
                break;
            }
            if msg.message == WM_KEYDOWN
                && msg.wParam.0 == VK_RETURN.0 as usize
                && form.fields.contains(&msg.hwnd)
            {
                send(window.0, WM_COMMAND, 1, 0);
            } else if !IsDialogMessageW(window.0, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        SetWindowLongPtrW(window.0, GWLP_USERDATA, 0);
        let _ = EnableWindow(owner, true);
        let _ = SetActiveWindow(owner);
    }
    Ok(result.into_inner())
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
