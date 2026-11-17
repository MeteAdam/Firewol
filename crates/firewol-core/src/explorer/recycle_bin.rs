//! The Recycle Bin, shown in Firewol instead of handed to Explorer.
//!
//! Windows keeps each deleted item as `X:\$Recycle.Bin\<SID>\$R…` beside a small `$I…` record
//! that holds where it came from, its size and when it was deleted. Reading those records lists
//! the bin of every fixed drive without the Shell; Restore goes through the Recycle Bin's own
//! command so Windows puts the item back exactly as Explorer would.
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::Win32::UI::Shell::{
    SHCNE_UPDATEDIR, SHCNF_PATHW, SHChangeNotify, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
    SHERB_NOSOUND, SHEmptyRecycleBinW,
};
use windows_core::PCWSTR;

use super::access::current_user_sid;
use crate::recycle::{Apartment, bin_item, shell_item, undelete};

/// What a pane opens to show the Recycle Bin; the interface knows the same text.
pub const RECYCLE_BIN: &str = "shell:RecycleBinFolder";
const BIN_CLASS: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";
/// A bin this full is listed in part; the rest stays one Empty away.
const MAX_ITEMS: usize = 10_000;

/// True for every way Windows names the Recycle Bin when it asks a folder program to open it.
pub fn is_recycle_bin(target: &str) -> bool {
    let target = target.trim().trim_end_matches('\\');
    target.eq_ignore_ascii_case(RECYCLE_BIN)
        || target.eq_ignore_ascii_case(BIN_CLASS)
        || target
            .strip_prefix("shell:")
            .is_some_and(|rest| rest.eq_ignore_ascii_case(BIN_CLASS))
}

/// One deleted item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recycled {
    /// The `$R…` item that holds the contents.
    pub path: PathBuf,
    /// Where it was deleted from, including its name.
    pub original: PathBuf,
    pub directory: bool,
    pub size: u64,
    /// Unix seconds.
    pub deleted: i64,
}

impl Recycled {
    pub fn name(&self) -> String {
        self.original.file_name().map_or_else(
            || self.original.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

/// `$Recycle.Bin\<SID>` on each fixed drive that has one.
fn bins() -> Result<Vec<PathBuf>> {
    let sid = current_user_sid()?;
    let mask = unsafe { GetLogicalDrives() };
    let mut found = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = char::from(b'A' + index as u8);
        let root: Vec<u16> = OsStr::new(&format!("{letter}:\\"))
            .encode_wide()
            .chain([0])
            .collect();
        if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
            continue;
        }
        let bin = PathBuf::from(format!(r"{letter}:\$Recycle.Bin\{sid}"));
        if bin.is_dir() {
            found.push(bin);
        }
    }
    Ok(found)
}

/// Seconds between 1601 and 1970, as FILETIME counts them in 100 ns steps.
const FILETIME_UNIX: i64 = 11_644_473_600;

/// Reads a `$I…` record: version 1 holds a fixed 260-character path, version 2 a counted one.
fn read_record(bytes: &[u8]) -> Result<(PathBuf, u64, i64)> {
    ensure!(bytes.len() >= 24, "The record is too short");
    let number = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"));
    let version = number(0);
    let size = number(8);
    let filetime = number(16) as i64;
    let deleted = filetime / 10_000_000 - FILETIME_UNIX;
    let text = match version {
        1 => &bytes[24..bytes.len().min(24 + 520)],
        2 => {
            ensure!(bytes.len() >= 28, "The record is too short");
            let length = u32::from_le_bytes(bytes[24..28].try_into().expect("four bytes")) as usize;
            let end = 28 + length * 2;
            ensure!(bytes.len() >= end, "The record's path is cut short");
            &bytes[28..end]
        }
        other => bail!("Unknown Recycle Bin record version {other}"),
    };
    let units: Vec<u16> = text
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    ensure!(!units.is_empty(), "The record has no path");
    Ok((PathBuf::from(OsString::from_wide(&units)), size, deleted))
}

/// Everything in the bins of this PC's fixed drives, newest first.
pub fn list() -> Result<Vec<Recycled>> {
    let mut items = Vec::new();
    for bin in bins()? {
        let Ok(entries) = fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(suffix) = name.to_str().and_then(|text| text.strip_prefix("$R")) else {
                continue;
            };
            let record = bin.join(format!("$I{suffix}"));
            let Ok((original, size, deleted)) = fs::read(&record)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| read_record(&bytes))
            else {
                continue;
            };
            let directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
            items.push(Recycled {
                path: entry.path(),
                original,
                directory,
                size,
                deleted,
            });
            if items.len() >= MAX_ITEMS {
                break;
            }
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.deleted));
    Ok(items)
}

/// Only an item of this account's Recycle Bin on a fixed drive may be restored or deleted here.
fn check_recycled(path: &Path, sid: &str) -> Result<PathBuf> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    let suffix = name
        .strip_prefix("$R")
        .context("Not an item of the Recycle Bin")?;
    let account = path.parent().context("Not an item of the Recycle Bin")?;
    let bin = account.parent().context("Not an item of the Recycle Bin")?;
    let root = bin.parent().context("Not an item of the Recycle Bin")?;
    ensure!(
        account
            .file_name()
            .is_some_and(|folder| folder.eq_ignore_ascii_case(sid))
            && bin
                .file_name()
                .is_some_and(|folder| folder.eq_ignore_ascii_case("$Recycle.Bin"))
            && path.is_absolute()
            && root.parent().is_none()
            && !suffix.contains(['\\', '/']),
        "Not an item of the Recycle Bin"
    );
    Ok(account.join(format!("$I{suffix}")))
}

/// Puts each item back where it was deleted from, as Explorer's Restore does.
pub fn restore(paths: &[PathBuf]) -> Result<()> {
    let sid = current_user_sid()?;
    let _sta = Apartment::sta()?;
    let bin = bin_item()?;
    for path in paths {
        check_recycled(path, &sid)?;
        let item = shell_item(path)?;
        undelete(&bin, &item).with_context(|| format!("Restore {}", path.display()))?;
    }
    Ok(())
}

/// Deletes each item for good: its contents and its record.
pub fn delete_forever(paths: &[PathBuf]) -> Result<()> {
    let sid = current_user_sid()?;
    for path in paths {
        let record = check_recycled(path, &sid)?;
        let metadata =
            fs::symlink_metadata(path).context("The item is no longer in the Recycle Bin")?;
        if metadata.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
        .with_context(|| format!("Delete {}", path.display()))?;
        let _ = fs::remove_file(&record);
        notify(path.parent());
    }
    Ok(())
}

/// Empties the bins of every drive without Windows' own question; Firewol asks in its window.
pub fn empty() -> Result<()> {
    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
    match unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), flags) } {
        Ok(()) => Ok(()),
        // An empty bin answers with an error that means nothing to do.
        Err(_) if list().is_ok_and(|items| items.is_empty()) => Ok(()),
        Err(error) => Err(error).context("Empty the Recycle Bin"),
    }
}

/// Tells Explorer and the desktop icon that a bin changed.
fn notify(folder: Option<&Path>) {
    let Some(folder) = folder else {
        return;
    };
    let wide: Vec<u16> = folder.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEDIR,
            SHCNF_PATHW,
            Some(wide.as_ptr().cast()),
            None,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_identity::{identity, open_item};

    #[test]
    fn every_name_windows_gives_the_bin_is_known() {
        for name in [
            "shell:RecycleBinFolder",
            "SHELL:recyclebinfolder",
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "::{645ff040-5081-101b-9f08-00aa002f954e}\\",
            "shell:::{645FF040-5081-101B-9F08-00AA002F954E}",
        ] {
            assert!(is_recycle_bin(name), "{name}");
        }
        assert!(!is_recycle_bin("shell:Downloads"));
        assert!(!is_recycle_bin(r"C:\$Recycle.Bin"));
    }

    #[test]
    fn records_of_both_versions_are_read() -> Result<()> {
        let path: Vec<u8> = r"C:\Users\me\note.txt"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let filetime = (1_700_000_000i64 + FILETIME_UNIX) * 10_000_000;
        let mut second = Vec::new();
        second.extend(2u64.to_le_bytes());
        second.extend(42u64.to_le_bytes());
        second.extend(filetime.to_le_bytes());
        second.extend(((path.len() / 2) as u32).to_le_bytes());
        second.extend(&path);
        assert_eq!(
            read_record(&second)?,
            (PathBuf::from(r"C:\Users\me\note.txt"), 42, 1_700_000_000)
        );
        let mut first = Vec::new();
        first.extend(1u64.to_le_bytes());
        first.extend(7u64.to_le_bytes());
        first.extend(filetime.to_le_bytes());
        first.extend(&path);
        first.resize(24 + 520, 0);
        assert_eq!(
            read_record(&first)?.0,
            PathBuf::from(r"C:\Users\me\note.txt")
        );
        assert!(read_record(&first[..10]).is_err());
        Ok(())
    }

    #[test]
    fn only_items_of_this_accounts_bin_are_touched() -> Result<()> {
        let sid = current_user_sid()?;
        let inside = PathBuf::from(format!(r"C:\$Recycle.Bin\{sid}\$RABC123.txt"));
        assert_eq!(
            check_recycled(&inside, &sid)?,
            PathBuf::from(format!(r"C:\$Recycle.Bin\{sid}\$IABC123.txt"))
        );
        for outside in [
            PathBuf::from(r"C:\Users\me\$Rnote.txt"),
            PathBuf::from(format!(r"C:\$Recycle.Bin\{sid}\note.txt")),
            PathBuf::from(r"C:\$Recycle.Bin\S-1-5-21-0-0-0-500\$RABC.txt"),
            PathBuf::from(format!(r"C:\x\$Recycle.Bin\{sid}\$RABC.txt")),
        ] {
            assert!(
                check_recycled(&outside, &sid).is_err(),
                "{}",
                outside.display()
            );
        }
        assert!(delete_forever(&[PathBuf::from(r"C:\Windows\notepad.exe")]).is_err());
        Ok(())
    }

    /// Recycles a file the way Firewol's Delete does, finds it in the list, restores it, then
    /// recycles it again and deletes it for good.
    #[test]
    fn a_recycled_file_is_listed_restored_and_deleted_for_good() -> Result<()> {
        let _sta = Apartment::sta()?;
        let root = tempfile::tempdir()?;
        let file = root
            .path()
            .join(format!("bin-test-{}.txt", std::process::id()));
        fs::write(&file, b"recycled")?;
        let id = identity(&open_item(&file)?)?;
        let find = |file: &Path| -> Result<Option<Recycled>> {
            Ok(list()?.into_iter().find(|item| {
                crate::explorer::display_path(&item.original) == crate::explorer::display_path(file)
            }))
        };

        crate::recycle::recycle(&file, id)?;
        let listed = find(&file)?.context("The recycled file is not listed")?;
        assert_eq!(listed.size, 8);
        assert!(!listed.directory);
        assert_eq!(listed.name(), file.file_name().unwrap().to_string_lossy());
        restore(std::slice::from_ref(&listed.path))?;
        assert_eq!(fs::read(&file)?, b"recycled");
        assert!(find(&file)?.is_none(), "Restore left the item in the bin");

        let id = identity(&open_item(&file)?)?;
        crate::recycle::recycle(&file, id)?;
        let listed = find(&file)?.context("The recycled file is not listed again")?;
        delete_forever(std::slice::from_ref(&listed.path))?;
        assert!(!listed.path.exists() && !file.exists());
        assert!(find(&file)?.is_none(), "Delete left the item in the bin");
        Ok(())
    }
}
