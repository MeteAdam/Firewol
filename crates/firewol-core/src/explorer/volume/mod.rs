mod pipe;

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, params};
use windows::Win32::Foundation::{
    ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED, ERROR_JOURNAL_NOT_ACTIVE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdType,
    GetFinalPathNameByHandleW, OpenFileById,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};
use windows::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
    USN_REASON_BASIC_INFO_CHANGE, USN_REASON_DATA_EXTEND, USN_REASON_DATA_TRUNCATION,
    USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE,
    USN_REASON_RENAME_NEW_NAME, USN_REASON_RENAME_OLD_NAME, USN_REASON_REPARSE_POINT_CHANGE,
    USN_REASON_SECURITY_CHANGE,
};

use super::search::{INSERT, IndexedItem, cancellable, rebuild, reset};
use super::{Cancellation, SearchIndex, attributes, encode, resolve_directory};
use crate::file_identity::{FileIdentity, identity, open_item};
use crate::ntfs::{self, Records};
use pipe::{CHECKPOINT, FAILED, Pipe, REFERENCES, RESET, Sender};

pub fn drive_of(path: &Path) -> Option<char> {
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                Some(char::from(drive.to_ascii_uppercase()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The drive letter of `path` when it lies on a local NTFS volume that volume search can index.
pub fn ntfs_drive(path: &Path) -> Option<char> {
    let drive = drive_of(path)?;
    (ntfs::is_local(drive) && ntfs::volume_serial(drive).is_ok()).then_some(drive)
}

impl SearchIndex {
    pub fn watch_volume(
        &self,
        drive: char,
        excluded: &Path,
        cancel: &Cancellation,
        mut updated: impl FnMut(u64),
    ) -> Result<()> {
        ensure!(
            !ntfs::is_elevated()?,
            "Start Firewol with a standard user token before enabling volume search"
        );
        let serial = ntfs::volume_serial(drive)?;
        let root = resolve_directory(Path::new(&format!("{drive}:\\")))?;
        let excluded = excluded.canonicalize()?;
        let volume_hint = open_item(&root)?;
        ensure!(
            identity(&volume_hint)?.volume == serial,
            "The selected NTFS volume changed"
        );
        let mut db = self.writer()?;
        let pipe = Pipe::launch(drive, serial, cancel)?;
        let mut block = Vec::new();
        let mut initial = false;
        let mut writing = false;
        let mut changed = false;
        while !cancel.is_cancelled() {
            match pipe.receive(&mut block, cancel)? {
                RESET if block.is_empty() => {
                    if writing {
                        db.execute_batch("ROLLBACK")?;
                    }
                    db.execute_batch("BEGIN IMMEDIATE")?;
                    reset(&db)?;
                    writing = true;
                    initial = true;
                    changed = true;
                }
                REFERENCES if block.len().is_multiple_of(8) => {
                    if !writing {
                        db.execute_batch("BEGIN IMMEDIATE")?;
                        writing = true;
                    }
                    let mut insert = db.prepare_cached(INSERT)?;
                    let mut delete = db.prepare_cached("DELETE FROM entries WHERE id = ?1")?;
                    for bytes in block.as_chunks::<8>().0 {
                        cancel.check()?;
                        let reference = u64::from_le_bytes(*bytes);
                        let item = resolve_reference(
                            &volume_hint,
                            &root,
                            &excluded,
                            FileIdentity {
                                volume: serial,
                                index: reference,
                            },
                        );
                        if initial {
                            delete.execute([reference as i64])?;
                            if let Some(item) = &item {
                                item.insert(&mut insert, Some(reference as i64))?;
                            }
                        } else {
                            changed |= update_reference(&db, reference as i64, item.as_ref())?;
                        }
                    }
                }
                CHECKPOINT if block.len() == 16 => {
                    if changed {
                        if initial {
                            cancellable(&mut db, cancel, |db| rebuild(db))?;
                        }
                        let journal = i64::from_le_bytes(block[..8].try_into()?);
                        let cursor = i64::from_le_bytes(block[8..].try_into()?);
                        db.execute("DELETE FROM checkpoint", [])?;
                        db.execute(
                            "INSERT INTO checkpoint VALUES (?1, ?2)",
                            params![journal, cursor],
                        )?;
                        db.execute_batch("COMMIT")?;
                        updated(db.query_row("SELECT COUNT(*) FROM entries", [], |row| {
                            row.get::<_, i64>(0)
                        })? as u64);
                    } else if writing {
                        // Cursor-only writes would produce another USN change on the indexed volume.
                        db.execute_batch("ROLLBACK")?;
                    }
                    writing = false;
                    initial = false;
                    changed = false;
                }
                FAILED => bail!("NTFS helper: {}", String::from_utf8_lossy(&block)),
                _ => bail!("Invalid NTFS helper response"),
            }
        }
        Ok(())
    }
}

struct VisibleItem {
    path: PathBuf,
    directory: bool,
    size: u64,
    modified: i64,
    id: FileIdentity,
}

impl VisibleItem {
    fn insert(&self, statement: &mut rusqlite::Statement<'_>, row: Option<i64>) -> Result<()> {
        IndexedItem {
            path: &self.path,
            directory: self.directory,
            size: self.size,
            modified: self.modified,
            file_id: Some(self.id),
        }
        .insert(statement, row)
    }
}

fn resolve_reference(
    hint: &File,
    root: &Path,
    excluded: &Path,
    expected: FileIdentity,
) -> Option<VisibleItem> {
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            FileId: expected.index as i64,
        },
    };
    let handle = unsafe {
        OpenFileById(
            HANDLE(hint.as_raw_handle()),
            &descriptor,
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    }
    .ok()?;
    let file = unsafe { File::from_raw_handle(handle.0) };
    let (directory, reparse, size, modified) = attributes(&file).ok()?;
    if reparse || identity(&file).ok()? != expected {
        return None;
    }
    let path = path_of(handle)?;
    if path == root
        || !path.starts_with(root)
        || path.starts_with(excluded)
        || !visible_parent(&path)
    {
        return None;
    }
    // Reopen by name to verify that this link is reachable without elevated access.
    if identity(&open_item(&path).ok()?).ok()? != expected {
        return None;
    }
    Some(VisibleItem {
        path,
        directory,
        size,
        modified,
        id: expected,
    })
}

fn path_of(handle: HANDLE) -> Option<PathBuf> {
    let mut stack = [0; 512];
    let flags = FILE_NAME_NORMALIZED;
    let length = unsafe { GetFinalPathNameByHandleW(handle, &mut stack, flags) } as usize;
    match length {
        0 => None,
        n if n < stack.len() => Some(OsString::from_wide(&stack[..n]).into()),
        n if n <= 32_768 => {
            let mut wide = vec![0; n + 1];
            let length = unsafe { GetFinalPathNameByHandleW(handle, &mut wide, flags) } as usize;
            (length > 0 && length < wide.len()).then(|| OsString::from_wide(&wide[..length]).into())
        }
        _ => None,
    }
}

pub(super) fn visible_parent(path: &Path) -> bool {
    path.ancestors().skip(1).all(|parent| {
        OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(parent)
            .ok()
            .and_then(|file| attributes(&file).ok())
            .is_some_and(|(directory, reparse, _, _)| directory && !reparse)
    })
}

fn update_reference(db: &Connection, row: i64, item: Option<&VisibleItem>) -> Result<bool> {
    if let Some(item) = item {
        let unchanged: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1 AND path = ?2 AND size = ?3 AND modified = ?4)",
            params![row, encode(item.path.as_os_str()), i64::try_from(item.size)?, item.modified],
            |result| result.get(0),
        )?;
        if unchanged {
            return Ok(false);
        }
    }
    db.prepare_cached("INSERT INTO names(names, rowid, name_key) SELECT 'delete', id, name_key FROM entries WHERE id = ?1")?.execute([row])?;
    let removed = db
        .prepare_cached("DELETE FROM entries WHERE id = ?1")?
        .execute([row])?;
    if let Some(item) = item {
        let mut insert = db.prepare_cached(INSERT)?;
        item.insert(&mut insert, Some(row))?;
        db.prepare_cached(
            "INSERT INTO names(rowid, name_key) SELECT id, name_key FROM entries WHERE id = ?1",
        )?
        .execute([row])?;
    }
    Ok(removed != 0 || item.is_some())
}

pub fn run_helper(drive: char, serial: u32, name: &str, parent: u32) -> Result<()> {
    // The helper runs elevated: nothing it loads may come from a folder a user can write to.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)? };
    let mut pipe = Sender::connect(name, parent)?;
    let result = feed_journal(drive, serial, &mut pipe);
    if let Err(error) = &result {
        let _ = pipe.send(FAILED, format!("{error:#}").as_bytes());
    }
    result
}

fn feed_journal(drive: char, serial: u32, pipe: &mut Sender) -> Result<()> {
    let volume = ntfs::open_volume(drive, serial)?;
    let mut references = Vec::with_capacity(8192);
    let mut buffer = vec![0; 64 * 1024];
    let mut was_active = false;
    while pipe.connected() {
        let wait = Instant::now();
        let before = loop {
            match journal(&volume) {
                Ok(state) => break state,
                Err(error)
                    if was_active
                        && journal_lost(&error)
                        && wait.elapsed() < Duration::from_secs(15) =>
                {
                    if !pipe.connected() {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                Err(error) => return Err(error),
            }
        };
        was_active = true;
        pipe.send(RESET, &[])?;
        let enumerated = ntfs::enumerate(&volume, |records| {
            references.clear();
            for record in records {
                references.extend_from_slice(&record?.reference.to_le_bytes());
            }
            pipe.send(REFERENCES, &references)?;
            Ok(true)
        });
        if enumerated.as_ref().is_err_and(journal_lost) {
            continue;
        }
        enumerated?;
        let mut cursor = before.NextUsn;
        while pipe.connected() {
            let after = match journal(&volume) {
                Ok(state) => state,
                Err(error) if journal_lost(&error) => break,
                Err(error) => return Err(error),
            };
            if after.UsnJournalID != before.UsnJournalID
                || cursor < after.FirstUsn.max(after.LowestValidUsn)
                || cursor > after.NextUsn
            {
                break;
            }
            let Some(next) =
                read_changes(&volume, &after, cursor, &mut buffer, &mut references, pipe)?
            else {
                break;
            };
            cursor = next;
            let mut checkpoint = [0; 16];
            checkpoint[..8].copy_from_slice(&after.UsnJournalID.to_le_bytes());
            checkpoint[8..].copy_from_slice(&cursor.to_le_bytes());
            pipe.send(CHECKPOINT, &checkpoint)?;
            thread::sleep(Duration::from_millis(700));
        }
    }
    Ok(())
}

fn journal_lost(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<windows::core::Error>()
        .is_some_and(|error| {
            [
                ERROR_JOURNAL_ENTRY_DELETED,
                ERROR_JOURNAL_DELETE_IN_PROGRESS,
                ERROR_JOURNAL_NOT_ACTIVE,
            ]
            .iter()
            .any(|code| code.to_hresult() == error.code())
        })
}

fn journal(volume: &OwnedHandle) -> Result<USN_JOURNAL_DATA_V0> {
    let mut journal = USN_JOURNAL_DATA_V0::default();
    let mut bytes = 0;
    unsafe {
        DeviceIoControl(
            HANDLE(volume.as_raw_handle()),
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(std::ptr::from_mut(&mut journal).cast()),
            size_of_val(&journal) as u32,
            Some(&mut bytes),
            None,
        )
    }
    .context("Query the selected volume's active USN journal")?;
    ensure!(
        bytes as usize >= size_of_val(&journal),
        "Truncated USN journal state"
    );
    Ok(journal)
}

fn read_changes(
    volume: &OwnedHandle,
    journal: &USN_JOURNAL_DATA_V0,
    mut cursor: i64,
    buffer: &mut [u8],
    references: &mut Vec<u8>,
    pipe: &mut Sender,
) -> Result<Option<i64>> {
    const SUBTREE: u32 = USN_REASON_RENAME_NEW_NAME
        | USN_REASON_RENAME_OLD_NAME
        | USN_REASON_SECURITY_CHANGE
        | USN_REASON_REPARSE_POINT_CHANGE
        | USN_REASON_FILE_DELETE;
    while cursor < journal.NextUsn {
        let input = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: cursor,
            UsnJournalID: journal.UsnJournalID,
            ReasonMask: SUBTREE
                | USN_REASON_FILE_CREATE
                | USN_REASON_DATA_EXTEND
                | USN_REASON_DATA_TRUNCATION
                | USN_REASON_BASIC_INFO_CHANGE
                | USN_REASON_HARD_LINK_CHANGE,
            ..Default::default()
        };
        let mut bytes = 0;
        let result = unsafe {
            DeviceIoControl(
                HANDLE(volume.as_raw_handle()),
                FSCTL_READ_USN_JOURNAL,
                Some(std::ptr::from_ref(&input).cast()),
                size_of_val(&input) as u32,
                Some(buffer.as_mut_ptr().cast()),
                buffer.len() as u32,
                Some(&mut bytes),
                None,
            )
        };
        if result.as_ref().is_err_and(|error| {
            [
                ERROR_JOURNAL_ENTRY_DELETED,
                ERROR_JOURNAL_DELETE_IN_PROGRESS,
                ERROR_JOURNAL_NOT_ACTIVE,
            ]
            .iter()
            .any(|code| code.to_hresult() == error.code())
        }) {
            return Ok(None);
        }
        result.context("Read NTFS journal changes")?;
        let data = buffer
            .get(..bytes as usize)
            .context("Invalid USN journal length")?;
        let (next, records) = data
            .split_at_checked(8)
            .context("Missing USN journal cursor")?;
        let next = i64::from_le_bytes(next.try_into()?);
        ensure!(next > cursor, "NTFS journal cursor stopped advancing");
        cursor = next;
        references.clear();
        for record in Records(records) {
            let record = record?;
            if record.attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0 && record.reason & SUBTREE != 0 {
                return Ok(None);
            }
            references.extend_from_slice(&record.reference.to_le_bytes());
        }
        if !references.is_empty() {
            pipe.send(REFERENCES, references)?;
        }
    }
    Ok(Some(cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{Filters, SearchQuery, page};
    use std::fs;

    #[test]
    fn ntfs_reference_resolves_and_updates_under_the_callers_token() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let directory = fixture.path().canonicalize()?;
        let drive = drive_of(&directory).context("The fixture needs a local drive")?;
        let root = resolve_directory(Path::new(&format!("{drive}:\\")))?;
        let hint = open_item(&root)?;
        let data = directory.join("data");
        let index = SearchIndex::create(&data)?;
        let excluded = data.canonicalize()?;
        let source = directory.join("initial-火.txt");
        fs::write(&source, b"reference lookup")?;
        let id = identity(&open_item(&source)?)?;
        let resolved = resolve_reference(&hint, &root, &excluded, id)
            .context("Resolve a visible NTFS file by reference")?;
        assert_eq!(resolved.path, source);

        let db = index.writer()?;
        assert!(update_reference(&db, id.index as i64, Some(&resolved))?);
        assert!(!update_reference(&db, id.index as i64, Some(&resolved))?);
        let mut reader = index.reader()?;
        let cancel = Cancellation::default();
        let search = |text: &str| SearchQuery {
            text: text.into(),
            regex: false,
            filters: Filters::default(),
            page: page(0),
        };
        assert_eq!(
            reader.search(&search("initial"), &cancel)?.items[0].path,
            source
        );

        let renamed = directory.join("renamed-火.txt");
        fs::rename(&source, &renamed)?;
        let resolved = resolve_reference(&hint, &root, &excluded, id)
            .context("Resolve the renamed NTFS file")?;
        assert!(update_reference(&db, id.index as i64, Some(&resolved))?);
        assert!(reader.search(&search("initial"), &cancel)?.items.is_empty());
        assert_eq!(
            reader.search(&search("renamed"), &cancel)?.items[0].path,
            renamed
        );
        let hidden = data.join("excluded.txt");
        fs::rename(&renamed, &hidden)?;
        assert!(resolve_reference(&hint, &root, &excluded, id).is_none());
        assert!(update_reference(&db, id.index as i64, None)?);
        assert!(reader.search(&search("renamed"), &cancel)?.items.is_empty());
        Ok(())
    }
}
