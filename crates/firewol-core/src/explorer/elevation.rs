use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_READ,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW, SW_SHOWNORMAL,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::{ItemId, attributes, display_path};
use crate::file_identity::identity;
use crate::recycle::Apartment;

/// In order of what each mode allows, so a raised policy compares greater.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationMode {
    #[default]
    Standard,
    Allowlist,
    AllExecutables,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ElevationPolicy {
    pub mode: ElevationMode,
    executables: Vec<TrustedExecutable>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TrustedExecutable {
    path: PathBuf,
    volume: u32,
    index: u64,
}

impl From<&ItemId> for TrustedExecutable {
    fn from(item: &ItemId) -> Self {
        Self {
            path: item.path().to_owned(),
            volume: item.identity.volume,
            index: item.identity.index,
        }
    }
}

impl ElevationPolicy {
    pub fn new(mode: ElevationMode, paths: &[PathBuf]) -> Result<Self> {
        ensure!(paths.len() <= 32, "Use up to 32 allowlisted executables");
        let mut executables = Vec::new();
        for path in paths {
            let item = executable_id(path)?;
            ensure!(!is_network_path(item.path()), NETWORK_ELEVATION);
            let entry = TrustedExecutable::from(&item);
            if !executables.contains(&entry) {
                executables.push(entry);
            }
        }
        Ok(Self { mode, executables })
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.executables.iter().map(|entry| entry.path.as_path())
    }

    pub fn allows(&self, program: &ItemId) -> bool {
        match self.mode {
            ElevationMode::Standard => false,
            ElevationMode::Allowlist => self.executables.iter().any(|entry| {
                entry.path == program.path()
                    && entry.volume == program.identity.volume
                    && entry.index == program.identity.index
            }),
            ElevationMode::AllExecutables => true,
        }
    }
}

const NETWORK_ELEVATION: &str = "Only a program on this PC can run as administrator";

/// True for a UNC path in either form; a mapped drive shows as one once canonicalized.
pub fn is_network_path(path: &Path) -> bool {
    display_path(path).to_string_lossy().starts_with(r"\\")
}

/// Asks the person at the PC directly, outside the WebView, so that script in the interface can
/// neither answer for them nor hide the question. Lists what the change would allow.
pub fn confirm_policy_change(owner: HWND, changes: &[String]) -> bool {
    const SHOWN: usize = 8;
    let mut text = String::from("A settings change would allow this on your PC:\n\n");
    for change in changes.iter().take(SHOWN) {
        text.push_str("\u{2022} ");
        text.push_str(change);
        text.push('\n');
    }
    if changes.len() > SHOWN {
        text.push_str(&format!("and {} more\n", changes.len() - SHOWN));
    }
    text.push_str("\nChoose OK only if you just made this change yourself.");
    let answer = unsafe {
        MessageBoxW(
            Some(owner),
            &HSTRING::from(text),
            w!("Firewol security check"),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    };
    answer == IDOK
}

/// The real System32 folder, which unlike `%SystemRoot%` no process environment can redirect.
pub(super) fn system_directory() -> Result<PathBuf> {
    let mut buffer = [0u16; 261];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    ensure!(
        (1..buffer.len()).contains(&length),
        "Find the Windows system folder"
    );
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

pub fn validate_executable(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe")),
        "Enter an absolute path to an .exe file"
    );
    Ok(())
}

pub fn executable_id(path: &Path) -> Result<ItemId> {
    validate_executable(path)?;
    let path = fs::canonicalize(path).context("Resolve the executable path")?;
    validate_executable(&path)?;
    let item = ItemId::at(path)?;
    let (directory, reparse, _, _) = attributes(&item.verify()?)?;
    ensure!(!directory && !reparse, "Select an executable file");
    Ok(item)
}

fn parameter(path: &OsStr) -> Vec<u16> {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let mut argument = vec![QUOTE];
    let mut backslashes = 0;
    for unit in path.encode_wide() {
        if unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        if unit == QUOTE {
            argument.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
        }
        argument.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        argument.push(unit);
        backslashes = 0;
    }
    argument.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    argument.extend([QUOTE, 0]);
    argument
}

pub fn launch(
    policy: &ElevationPolicy,
    program: &ItemId,
    argument: Option<&ItemId>,
    owner: HWND,
) -> Result<()> {
    if let Some(argument) = argument {
        argument.verify()?;
    }
    let arguments = argument.map(|item| parameter(&display_path(item.path())));
    launch_with(policy, program, arguments.as_deref(), owner)
}

/// For a command line Firewol composes itself, under the same policy and identity checks.
pub fn launch_command(
    policy: &ElevationPolicy,
    program: &ItemId,
    arguments: &str,
    owner: HWND,
) -> Result<()> {
    let arguments: Vec<u16> = arguments.encode_utf16().chain([0]).collect();
    launch_with(policy, program, Some(&arguments), owner)
}

fn launch_with(
    policy: &ElevationPolicy,
    program: &ItemId,
    arguments: Option<&[u16]>,
    owner: HWND,
) -> Result<()> {
    ensure!(
        policy.allows(program),
        "Elevation for this executable is disabled in Admin settings"
    );
    validate_executable(program.path())?;
    ensure!(!is_network_path(program.path()), NETWORK_ELEVATION);
    let pinned = OpenOptions::new()
        .access_mode(FILE_GENERIC_READ.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(program.path())?;
    let (directory, reparse, _, _) = attributes(&pinned)?;
    ensure!(
        !directory && !reparse && identity(&pinned)? == program.identity,
        "The executable changed. Verify it and try again."
    );
    let executable = display_path(program.path());
    let directory = display_path(
        program
            .path()
            .parent()
            .context("The executable has no parent folder")?,
    );
    let wide = |text: OsString| text.encode_wide().chain([0]).collect::<Vec<_>>();
    let executable = wide(executable);
    let directory = wide(directory);
    let _sta = Apartment::sta()?;
    let mut request = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        hwnd: owner,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(executable.as_ptr()),
        lpParameters: arguments.map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    match unsafe { ShellExecuteExW(&mut request) } {
        Err(error) if error.code() == ERROR_CANCELLED.to_hresult() => {
            bail!("Windows elevation was cancelled")
        }
        result => result.context("Request Windows administrator consent"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_paths_are_told_apart_from_local_ones() {
        assert!(is_network_path(Path::new(r"\\server\share\a.exe")));
        assert!(is_network_path(Path::new(r"\\?\UNC\server\share\a.exe")));
        assert!(!is_network_path(Path::new(r"C:\Tools\a.exe")));
        assert!(!is_network_path(Path::new(r"\\?\C:\Tools\a.exe")));
    }

    #[test]
    fn modes_are_ordered_by_what_they_allow() {
        assert!(ElevationMode::Standard < ElevationMode::Allowlist);
        assert!(ElevationMode::Allowlist < ElevationMode::AllExecutables);
    }
}
