//! Changes the icon Windows itself shows for a folder (`desktop.ini`) or a shortcut (`.lnk`).
//! The change lands on the real item, through the same APIs Explorer's dialogs use.

use std::mem::size_of;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile, STGM_READWRITE,
};
use windows::Win32::UI::Shell::{
    FCS_FORCEWRITE, FCS_READ, FCSM_ICONFILE, IShellLinkW, PickIconDlg, SHCNE_UPDATEITEM,
    SHCNF_FLUSHNOWAIT, SHCNF_PATHW, SHChangeNotify, SHFOLDERCUSTOMSETTINGS,
    SHGetSetFolderCustomSettings, ShellLink,
};
use windows_core::{HSTRING, Interface, PCWSTR, PWSTR};

use super::{ItemId, display_path, icons};

const DEFAULT_LIBRARY: &str = r"%SystemRoot%\System32\shell32.dll";
const DEFAULT_FOLDER_INDEX: i32 = 3;
const MAX_PATH: usize = 260;

#[derive(Clone, Copy)]
enum Kind {
    Folder,
    Shortcut,
}

/// Re-verifies the identity immediately before use, so a replaced item is never changed.
fn target(item: &ItemId) -> Result<(Kind, PathBuf)> {
    let directory = item.verify()?.metadata()?.is_dir();
    let path = PathBuf::from(display_path(item.path()));
    let kind = if directory {
        Kind::Folder
    } else if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
    {
        Kind::Shortcut
    } else {
        bail!("Only folders and shortcuts have an icon that can be changed");
    };
    Ok((kind, path))
}

/// These Shell objects are apartment-threaded, and the picker is a modal dialog.
fn on_sta<T: Send>(task: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                    .ok()
                    .context("Start the Shell icon apartment")?;
                let result = task();
                unsafe { CoUninitialize() };
                result
            })
            .join()
            .unwrap_or_else(|_| bail!("The Shell icon task stopped unexpectedly"))
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn from_wide(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn open_link(path: &Path, mode_write: bool) -> Result<(IShellLinkW, IPersistFile)> {
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    let file: IPersistFile = link.cast()?;
    let mode = if mode_write {
        STGM_READWRITE
    } else {
        windows::Win32::System::Com::STGM_READ
    };
    unsafe { file.Load(&HSTRING::from(path.as_os_str()), mode) }
        .with_context(|| format!("Open the shortcut {}", path.display()))?;
    Ok((link, file))
}

/// The folder a shortcut points to, so Firewol can show it instead of starting Explorer.
pub fn shortcut_folder(path: &Path) -> Option<PathBuf> {
    let shortcut = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"));
    if !shortcut {
        return None;
    }
    let target = on_sta(|| {
        let (link, _file) = open_link(path, false)?;
        let mut buffer = [0u16; MAX_PATH];
        unsafe { link.GetPath(&mut buffer, std::ptr::null_mut(), 0)? };
        Ok(PathBuf::from(from_wide(&buffer)))
    });
    target.ok().filter(|target| target.is_dir())
}

fn current(kind: Kind, path: &Path) -> Result<(String, i32)> {
    let mut buffer = [0u16; MAX_PATH];
    match kind {
        Kind::Folder => {
            // The read fails outright on a folder that has never been customized.
            if !path.join("desktop.ini").exists() {
                return Ok((String::new(), 0));
            }
            let mut settings = SHFOLDERCUSTOMSETTINGS {
                dwSize: size_of::<SHFOLDERCUSTOMSETTINGS>() as u32,
                dwMask: FCSM_ICONFILE,
                pszIconFile: PWSTR(buffer.as_mut_ptr()),
                cchIconFile: MAX_PATH as u32,
                ..Default::default()
            };
            unsafe {
                SHGetSetFolderCustomSettings(
                    &mut settings,
                    &HSTRING::from(path.as_os_str()),
                    FCS_READ,
                )?;
            }
            Ok((from_wide(&buffer), settings.iIconIndex))
        }
        Kind::Shortcut => {
            let (link, _file) = open_link(path, false)?;
            let mut index = 0;
            unsafe { link.GetIconLocation(&mut buffer, &mut index)? };
            Ok((from_wide(&buffer), index))
        }
    }
}

/// Writes the icon (`None` restores the default) and returns the icon key to refresh.
fn write(item: &ItemId, icon: Option<(&str, i32)>) -> Result<String> {
    let (kind, path) = target(item)?;
    let refused = || format!("Windows did not change the icon of {}", path.display());
    match kind {
        Kind::Folder => {
            let mut file = icon.map(|(file, _)| wide(file));
            let mut settings = SHFOLDERCUSTOMSETTINGS {
                dwSize: size_of::<SHFOLDERCUSTOMSETTINGS>() as u32,
                dwMask: FCSM_ICONFILE,
                pszIconFile: file
                    .as_mut()
                    .map_or_else(PWSTR::null, |file| PWSTR(file.as_mut_ptr())),
                iIconIndex: icon.map_or(0, |(_, index)| index),
                ..Default::default()
            };
            unsafe {
                SHGetSetFolderCustomSettings(
                    &mut settings,
                    &HSTRING::from(path.as_os_str()),
                    FCS_FORCEWRITE,
                )
            }
            .with_context(refused)?;
        }
        Kind::Shortcut => {
            let (link, file) = open_link(&path, true).with_context(refused)?;
            let (location, index) = icon.unwrap_or(("", 0));
            unsafe { link.SetIconLocation(&HSTRING::from(location), index) }
                .with_context(refused)?;
            unsafe { file.Save(PCWSTR::null(), true) }.with_context(refused)?;
        }
    }
    let name: Vec<u16> = wide(&path.to_string_lossy());
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
            Some(name.as_ptr().cast()),
            None,
        );
    }
    Ok(icons::path_key(&path))
}

/// Shows Windows' own Change Icon dialog and applies the choice. `None` means it was cancelled.
pub fn pick(owner: isize, item: &ItemId) -> Result<Option<String>> {
    on_sta(|| {
        let (kind, path) = target(item)?;
        let (mut file, mut index) = current(kind, &path).unwrap_or_default();
        if file.is_empty() {
            file = DEFAULT_LIBRARY.to_owned();
            index = DEFAULT_FOLDER_INDEX;
        }
        let mut buffer = [0u16; MAX_PATH];
        for (slot, unit) in buffer[..MAX_PATH - 1].iter_mut().zip(file.encode_utf16()) {
            *slot = unit;
        }
        let chosen = unsafe { PickIconDlg(Some(HWND(owner as _)), &mut buffer, Some(&mut index)) };
        if chosen == 0 {
            return Ok(None);
        }
        write(item, Some((&from_wide(&buffer), index))).map(Some)
    })
}

/// Applies `file`,`index` directly. Returns the icon key to refresh.
pub fn set(item: &ItemId, file: &str, index: i32) -> Result<String> {
    on_sta(|| write(item, Some((file, index))))
}

/// Restores the default icon. Returns the icon key to refresh.
pub fn reset(item: &ItemId) -> Result<String> {
    on_sta(|| write(item, None))
}

/// The icon file and index the item uses now; an empty file means the default icon.
pub fn location(item: &ItemId) -> Result<(String, i32)> {
    on_sta(|| {
        let (kind, path) = target(item)?;
        current(kind, &path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use windows::Win32::UI::Shell::IShellLinkW;

    const ICON: &str = r"C:\Windows\System32\shell32.dll";

    #[test]
    fn folder_icon_is_written_and_reset() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let folder = fixture.path().join("styled");
        fs::create_dir(&folder)?;
        let item = ItemId::at(folder.clone())?;
        assert!(location(&item)?.0.is_empty());
        let key = set(&item, ICON, 12)?;
        assert!(key.starts_with("path:"));
        assert!(folder.join("desktop.ini").exists());
        assert_eq!(location(&item)?, (ICON.to_owned(), 12));
        reset(&item)?;
        assert!(location(&item)?.0.is_empty());
        // Windows keeps the folder read-only; the temporary fixture must stay removable.
        let mut permissions = fs::metadata(&folder)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&folder, permissions)?;
        Ok(())
    }

    #[test]
    fn shortcut_icon_is_written_and_reset() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let path = fixture.path().join("note.lnk");
        let target_file = fixture.path().join("note.txt");
        fs::write(&target_file, "x")?;
        let created = path.clone();
        on_sta(move || {
            let link: IShellLinkW =
                unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
            unsafe { link.SetPath(&HSTRING::from(target_file.as_os_str()))? };
            let file: IPersistFile = link.cast()?;
            unsafe { file.Save(&HSTRING::from(created.as_os_str()), true)? };
            Ok(())
        })?;
        let item = ItemId::at(path)?;
        set(&item, ICON, 7)?;
        assert_eq!(location(&item)?, (ICON.to_owned(), 7));
        reset(&item)?;
        assert!(location(&item)?.0.is_empty());
        Ok(())
    }

    #[test]
    fn other_files_are_refused() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let path = fixture.path().join("note.txt");
        fs::write(&path, "x")?;
        assert!(set(&ItemId::at(path)?, ICON, 0).is_err());
        Ok(())
    }
}
