//! The drives and Windows user folders the sidebar shows, and device-change notifications.

use std::mem::size_of;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_REMOTE, DRIVE_REMOVABLE};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Profile, FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86,
    FOLDERID_RoamingAppData, FOLDERID_StartMenu, FOLDERID_System, FOLDERID_UserProfiles,
    FOLDERID_Videos, FOLDERID_Windows, KF_FLAG_DEFAULT, SHFILEINFOW, SHGFI_DISPLAYNAME,
    SHGetFileInfoW, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
    WINDOW_EX_STYLE, WM_DEVICECHANGE, WNDCLASSW, WS_POPUP,
};
use windows_core::{GUID, HSTRING, PCWSTR, w};

use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;

const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_UNKNOWN: u32 = 0;
const DBT_DEVNODES_CHANGED: usize = 0x0007;
const DBT_DEVICEARRIVAL: usize = 0x8000;
const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;

#[derive(Clone, Debug)]
pub struct Drive {
    pub path: PathBuf,
    /// The name Explorer shows, for example `Local Disk (C:)`.
    pub name: String,
}

fn from_wide(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// The volume label, or `None` when the drive has no readable media.
fn volume_label(root: &HSTRING) -> Option<String> {
    let mut label = [0u16; 261];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            Some(&mut label),
            None,
            None,
            None,
            None,
        )
    }
    .ok()?;
    Some(from_wide(&label))
}

/// Explorer's own text for an unlabelled drive, in the Windows display language.
fn shell_name(root: &HSTRING) -> Option<String> {
    let mut info = SHFILEINFOW::default();
    let found = unsafe {
        SHGetFileInfoW(
            PCWSTR(root.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_DISPLAYNAME,
        )
    };
    let name = from_wide(&info.szDisplayName);
    (found != 0 && !name.is_empty()).then_some(name)
}

/// Every drive that can be opened now, in letter order. A disconnected network drive stays
/// listed by letter, because asking it for a label can block for many seconds.
pub fn drives() -> Vec<Drive> {
    let letters = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|bit| letters & (1 << bit) != 0)
        .filter_map(|bit| {
            let letter = char::from(b'A' + bit as u8);
            let path = format!("{letter}:\\");
            let root = HSTRING::from(&path);
            let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
            let name = match kind {
                DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR => return None,
                DRIVE_REMOTE => format!("Network Drive ({letter}:)"),
                _ => {
                    let label = volume_label(&root);
                    if label.is_none() && matches!(kind, DRIVE_REMOVABLE | DRIVE_CDROM) {
                        return None;
                    }
                    match label.filter(|label| !label.is_empty()) {
                        Some(label) => format!("{label} ({letter}:)"),
                        None => {
                            shell_name(&root).unwrap_or_else(|| format!("Local Disk ({letter}:)"))
                        }
                    }
                }
            };
            Some(Drive {
                path: PathBuf::from(path),
                name,
            })
        })
        .collect()
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let found = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    let path = unsafe { found.to_string() }.ok().map(PathBuf::from);
    unsafe { CoTaskMemFree(Some(found.0 as _)) };
    path.filter(|path| path.is_dir())
}

/// The folders Explorer's Quick access starts with, in its order.
pub(super) fn known_folders() -> Vec<PathBuf> {
    [
        FOLDERID_Desktop,
        FOLDERID_Downloads,
        FOLDERID_Documents,
        FOLDERID_Pictures,
        FOLDERID_Music,
        FOLDERID_Videos,
    ]
    .iter()
    .filter_map(known_folder)
    .collect()
}

/// The folders that start as tiles rather than details.
pub(super) fn media_folders() -> Vec<PathBuf> {
    [FOLDERID_Pictures, FOLDERID_Videos]
        .iter()
        .filter_map(known_folder)
        .collect()
}

/// The folders of the Go To menu, in the order and with the names of Nilesoft's `goto.nss`.
pub fn go_to_folders() -> Vec<(&'static str, PathBuf)> {
    let known = [
        ("Windows", FOLDERID_Windows),
        ("System", FOLDERID_System),
        ("Program Files", FOLDERID_ProgramFiles),
        ("Program Files x86", FOLDERID_ProgramFilesX86),
        ("ProgramData", FOLDERID_ProgramData),
        ("Users", FOLDERID_UserProfiles),
        ("Desktop", FOLDERID_Desktop),
        ("Downloads", FOLDERID_Downloads),
        ("Pictures", FOLDERID_Pictures),
        ("Documents", FOLDERID_Documents),
        ("Startmenu", FOLDERID_StartMenu),
        ("Profile", FOLDERID_Profile),
        ("AppData", FOLDERID_RoamingAppData),
    ];
    known
        .iter()
        .filter_map(|(name, id)| Some((*name, known_folder(id)?)))
        .chain(Some(("Temp", std::env::temp_dir())))
        .collect()
}

type Changed = Box<dyn Fn() + Send + Sync>;
static CHANGED: OnceLock<Changed> = OnceLock::new();

unsafe extern "system" fn device_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_DEVICECHANGE
        && matches!(
            wparam.0,
            DBT_DEVNODES_CHANGED | DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE
        )
        && let Some(changed) = CHANGED.get()
    {
        changed();
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Calls `changed` when a drive appears or disappears. WM_DEVICECHANGE is broadcast to top-level
/// windows only, so this is an invisible popup rather than a message-only window.
pub fn watch_devices(changed: impl Fn() + Send + Sync + 'static) -> Result<()> {
    CHANGED
        .set(Box::new(changed))
        .ok()
        .context("Device changes are already watched")?;
    std::thread::Builder::new()
        .name("device-watch".into())
        .spawn(|| unsafe {
            let Ok(module) = GetModuleHandleW(None) else {
                return;
            };
            let class = w!("FirewolDeviceWatch");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(device_window),
                hInstance: module.into(),
                lpszClassName: class,
                ..Default::default()
            });
            if CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(module.into()),
                None,
            )
            .is_err()
            {
                return;
            }
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = DispatchMessageW(&message);
            }
        })
        .context("Start the device watcher")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_drive_is_listed_with_a_name() {
        let drives = drives();
        let system = drives
            .iter()
            .find(|drive| drive.path == std::path::Path::new("C:\\"))
            .expect("C: is listed");
        assert!(system.name.ends_with("(C:)"), "{}", system.name);
    }

    #[test]
    fn known_folders_resolve_to_existing_folders() {
        let folders = known_folders();
        assert!(!folders.is_empty());
        assert!(folders.iter().all(|folder| folder.is_dir()));
    }
}
