//! Menu icon resources identified by the Shell, resolved outside the WebView.
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;

use windows::Win32::Foundation::{ERROR_SUCCESS, FreeLibrary, HINSTANCE};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::UI::Shell::{
    FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::LoadStringW;
use windows_core::{HSTRING, PCWSTR, PWSTR};

use super::shell_menu::clean_label;

const WINRAR: &str = "b41db860-64e4-11d2-9906-e49fadc173ca";
const SEVEN_ZIP: &str = "23170f69-40c1-278a-1000-000100020000";
const DEFENDER: &str = "09a47860-11b0-4da5-afa5-26d86198a780";

#[derive(Clone)]
pub struct ResourceIcon {
    pub(super) path: PathBuf,
    pub(super) index: i32,
}

fn system_directory() -> Option<PathBuf> {
    let mut buffer = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    (len > 0 && len < buffer.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

fn trusted_roots() -> &'static [PathBuf] {
    static ROOTS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut roots: Vec<PathBuf> = system_directory().into_iter().collect();
        for id in [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86] {
            if let Ok(path) = unsafe { SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) } {
                let value = unsafe { path.to_string() }.ok();
                unsafe { CoTaskMemFree(Some(path.0.cast())) };
                roots.extend(value.map(PathBuf::from));
            }
        }
        roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .collect()
    })
}

fn resource(path: &Path, index: i32) -> Option<ResourceIcon> {
    if !path.is_absolute()
        || !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)))
        || !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ["dll", "exe", "ico", "mun"]
                    .iter()
                    .any(|kind| ext.eq_ignore_ascii_case(kind))
            })
        || path
            .to_string_lossy()
            .chars()
            .filter(|&ch| ch == ':')
            .count()
            != 1
    {
        return None;
    }
    let path = path.canonicalize().ok()?;
    if !trusted_roots().iter().any(|root| path.starts_with(root)) || !path.is_file() {
        return None;
    }
    // The Shell expects a DOS path. A canonicalized UNC or device path cannot pass the roots check.
    let path = PathBuf::from(path.to_str()?.strip_prefix(r"\\?\")?);
    Some(ResourceIcon { path, index })
}

fn parse_location(location: &str) -> Option<(&str, i32)> {
    let (path, index) = match location.trim().rsplit_once(',') {
        Some((path, index)) => (path, index.trim().parse().ok()?),
        None => (location, 0),
    };
    let path = path.trim().trim_matches('"');
    (!path.is_empty() && !path.contains('\0')).then_some((path, index))
}

pub(super) fn modern(location: &str) -> Option<ResourceIcon> {
    let (path, index) = parse_location(location)?;
    let path = PathBuf::from(path.replace('/', "\\"));
    if matches!(path.components().next(), Some(Component::Normal(_)))
        && path.components().count() == 1
    {
        return resource(&system_directory()?.join(path), index);
    }
    resource(&path, index)
}

fn registered(clsid: &str) -> Option<ResourceIcon> {
    let subkey = HSTRING::from(format!(
        r"SOFTWARE\Classes\CLSID\{{{clsid}}}\InprocServer32"
    ));
    let mut buffer = [0u16; 1024];
    let mut bytes = size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &subkey,
            PCWSTR::null(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buffer.iter().position(|&ch| ch == 0)?;
    let location = String::from_utf16(&buffer[..end]).ok()?;
    resource(Path::new(location.trim().trim_matches('"')), 0)
}

fn defender_title(icon: &ResourceIcon) -> Option<String> {
    let Ok(module) = (unsafe {
        LoadLibraryExW(
            &HSTRING::from(icon.path.as_os_str()),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    }) else {
        return None;
    };
    let mut buffer = [0u16; 512];
    // The installed extension's MUI string uses a brand placeholder on current Windows.
    let count = unsafe {
        LoadStringW(
            Some(HINSTANCE(module.0)),
            101,
            PWSTR(buffer.as_mut_ptr()),
            buffer.len() as i32,
        )
    };
    unsafe {
        let _ = FreeLibrary(module);
    }
    if count <= 0 || count as usize >= buffer.len() - 1 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..count as usize]))
}

#[derive(Clone, Copy)]
pub(super) enum Archiver {
    WinRar,
    SevenZip,
}

impl Archiver {
    pub(super) fn of(verb: &str) -> Option<Self> {
        match verb {
            WINRAR => Some(Self::WinRar),
            verb if verb.starts_with("sevenzip") => Some(Self::SevenZip),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct Classic {
    winrar: std::cell::OnceCell<Option<ResourceIcon>>,
    seven_zip: std::cell::OnceCell<Option<ResourceIcon>>,
    defender: std::cell::OnceCell<Option<(ResourceIcon, String)>>,
}

impl Classic {
    pub(super) fn resolve(
        &self,
        archiver: Option<Archiver>,
        defender_label: Option<&str>,
    ) -> Option<ResourceIcon> {
        match archiver {
            Some(Archiver::WinRar) => self.winrar.get_or_init(|| registered(WINRAR)).clone(),
            Some(Archiver::SevenZip) => {
                self.seven_zip.get_or_init(|| registered(SEVEN_ZIP)).clone()
            }
            None => {
                let label = defender_label?;
                let (icon, template) = self
                    .defender
                    .get_or_init(|| {
                        let icon = registered(DEFENDER)?;
                        let title = defender_title(&icon)?;
                        Some((icon, title))
                    })
                    .as_ref()?;
                ["Microsoft Defender", "Windows Defender"]
                    .iter()
                    .any(|brand| {
                        clean_label(&template.replace("$(BrandName)", brand))
                            .eq_ignore_ascii_case(label)
                    })
                    .then(|| icon.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_locations_keep_signed_indices_and_commas_in_paths() {
        assert_eq!(
            parse_location(r#""C:\Program Files\App, Inc\ext.dll",-249"#),
            Some((r"C:\Program Files\App, Inc\ext.dll", -249))
        );
        assert!(parse_location("shell32.dll,unknown").is_none());
        assert!(modern(r"\\server\share\ext.dll,-1").is_none());
        assert!(modern(r"..\shell32.dll,-1").is_none());
    }
}
