use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, Prefix};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{ERROR_HANDLE_EOF, ERROR_REQUEST_PAUSED, HANDLE};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::SystemServices::FILE_NAMED_STREAMS;
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::core::PCWSTR;

use super::{Stamp, destination, wide};
use crate::explorer::ItemId;
use crate::file_identity::identity;

pub const MAX_TRANSFER_ITEMS: usize = 32;

pub struct TransferRequest {
    pub items: Vec<ItemId>,
    pub into: ItemId,
    pub move_items: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferState {
    #[default]
    Running,
    Paused,
    Cancelled,
}

#[derive(Clone, Default)]
pub struct TransferControl(Arc<Control>);

#[derive(Default)]
struct Control {
    state: Mutex<TransferState>,
    changed: Condvar,
    bytes: AtomicU64,
    finished: AtomicUsize,
    verifying: AtomicUsize,
    restarts: AtomicUsize,
    total_bytes: AtomicU64,
    total_items: AtomicUsize,
}

pub struct TransferProgress {
    pub state: TransferState,
    pub bytes: u64,
    pub finished: usize,
    pub verifying: usize,
    pub paused_copies: usize,
    /// Bytes the planned file copies will write; folders and same-volume moves add none.
    pub total_bytes: u64,
    /// Items the job handles, or 0 when unknown (undo).
    pub total_items: usize,
}

impl TransferControl {
    pub fn progress(&self) -> TransferProgress {
        TransferProgress {
            state: *self.0.state.lock().unwrap(),
            bytes: self.0.bytes.load(Relaxed),
            finished: self.0.finished.load(Relaxed),
            verifying: self.0.verifying.load(Relaxed),
            paused_copies: self.0.restarts.load(Relaxed),
            total_bytes: self.0.total_bytes.load(Relaxed),
            total_items: self.0.total_items.load(Relaxed),
        }
    }

    pub(super) fn plan(&self, items: usize, bytes: u64) {
        self.0.total_items.store(items, Relaxed);
        self.0.total_bytes.store(bytes, Relaxed);
    }

    pub fn toggle_pause(&self) {
        let mut state = self.0.state.lock().unwrap();
        *state = match *state {
            TransferState::Running => TransferState::Paused,
            TransferState::Paused => TransferState::Running,
            TransferState::Cancelled => return,
        };
        self.0.changed.notify_all();
    }

    pub fn cancel(&self) {
        *self.0.state.lock().unwrap() = TransferState::Cancelled;
        self.0.changed.notify_all();
    }

    pub(super) fn checkpoint(&self) -> Result<()> {
        let state = self.0.state.lock().unwrap();
        let state = self
            .0
            .changed
            .wait_while(state, |state| *state == TransferState::Paused)
            .unwrap();
        ensure!(*state != TransferState::Cancelled, "Transfer cancelled");
        Ok(())
    }

    pub(super) fn finish_item(&self) {
        self.0.finished.fetch_add(1, Relaxed);
    }
}

pub(super) fn local(path: &Path) -> bool {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let drive = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
        _ => return false,
    };
    let root = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED }
}

pub(super) fn regular(item: &ItemId, into: &ItemId) -> Result<bool> {
    Ok(local(item.path()) && local(into.path()) && !Stamp::read(&item.verify()?)?.directory)
}

fn locked(item: &ItemId, access: FILE_ACCESS_RIGHTS, share: FILE_SHARE_MODE) -> Result<File> {
    let file = OpenOptions::new()
        .access_mode(access.0)
        .share_mode(share.0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(item.path())
        .with_context(|| format!("Lock {}", item.path().display()))?;
    ensure!(
        identity(&file)? == item.identity,
        "The transfer item was replaced"
    );
    Stamp::read(&file)?;
    Ok(file)
}

fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

struct CopyContext<'a> {
    control: &'a TransferControl,
    bytes: AtomicU64,
}

impl CopyContext<'_> {
    fn update(&self, bytes: u64) {
        let previous = self.bytes.fetch_max(bytes, Relaxed);
        self.control
            .0
            .bytes
            .fetch_add(bytes.saturating_sub(previous), Relaxed);
    }
}

unsafe extern "system" fn progress(
    message: *const COPYFILE2_MESSAGE,
    context: *const c_void,
) -> COPYFILE2_MESSAGE_ACTION {
    let (message, context) = unsafe { (&*message, &*context.cast::<CopyContext<'_>>()) };
    match message.Type {
        COPYFILE2_CALLBACK_CHUNK_FINISHED => {
            context.update(unsafe { message.Info.ChunkFinished.uliTotalBytesTransferred })
        }
        COPYFILE2_CALLBACK_STREAM_FINISHED => {
            context.update(unsafe { message.Info.StreamFinished.uliTotalBytesTransferred })
        }
        _ => {}
    }
    match *context.control.0.state.lock().unwrap() {
        TransferState::Running => COPYFILE2_PROGRESS_CONTINUE,
        TransferState::Paused if message.Type == COPYFILE2_CALLBACK_CHUNK_FINISHED => {
            COPYFILE2_PROGRESS_PAUSE
        }
        TransferState::Paused => COPYFILE2_PROGRESS_CONTINUE,
        TransferState::Cancelled => COPYFILE2_PROGRESS_CANCEL,
    }
}

fn copy_file(source: &Path, target: &Path, control: &TransferControl) -> Result<()> {
    let source = wide(source.as_os_str());
    let target = wide(target.as_os_str());
    let context = CopyContext {
        control,
        bytes: AtomicU64::default(),
    };
    let mut params = COPYFILE2_EXTENDED_PARAMETERS {
        dwSize: size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
        dwCopyFlags: COPY_FILE_FAIL_IF_EXISTS | COPY_FILE_NO_OFFLOAD,
        pProgressRoutine: Some(progress),
        pvCallbackContext: std::ptr::from_ref(&context).cast_mut().cast(),
        ..Default::default()
    };
    loop {
        control.checkpoint()?;
        let result = unsafe {
            CopyFile2(
                PCWSTR(source.as_ptr()),
                PCWSTR(target.as_ptr()),
                Some(&params),
            )
        };
        match result {
            Err(error) if error.code() == ERROR_REQUEST_PAUSED.to_hresult() => {
                control.0.restarts.fetch_add(1, Relaxed);
                params.dwCopyFlags |= COPY_FILE_RESUME_FROM_PAUSE;
            }
            result => {
                control.checkpoint()?;
                return result.context("CopyFile2 transfer");
            }
        }
    }
}

struct StreamSearch(HANDLE);

impl Drop for StreamSearch {
    fn drop(&mut self) {
        let _ = unsafe { FindClose(self.0) };
    }
}

fn named_streams(path: &Path, file: &File) -> Result<BTreeMap<OsString, File>> {
    let mut flags = 0;
    unsafe {
        GetVolumeInformationByHandleW(handle(file), None, None, None, Some(&mut flags), None)?
    };
    let mut streams = BTreeMap::new();
    if flags & FILE_NAMED_STREAMS == 0 {
        return Ok(streams);
    }
    let path_units = wide(path.as_os_str());
    let mut info = WIN32_FIND_STREAM_DATA::default();
    let found = unsafe {
        FindFirstStreamW(
            PCWSTR(path_units.as_ptr()),
            FindStreamInfoStandard,
            std::ptr::from_mut(&mut info).cast(),
            None,
        )
    };
    let search = match found {
        Ok(handle) => StreamSearch(handle),
        Err(error) if error.code() == ERROR_HANDLE_EOF.to_hresult() => return Ok(streams),
        Err(error) => return Err(error).context("Enumerate file data streams"),
    };
    loop {
        let name = OsString::from_wide(info.cStreamName.split(|unit| *unit == 0).next().unwrap());
        if name != "::$DATA" {
            let mut stream_path = path.as_os_str().to_owned();
            stream_path.push(&name);
            let stream = OpenOptions::new()
                .read(true)
                .share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0)
                .open(stream_path)?;
            streams.insert(name, stream);
        }
        match unsafe { FindNextStreamW(search.0, std::ptr::from_mut(&mut info).cast()) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_HANDLE_EOF.to_hresult() => return Ok(streams),
            Err(error) => return Err(error).context("Enumerate file data streams"),
        }
    }
}

fn checksum(mut file: &File, control: &TransferControl, buffer: &mut [u8]) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    loop {
        control.checkpoint()?;
        let length = file.read(buffer)?;
        if length == 0 {
            return Ok(hasher.finalize().into());
        }
        hasher.update(&buffer[..length]);
    }
}

fn verify(
    source: &File,
    staged: &File,
    streams: &BTreeMap<OsString, File>,
    staged_streams: &BTreeMap<OsString, File>,
    control: &TransferControl,
) -> Result<()> {
    ensure!(
        streams.keys().eq(staged_streams.keys()),
        "The copy has different data streams"
    );
    let mut buffer = vec![0; 128 * 1024];
    for (from, to) in
        std::iter::once((source, staged)).chain(streams.values().zip(staged_streams.values()))
    {
        ensure!(
            checksum(from, control, &mut buffer)? == checksum(to, control, &mut buffer)?,
            "SHA-256 verification failed"
        );
    }
    Ok(())
}

fn publish(file: &File, path: &Path) -> Result<()> {
    let name = wide(path.as_os_str());
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let name_bytes = std::mem::size_of_val(name.as_slice());
    let size = (offset + name_bytes).max(size_of::<FILE_RENAME_INFO>());
    // Keep the variable-length Win32 structure aligned, with ReplaceIfExists left false.
    let mut buffer =
        vec![FILE_RENAME_INFO::default(); size.div_ceil(size_of::<FILE_RENAME_INFO>())];
    buffer[0].FileNameLength = (name_bytes - size_of::<u16>()) as u32;
    unsafe {
        std::ptr::copy_nonoverlapping(
            name.as_ptr().cast::<u8>(),
            buffer.as_mut_ptr().cast::<u8>().add(offset),
            name_bytes,
        );
        SetFileInformationByHandle(
            handle(file),
            FileRenameInfo,
            buffer.as_ptr().cast(),
            size as u32,
        )
    }
    .context("Publish the verified file without replacing an existing item")
}

fn remove(file: &File, ignore_read_only: bool) -> windows::core::Result<()> {
    let mut flags = FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0;
    if ignore_read_only {
        flags |= FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE.0;
    }
    let info = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_INFO_EX_FLAGS(flags),
    };
    unsafe {
        SetFileInformationByHandle(
            handle(file),
            FileDispositionInfoEx,
            std::ptr::from_ref(&info).cast(),
            size_of_val(&info) as u32,
        )
    }
}

pub(super) fn place(
    item: &ItemId,
    into: &ItemId,
    name: &OsStr,
    remove_source: bool,
    control: &TransferControl,
) -> Result<(ItemId, Stamp)> {
    let destination = destination(item, into, name)?;
    let _parent = locked(
        into,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )?;
    let source = locked(item, FILE_GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_DELETE)?;
    let stamp = Stamp::read(&source)?;
    ensure!(!stamp.directory, "Select a regular file");
    let streams = named_streams(item.path(), &source)?;
    let stage = tempfile::Builder::new()
        .prefix(".firewol-")
        .tempdir_in(into.path())?;
    let staged_path = stage.path().join("payload");
    // Hold both directories in place until the completed copy is published.
    let stage_id = ItemId::at(stage.path().to_owned())?;
    let _stage = locked(
        &stage_id,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )?;
    let result = (|| {
        copy_file(item.path(), &staged_path, control)?;
        let staged_id = ItemId::at(staged_path.clone())?;
        let staged = locked(&staged_id, FILE_GENERIC_READ | DELETE, FILE_SHARE_READ)?;
        let staged_stamp = Stamp::read(&staged)?;
        let staged_streams = named_streams(&staged_path, &staged)?;
        control.0.verifying.fetch_add(1, Relaxed);
        let verified = verify(&source, &staged, &streams, &staged_streams, control);
        control.0.verifying.fetch_sub(1, Relaxed);
        verified?;
        drop(staged_streams);
        let source_to_remove = if remove_source {
            Some(locked(
                item,
                DELETE | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
            )?)
        } else {
            None
        };
        item.verify()?;
        ensure!(
            Stamp::read(&source)? == stamp,
            "The source changed during the transfer"
        );
        ensure!(
            named_streams(item.path(), &source)?
                .keys()
                .eq(streams.keys()),
            "The source data streams changed"
        );
        into.verify()?;
        control.checkpoint()?;
        publish(&staged, &destination)?;
        let committed = (|| {
            into.verify()?;
            let item = ItemId::at(destination.clone())?;
            ensure!(
                item.identity == staged_id.identity,
                "The published item could not be verified"
            );
            if let Some(source_to_remove) = source_to_remove {
                remove(&source_to_remove, false).context("Remove the original file")?;
            }
            Ok(item)
        })();
        if let Err(error) = &committed {
            remove(&staged, true).with_context(|| {
                format!(
                    "Could not finish the transfer ({error:#}). The verified copy was published at {}",
                    destination.display()
                )
            })?;
        }
        Ok((
            committed.context("The transfer was rolled back")?,
            staged_stamp,
        ))
    })();
    if result.is_err()
        && let Ok(payload) = ItemId::at(staged_path)
    {
        // A paused or read-only staging file may need explicit cleanup.
        if let Ok(file) = locked(&payload, DELETE | FILE_READ_ATTRIBUTES, FILE_SHARE_READ) {
            let _ = remove(&file, true);
        }
    }
    result
}
