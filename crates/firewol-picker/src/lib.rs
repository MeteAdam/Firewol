//! A probe for Windows' Open and Save windows. Registered for one user, it is loaded in place of
//! the system's class, notes which program asked and hands back the real Windows window, so
//! nothing about the window changes. It answers one question: which programs could Firewol serve?
#![cfg(windows)]

use std::ffi::c_void;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, HMODULE, S_FALSE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_core::{GUID, HRESULT, HSTRING, s};

const FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xdc1c5a9c_e88a_4dde_a5a1_60f82a20aef7);
const FILE_SAVE_DIALOG: GUID = GUID::from_u128(0xc0b4e2f3_ba21_4773_8dba_335ec946eb8b);
/// Stops a runaway program from filling the disk one line at a time.
const MAX_LOG_BYTES: u64 = 64 * 1024;

type GetClassObject =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

/// Windows' own implementation, loaded by full path: the registry now names this file instead.
fn system_get_class_object() -> Option<GetClassObject> {
    static FUNCTION: OnceLock<Option<GetClassObject>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        let mut buffer = [0u16; 260];
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("comdlg32.dll");
        let module: HMODULE = unsafe { LoadLibraryW(&HSTRING::from(path.as_os_str())) }.ok()?;
        let function = unsafe { GetProcAddress(module, s!("DllGetClassObject")) }?;
        // The export has this signature by the COM contract.
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, GetClassObject>(function)
        })
    })
}

fn note(kind: &str) {
    let Some(folder) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let path = PathBuf::from(folder).join(r"Firewol\file-dialog-probe.log");
    if fs::metadata(&path).is_ok_and(|item| item.len() > MAX_LOG_BYTES) {
        return;
    }
    let program = std::env::current_exe().unwrap_or_default();
    if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(log, "{kind}\t{}", program.display());
    }
}

/// # Safety
/// Called by COM with valid pointers, as for any in-process server.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const GUID,
    interface: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    let Some(requested) = (unsafe { class.as_ref() }) else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    let kind = match *requested {
        FILE_OPEN_DIALOG => "open",
        FILE_SAVE_DIALOG => "save",
        _ => return CLASS_E_CLASSNOTAVAILABLE,
    };
    // A failure to log must never cost the program its window.
    let _ = std::panic::catch_unwind(|| note(kind));
    match system_get_class_object() {
        Some(real) => unsafe { real(class, interface, object) },
        None => CLASS_E_CLASSNOTAVAILABLE,
    }
}

/// The real implementation's factory lives on, so this file stays loaded with it.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
