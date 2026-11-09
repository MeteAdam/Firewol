use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use windows::Win32::Foundation::{ERROR_NOTIFY_ENUM_DIR, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SECURITY, FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

use super::{Cancellation, decode};

pub(super) struct Watch {
    directory: File,
    event: OwnedHandle,
    request: Box<OVERLAPPED>,
    buffer: Box<[u32; 16_384]>,
    pending: bool,
    recursive: bool,
}

/// How long a folder must stay quiet, and how long a burst may last, before one change is reported.
const QUIET_MS: u32 = 200;
const BURST: Duration = Duration::from_secs(1);

/// Calls `changed` after each burst of changes directly inside `path`, until `cancel` fires.
/// A folder Windows cannot watch (some network shares) simply never reports; a watch that breaks
/// later (the folder was deleted) reports once more, so the caller finds out by reading it.
pub fn watch_directory(path: &Path, cancel: &Cancellation, mut changed: impl FnMut()) {
    let Ok(mut watch) = Watch::open(path, false) else {
        return;
    };
    let mut wait = || -> Result<()> {
        while !cancel.is_cancelled() {
            if !watch.changed(None, QUIET_MS)? {
                continue;
            }
            let burst = Instant::now();
            while burst.elapsed() < BURST
                && !cancel.is_cancelled()
                && watch.changed(None, QUIET_MS)?
            {}
            if !cancel.is_cancelled() {
                changed();
            }
        }
        Ok(())
    };
    let broken = wait().is_err();
    if broken && !cancel.is_cancelled() {
        changed();
    }
}

impl Watch {
    pub fn open(path: &Path, recursive: bool) -> Result<Self> {
        let directory = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY.0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED).0)
            .open(path)?;
        let event = unsafe { CreateEventW(None, true, false, None)? };
        let mut watch = Self {
            directory,
            event: unsafe { OwnedHandle::from_raw_handle(event.0) },
            request: Box::new(OVERLAPPED {
                hEvent: event,
                ..Default::default()
            }),
            buffer: Box::new([0; 16_384]),
            pending: false,
            recursive,
        };
        watch.arm()?;
        Ok(watch)
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.directory.as_raw_handle())
    }

    fn arm(&mut self) -> Result<()> {
        unsafe {
            ResetEvent(HANDLE(self.event.as_raw_handle()))?;
            ReadDirectoryChangesW(
                self.handle(),
                self.buffer.as_mut_ptr().cast(),
                size_of_val(self.buffer.as_ref()) as u32,
                self.recursive,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_SECURITY,
                None,
                Some(self.request.as_mut()),
                None,
            )?;
        }
        self.pending = true;
        Ok(())
    }

    pub fn changed(&mut self, excluded: Option<&Path>, timeout_ms: u32) -> Result<bool> {
        let wait = unsafe { WaitForSingleObject(HANDLE(self.event.as_raw_handle()), timeout_ms) };
        if wait == WAIT_TIMEOUT {
            return Ok(false);
        }
        ensure!(wait == WAIT_OBJECT_0, "Wait for folder changes failed");
        let mut bytes = 0;
        let result =
            unsafe { GetOverlappedResult(self.handle(), self.request.as_ref(), &mut bytes, false) };
        self.pending = false;
        let changed = match result {
            Err(error) if error.code() == ERROR_NOTIFY_ENUM_DIR.to_hresult() => true,
            Err(error) => return Err(error).context("Read folder change notifications"),
            Ok(()) if bytes == 0 => true,
            Ok(()) => {
                ensure!(
                    bytes as usize <= size_of_val(self.buffer.as_ref()),
                    "Invalid directory change size"
                );
                let data = unsafe {
                    std::slice::from_raw_parts(self.buffer.as_ptr().cast(), bytes as usize)
                };
                includes_change(data, excluded)?
            }
        };
        self.arm()?;
        Ok(changed)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if self.pending {
            // The kernel must release the request and buffer before either allocation is dropped.
            unsafe {
                let _ = CancelIoEx(self.handle(), Some(self.request.as_ref()));
                let _ = GetOverlappedResult(self.handle(), self.request.as_ref(), &mut 0, true);
            }
        }
    }
}

fn includes_change(mut records: &[u8], excluded: Option<&Path>) -> Result<bool> {
    let Some(excluded) = excluded else {
        return Ok(true);
    };
    loop {
        let header = records.get(..12).context("Truncated folder notification")?;
        let next = u32::from_le_bytes(header[..4].try_into()?) as usize;
        let length = u32::from_le_bytes(header[8..12].try_into()?) as usize;
        let name = decode(
            records
                .get(12..12 + length)
                .context("Truncated notification name")?,
        );
        if !Path::new(&name).starts_with(excluded) {
            return Ok(true);
        }
        if next == 0 {
            return Ok(false);
        }
        ensure!(next >= 12 + length, "Invalid folder notification offset");
        records = records
            .get(next..)
            .context("Invalid folder notification offset")?;
    }
}
