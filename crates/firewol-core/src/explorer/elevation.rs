use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_READ,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use super::{ItemId, attributes, display_path};
use crate::file_identity::identity;
use crate::recycle::Apartment;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
            let entry = TrustedExecutable::from(&executable_id(path)?);
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
