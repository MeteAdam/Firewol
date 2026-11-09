//! Asks Firewol to pick files over a private named pipe, keeping the calling program responsive.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};
use windows_core::{HSTRING, Result, w};

const BUFFER_BYTES: u32 = 64 * 1024;
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
const WAIT_OBJECT_FIRST: u32 = 0;

#[derive(Debug, Default, Clone)]
pub struct PickRequest {
    pub folders: bool,
    pub multi: bool,
    pub title: Option<String>,
    pub folder: Option<String>,
    pub file_name: Option<String>,
    /// Display name and pattern list, such as `Pictures` and `*.png;*.jpg`.
    pub types: Vec<(String, String)>,
    /// One-based, as the Windows dialog counts them.
    pub type_index: u32,
}

/// A value travels on one line, so it can hold neither the separator nor a line break.
fn field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

impl PickRequest {
    pub fn to_wire(&self) -> String {
        let mut text = format!(
            "mode\t{}\nmulti\t{}\ntypeIndex\t{}\n",
            if self.folders { "folder" } else { "open" },
            u8::from(self.multi),
            self.type_index
        );
        for (key, value) in [
            ("title", &self.title),
            ("folder", &self.folder),
            ("name", &self.file_name),
        ] {
            if let Some(value) = value {
                text.push_str(&format!("{key}\t{}\n", field(value)));
            }
        }
        for (name, spec) in &self.types {
            text.push_str(&format!("type\t{}\t{}\n", field(name), field(spec)));
        }
        text.push('\n');
        text
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Outcome {
    Picked(Vec<PathBuf>),
    Cancelled,
    /// Anything else: the caller shows the normal Windows window instead.
    Failed,
}

#[derive(Default)]
struct Reply {
    collecting: bool,
    paths: Vec<PathBuf>,
}

/// Reads one finished line of the reply; returns the outcome once the reply is complete.
fn take_line(line: &str, reply: &mut Reply, ready: &AtomicBool) -> Option<Outcome> {
    match line {
        "ready" if !reply.collecting => ready.store(true, Ordering::Release),
        "cancel" if !reply.collecting => return Some(Outcome::Cancelled),
        "ok" if !reply.collecting => reply.collecting = true,
        "end" if reply.collecting => {
            let paths = std::mem::take(&mut reply.paths);
            let usable = !paths.is_empty() && paths.iter().all(|path| path.exists());
            return Some(if usable {
                Outcome::Picked(paths)
            } else {
                Outcome::Failed
            });
        }
        path if reply.collecting => reply.paths.push(PathBuf::from(path)),
        _ => {}
    }
    None
}

struct Shared {
    /// Firewol has its window on screen.
    ready: AtomicBool,
    outcome: Mutex<Option<Outcome>>,
    /// Holds the pipe while it is open, so cancelling can never touch a closed handle.
    pipe: Mutex<Option<isize>>,
    done: isize,
}

impl Shared {
    fn finish(&self, outcome: Outcome) {
        *self
            .outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(outcome);
        if let Some(pipe) = self
            .pipe
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            unsafe {
                let _ = CloseHandle(HANDLE(pipe as _));
            }
        }
        unsafe {
            let _ = SetEvent(HANDLE(self.done as _));
        }
    }

    fn cancel_io(&self) {
        if let Some(pipe) = *self
            .pipe
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
        {
            unsafe {
                let _ = CancelIoEx(HANDLE(pipe as _), None);
            }
        }
    }
}

/// Runs one overlapped operation to its end, however long the person takes.
fn complete(
    pipe: HANDLE,
    event: HANDLE,
    start: impl FnOnce(*mut OVERLAPPED) -> Result<()>,
) -> Result<u32> {
    let mut request = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    match start(&mut request) {
        Ok(()) => {}
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
        Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(0),
        Err(error) => return Err(error),
    }
    let mut bytes = 0;
    unsafe { GetOverlappedResult(pipe, &request, &mut bytes, true)? };
    Ok(bytes)
}

fn serve(pipe: HANDLE, request: &str, shared: &Shared) -> Result<Outcome> {
    let event = unsafe { CreateEventW(None, true, false, None)? };
    let result = (|| -> Result<Outcome> {
        complete(pipe, event, |overlapped| unsafe {
            ConnectNamedPipe(pipe, Some(overlapped))
        })?;
        let mut unsent = request.as_bytes();
        while !unsent.is_empty() {
            let sent = complete(pipe, event, |overlapped| unsafe {
                WriteFile(pipe, Some(unsent), None, Some(overlapped))
            })? as usize;
            unsent = &unsent[sent.min(unsent.len())..];
        }
        let mut reply = Reply::default();
        let mut pending: Vec<u8> = Vec::new();
        let mut total = 0;
        let mut buffer = vec![0u8; BUFFER_BYTES as usize];
        loop {
            let read = complete(pipe, event, |overlapped| unsafe {
                ReadFile(pipe, Some(&mut buffer), None, Some(overlapped))
            })? as usize;
            if read == 0 {
                return Ok(Outcome::Failed);
            }
            total += read;
            if total > MAX_REPLY_BYTES {
                return Ok(Outcome::Failed);
            }
            pending.extend_from_slice(&buffer[..read]);
            while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let text = String::from_utf8_lossy(&line[..end]);
                if let Some(outcome) =
                    take_line(text.trim_end_matches('\r'), &mut reply, &shared.ready)
                {
                    return Ok(outcome);
                }
            }
        }
    })();
    unsafe {
        let _ = CloseHandle(event);
    }
    result
}

fn create_pipe(name: &str) -> Result<HANDLE> {
    // Only the account that created the pipe may connect to it.
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:P(A;;GA;;;OW)"),
            1,
            &mut descriptor,
            None,
        )?;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let pipe = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER_BYTES,
            BUFFER_BYTES,
            0,
            Some(&attributes),
        )
    };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if pipe.is_invalid() {
        return Err(windows_core::Error::from_thread());
    }
    Ok(pipe)
}

/// Keeps the calling program's windows painting until `done` is set or the time runs out.
fn pump_until(done: HANDLE, limit_ms: u32) -> bool {
    let handles = [done];
    let started = Instant::now();
    loop {
        let spent = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
        if spent >= limit_ms {
            return false;
        }
        let waited = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&handles),
                limit_ms - spent,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if waited.0 == WAIT_OBJECT_FIRST {
            return true;
        }
        if waited.0 != WAIT_OBJECT_FIRST + 1 {
            return false;
        }
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

fn unique_pipe_name() -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    format!(r"\\.\pipe\firewol-pick-{}-{stamp:x}", std::process::id())
}

/// Cancels the worker's pipe operation; it may not have started one yet, so this repeats.
fn stop(shared: &Shared, done: HANDLE) {
    for _ in 0..100 {
        shared.cancel_io();
        if pump_until(done, 20) {
            return;
        }
    }
}

/// Starts Firewol through `launch` and waits for its answer. `Failed` covers every way this can
/// go wrong: no Firewol, no window within `ready_within`, or a Firewol that goes away.
pub fn run_pick(
    request: &PickRequest,
    ready_within: Duration,
    launch: impl FnOnce(&str) -> bool,
) -> Outcome {
    let name = unique_pipe_name();
    let Ok(pipe) = create_pipe(&name) else {
        return Outcome::Failed;
    };
    let Ok(done) = (unsafe { CreateEventW(None, true, false, None) }) else {
        unsafe {
            let _ = CloseHandle(pipe);
        }
        return Outcome::Failed;
    };
    let shared = Arc::new(Shared {
        ready: AtomicBool::new(false),
        outcome: Mutex::new(None),
        pipe: Mutex::new(Some(pipe.0 as isize)),
        done: done.0 as isize,
    });
    let worker = {
        let (shared, wire) = (shared.clone(), request.to_wire());
        let raw = pipe.0 as isize;
        std::thread::spawn(move || {
            let outcome = serve(HANDLE(raw as _), &wire, &shared).unwrap_or(Outcome::Failed);
            shared.finish(outcome);
        })
    };
    if !launch(&name) {
        stop(&shared, done);
    }
    // Until Firewol shows its window the wait is limited; after that it is the person's time.
    let limit = ready_within.as_millis().min(u128::from(u32::MAX - 1)) as u32;
    let mut finished = pump_until(done, limit);
    if !finished && shared.ready.load(Ordering::Acquire) {
        finished = pump_until(done, u32::MAX - 1);
    }
    if !finished {
        stop(&shared, done);
    }
    let _ = worker.join();
    unsafe {
        let _ = CloseHandle(done);
    }
    let outcome = shared
        .outcome
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take();
    if finished {
        outcome.unwrap_or(Outcome::Failed)
    } else {
        Outcome::Failed
    }
}
