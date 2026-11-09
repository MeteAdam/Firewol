//! Context-menu actions: starting other programs, terminals, file attributes and new items.
//! Every target is re-verified by identity first, and elevation always goes through the policy.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW,
};
use windows_core::HSTRING;

use super::elevation::{
    self, ElevationPolicy, executable_id, system_directory, validate_executable,
};
use super::operations::validate_name;
use super::{ItemId, attributes, display_path, unix_seconds};
use crate::file_identity::information;

const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Terminal {
    CommandPrompt,
    PowerShell,
    WindowsTerminal,
    /// `systeminfo` in a console that stays open.
    SystemInformation,
}

/// The folder a command should run in: a folder itself, or the folder holding a file.
fn working_folder(target: &ItemId) -> Result<PathBuf> {
    let (directory, ..) = attributes(&target.verify()?)?;
    let folder = if directory {
        target.path()
    } else {
        target.path().parent().context("The target has no folder")?
    };
    Ok(PathBuf::from(display_path(folder)))
}

/// Opens a console in the target's folder. The folder goes through the working directory, so no
/// file name ever reaches a command line.
pub fn open_terminal(kind: Terminal, target: &ItemId) -> Result<()> {
    terminal_command(kind)?
        .current_dir(working_folder(target)?)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .context("Open the terminal")?;
    Ok(())
}

fn terminal_command(kind: Terminal) -> Result<Command> {
    Ok(match kind {
        Terminal::CommandPrompt => Command::new("cmd.exe"),
        Terminal::PowerShell => {
            let mut shell = Command::new("powershell.exe");
            shell.arg("-NoExit");
            shell
        }
        Terminal::WindowsTerminal => {
            let mut shell = Command::new("wt.exe");
            shell.args(["-d", "."]);
            shell
        }
        Terminal::SystemInformation => {
            // `cmd` tries the working folder first, so a bare name could run a planted file.
            let system = system_directory()?;
            let mut shell = Command::new(system.join("cmd.exe"));
            shell.arg("/K").arg(system.join("systeminfo.exe"));
            shell
        }
    })
}

/// `wt.exe` is an app execution alias, which only `symlink_metadata` can see.
pub fn windows_terminal_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths)
            .any(|folder| fs::symlink_metadata(folder.join("wt.exe")).is_ok())
    })
}

/// Gives the administrators group the item and its contents, through the elevation policy. The
/// console stays open so the result can be read. Blocks until the UAC prompt is answered.
pub fn take_ownership(target: &ItemId, policy: &ElevationPolicy, owner: HWND) -> Result<()> {
    let (directory, ..) = attributes(&target.verify()?)?;
    let path = display_path(target.path()).to_string_lossy().into_owned();
    // `cmd` expands %NAME% even inside quotes, which could point the command at another item.
    ensure!(
        !path.contains(['%', '"']),
        "Firewol cannot take ownership of a path that contains %"
    );
    let system = std::env::var_os("SystemRoot").context("SystemRoot is unset")?;
    let shell = executable_id(&Path::new(&system).join(r"System32\cmd.exe"))?;
    let (recurse, grant) = if directory {
        ("/r /d y", "/t /c /l /q")
    } else {
        ("", "/c /l")
    };
    let arguments = format!(
        r#"/K takeown /f "{path}" {recurse} && icacls "{path}" /grant *S-1-5-32-544:F {grant}"#
    );
    elevation::launch_command(policy, &shell, &arguments, owner)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribute {
    Hidden,
    System,
    ReadOnly,
    Archive,
}

impl Attribute {
    fn flag(self) -> u32 {
        match self {
            Self::Hidden => FILE_ATTRIBUTE_HIDDEN.0,
            Self::System => FILE_ATTRIBUTE_SYSTEM.0,
            Self::ReadOnly => FILE_ATTRIBUTE_READONLY.0,
            Self::Archive => FILE_ATTRIBUTE_ARCHIVE.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ItemAttributes {
    pub hidden: bool,
    pub system: bool,
    pub read_only: bool,
    pub archive: bool,
    /// Unix seconds.
    pub created: i64,
    pub modified: i64,
    pub accessed: i64,
}

pub fn item_attributes(item: &ItemId) -> Result<ItemAttributes> {
    let info = information(&item.verify()?)?;
    let has = |attribute: Attribute| info.dwFileAttributes & attribute.flag() != 0;
    let seconds = |time: windows::Win32::Foundation::FILETIME| {
        unix_seconds((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime))
    };
    Ok(ItemAttributes {
        hidden: has(Attribute::Hidden),
        system: has(Attribute::System),
        read_only: has(Attribute::ReadOnly),
        archive: has(Attribute::Archive),
        created: seconds(info.ftCreationTime),
        modified: seconds(info.ftLastWriteTime),
        accessed: seconds(info.ftLastAccessTime),
    })
}

/// Turns one attribute on or off and leaves every other one as it was.
pub fn set_item_attribute(item: &ItemId, attribute: Attribute, enabled: bool) -> Result<()> {
    let info = information(&item.verify()?)?;
    let settable = FILE_ATTRIBUTE_READONLY.0
        | FILE_ATTRIBUTE_HIDDEN.0
        | FILE_ATTRIBUTE_SYSTEM.0
        | FILE_ATTRIBUTE_ARCHIVE.0
        | FILE_ATTRIBUTE_TEMPORARY.0
        | FILE_ATTRIBUTE_OFFLINE.0
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED.0;
    let current = info.dwFileAttributes & settable;
    let next = if enabled {
        current | attribute.flag()
    } else {
        current & !attribute.flag()
    };
    let next = if next == 0 {
        FILE_ATTRIBUTE_NORMAL.0
    } else {
        next
    };
    unsafe {
        SetFileAttributesW(
            &HSTRING::from(item.path().as_os_str()),
            FILE_FLAGS_AND_ATTRIBUTES(next),
        )
    }
    .context("Change the item's attributes")
}

fn new_item_path(folder: &ItemId, name: &OsStr) -> Result<PathBuf> {
    validate_name(name)?;
    let (directory, ..) = attributes(&folder.verify()?)?;
    ensure!(directory, "New items need a folder");
    Ok(folder.path().join(name))
}

pub fn create_folder(into: &ItemId, name: &OsStr) -> Result<ItemId> {
    let path = new_item_path(into, name)?;
    fs::create_dir(&path).context("Create the folder")?;
    ItemId::at(path)
}

/// Never replaces an existing file.
pub fn create_file(into: &ItemId, name: &OsStr, contents: &str) -> Result<ItemId> {
    let path = new_item_path(into, name)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .context("Create the file")?;
    file.write_all(contents.as_bytes())?;
    ItemId::at(path)
}

/// Starts `executable` with the target's path as its only argument. A folder target is also the
/// working directory; a file target runs in its parent folder. Administrator actions ask Windows
/// for consent through the elevation policy and block until the UAC prompt is answered.
pub fn run_custom(
    executable: &Path,
    administrator: bool,
    target: &ItemId,
    policy: &ElevationPolicy,
    owner: HWND,
) -> Result<()> {
    let (directory, ..) = attributes(&target.verify()?)?;
    validate_executable(executable)?;
    if administrator {
        return elevation::launch(policy, &executable_id(executable)?, Some(target), owner);
    }
    let folder = if directory {
        target.path()
    } else {
        target.path().parent().context("The target has no folder")?
    };
    Command::new(executable)
        .arg(display_path(target.path()))
        .current_dir(folder)
        .spawn()
        .with_context(|| format!("Launch {}", executable.display()))?;
    Ok(())
}

/// Starts an allowlisted executable elevated. Blocks until the UAC prompt is answered.
pub fn run_as_administrator(program: &ItemId, policy: &ElevationPolicy, owner: HWND) -> Result<()> {
    let (directory, ..) = attributes(&program.verify()?)?;
    anyhow::ensure!(!directory, "Select an executable file");
    elevation::launch(policy, program, None, owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::elevation::ElevationMode;

    #[test]
    fn system_information_never_runs_by_bare_name() -> Result<()> {
        let command = terminal_command(Terminal::SystemInformation)?;
        assert!(Path::new(command.get_program()).is_absolute());
        let program = command.get_args().nth(1).context("The program argument")?;
        assert!(Path::new(program).is_absolute());
        assert!(
            program
                .to_string_lossy()
                .to_lowercase()
                .ends_with(r"\system32\systeminfo.exe")
        );
        Ok(())
    }

    #[test]
    fn attributes_change_one_at_a_time() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let file = fixture.path().join("note.txt");
        std::fs::write(&file, "x")?;
        let item = ItemId::at(file)?;
        assert!(!item_attributes(&item)?.hidden);
        set_item_attribute(&item, Attribute::Hidden, true)?;
        set_item_attribute(&item, Attribute::ReadOnly, true)?;
        let state = item_attributes(&item)?;
        assert!(state.hidden && state.read_only && state.modified > 0);
        set_item_attribute(&item, Attribute::Hidden, false)?;
        set_item_attribute(&item, Attribute::ReadOnly, false)?;
        let state = item_attributes(&item)?;
        assert!(!state.hidden && !state.read_only);
        Ok(())
    }

    #[test]
    fn new_items_never_replace_existing_ones_and_need_valid_names() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let into = ItemId::at(fixture.path().to_owned())?;
        let made = create_file(&into, OsStr::new("a.txt"), "first")?;
        assert_eq!(std::fs::read_to_string(made.path())?, "first");
        assert!(create_file(&into, OsStr::new("a.txt"), "second").is_err());
        assert_eq!(std::fs::read_to_string(made.path())?, "first");
        assert!(create_folder(&into, OsStr::new("b"))?.path().is_dir());
        assert!(create_file(&into, OsStr::new(r"..\escape.txt"), "x").is_err());
        Ok(())
    }

    #[test]
    fn taking_ownership_follows_the_policy_and_refuses_percent_signs() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let plain = ItemId::at(fixture.path().to_owned())?;
        let policy = ElevationPolicy::new(ElevationMode::Standard, &[])?;
        let error = take_ownership(&plain, &policy, HWND::default()).unwrap_err();
        assert!(format!("{error:#}").contains("disabled in Admin settings"));
        let named = fixture.path().join("100%done");
        std::fs::create_dir(&named)?;
        let error = take_ownership(&ItemId::at(named)?, &policy, HWND::default()).unwrap_err();
        assert!(format!("{error:#}").contains("contains %"));
        Ok(())
    }

    #[test]
    fn standard_policy_refuses_elevation_and_bad_executables() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let file = fixture.path().join("note.txt");
        std::fs::write(&file, "x")?;
        let target = ItemId::at(file)?;
        let policy = ElevationPolicy::new(ElevationMode::Standard, &[])?;
        let none = HWND::default();
        assert!(run_custom(Path::new("relative.exe"), false, &target, &policy, none).is_err());
        assert!(
            run_custom(
                &fixture.path().join("tool.bat"),
                false,
                &target,
                &policy,
                none
            )
            .is_err()
        );
        let system = std::env::var_os("SystemRoot")
            .map(|root| Path::new(&root).join("System32\\whoami.exe"));
        if let Some(program) = system.filter(|path| path.exists()) {
            let error = run_custom(&program, true, &target, &policy, none).unwrap_err();
            assert!(format!("{error:#}").contains("disabled in Admin settings"));
            let error = run_as_administrator(&executable_id(&program)?, &policy, none).unwrap_err();
            assert!(format!("{error:#}").contains("disabled in Admin settings"));
        }
        Ok(())
    }
}
