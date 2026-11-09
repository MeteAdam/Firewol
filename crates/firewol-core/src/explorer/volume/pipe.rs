use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, GENERIC_READ,
    GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_SHARE_MODE, FILE_SHARE_READ, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Com::CoCreateGuid;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PeekNamedPipe,
};
use windows::Win32::System::Threading::{GetProcessId, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, w};

use super::super::attributes;
use super::super::elevation::system_directory;
use super::Cancellation;
use crate::file_identity::{FileIdentity, identity, open_item};
use crate::recycle::Apartment;

const PREFIX: &str = r"\\.\pipe\Firewol-";
const MAX_PAYLOAD: usize = 64 * 1024;

pub(super) const RESET: u32 = 1;
pub(super) const REFERENCES: u32 = 2;
pub(super) const CHECKPOINT: u32 = 3;
pub(super) const FAILED: u32 = 4;

static EXECUTABLE: OnceLock<Option<FileIdentity>> = OnceLock::new();

/// Records which file this process started from, so a file dropped at the same path later is
/// never the one elevated. Call it first thing in `main`.
pub fn remember_executable() {
    EXECUTABLE.get_or_init(running_executable);
}

fn running_executable() -> Option<FileIdentity> {
    let file = open_item(&std::env::current_exe().ok()?).ok()?;
    identity(&file).ok()
}

/// Opens the program about to be elevated and blocks replacing it while the handle lives.
fn pin_executable(path: &Path, expected: Option<FileIdentity>) -> Result<File> {
    let file = OpenOptions::new()
        .access_mode(FILE_GENERIC_READ.0)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .context("Open Firewol's own executable")?;
    let (directory, reparse, _, _) = attributes(&file)?;
    ensure!(
        !directory && !reparse && expected == Some(identity(&file)?),
        "Firewol's executable changed since it started. Restart Firewol and try again."
    );
    Ok(file)
}

pub(super) struct Pipe(OwnedHandle);

impl Pipe {
    pub fn launch(drive: char, serial: u32, cancel: &Cancellation) -> Result<Self> {
        let _sta = Apartment::sta()?;
        let guid = unsafe { CoCreateGuid()? }.to_u128();
        let name = format!("{PREFIX}{guid:032x}");
        let pipe = Self::listen(&name)?;
        let executable = std::env::current_exe()?;
        let pinned = pin_executable(&executable, *EXECUTABLE.get_or_init(running_executable))?;
        let program = wide(executable.as_os_str());
        let args = wide(OsStr::new(&format!(
            "--ntfs-helper {drive} {serial} {name} {}",
            std::process::id()
        )));
        let directory = wide(system_directory()?.as_os_str());
        cancel.check()?;
        let mut request = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(program.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            lpDirectory: PCWSTR(directory.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if let Err(error) = unsafe { ShellExecuteExW(&mut request) } {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                bail!("Windows elevation was cancelled. Folder search is available.");
            }
            return Err(error).context("Start the read-only NTFS helper");
        }
        drop(pinned);
        ensure!(
            !request.hProcess.is_invalid(),
            "NTFS helper process was unavailable"
        );
        let process = unsafe { OwnedHandle::from_raw_handle(request.hProcess.0) };
        let process = HANDLE(process.as_raw_handle());
        loop {
            cancel.check()?;
            match unsafe { ConnectNamedPipe(pipe.handle(), None) } {
                Ok(()) => break,
                Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => break,
                Err(error) if error.code() == ERROR_PIPE_LISTENING.to_hresult() => (),
                Err(error) => return Err(error).context("Connect to the NTFS helper"),
            }
            ensure!(
                unsafe { WaitForSingleObject(process, 30) } == WAIT_TIMEOUT,
                "NTFS helper exited before connecting"
            );
        }
        let mut peer = 0;
        unsafe { GetNamedPipeClientProcessId(pipe.handle(), &mut peer)? };
        ensure!(
            peer != 0 && peer == unsafe { GetProcessId(process) },
            "Unexpected NTFS pipe client"
        );
        Ok(pipe)
    }

    fn listen(name: &str) -> Result<Self> {
        let name = wide(OsStr::new(name));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // The pipe owner and administrators can connect. Remote clients are rejected below.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P(A;;GA;;;OW)(A;;GA;;;BA)"),
                1,
                &mut descriptor,
                None,
            )?
        };
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            ..Default::default()
        };
        let handle = unsafe {
            let handle = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                MAX_PAYLOAD as u32,
                0,
                Some(&security),
            );
            let error = (handle == INVALID_HANDLE_VALUE).then(windows::core::Error::from_thread);
            LocalFree(Some(HLOCAL(descriptor.0)));
            if let Some(error) = error {
                return Err(error).context("Create the private NTFS pipe");
            }
            handle
        };
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle.0) }))
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }

    pub fn receive(&self, data: &mut Vec<u8>, cancel: &Cancellation) -> Result<u32> {
        let mut header = [0; 8];
        self.read(&mut header, cancel)?;
        let kind = u32::from_le_bytes(header[..4].try_into()?);
        let length = u32::from_le_bytes(header[4..].try_into()?) as usize;
        ensure!(length <= MAX_PAYLOAD, "Invalid NTFS response length");
        data.resize(length, 0);
        self.read(data, cancel)?;
        Ok(kind)
    }

    fn read(&self, mut data: &mut [u8], cancel: &Cancellation) -> Result<()> {
        while !data.is_empty() {
            cancel.check()?;
            let mut bytes = 0;
            let result = unsafe { ReadFile(self.handle(), Some(data), Some(&mut bytes), None) };
            match result {
                Ok(()) if bytes > 0 => data = &mut data[bytes as usize..],
                Ok(()) => bail!("The NTFS helper disconnected"),
                Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => {
                    thread::sleep(Duration::from_millis(12))
                }
                Err(error) => return Err(error).context("Read from the NTFS helper"),
            }
        }
        Ok(())
    }
}

pub(super) struct Sender(File);

impl Sender {
    pub fn connect(name: &str, parent: u32) -> Result<Self> {
        ensure!(
            name.strip_prefix(PREFIX).is_some_and(
                |name| name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
            ),
            "Invalid NTFS pipe name"
        );
        let name = wide(OsStr::new(name));
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                // The elevated helper must never lend its token to whoever owns the pipe.
                SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
                None,
            )?
        };
        let file = unsafe { File::from_raw_handle(handle.0) };
        let mut peer = 0;
        unsafe { GetNamedPipeServerProcessId(handle, &mut peer)? };
        ensure!(peer != 0 && peer == parent, "Unexpected NTFS pipe server");
        Ok(Self(file))
    }

    pub fn connected(&self) -> bool {
        unsafe { PeekNamedPipe(HANDLE(self.0.as_raw_handle()), None, 0, None, None, None) }.is_ok()
    }

    pub fn send(&mut self, kind: u32, payload: &[u8]) -> Result<()> {
        ensure!(payload.len() <= MAX_PAYLOAD, "Invalid NTFS payload size");
        let mut header = [0; 8];
        header[..4].copy_from_slice(&kind.to_le_bytes());
        header[4..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.0.write_all(&header)?;
        self.0.write_all(payload)?;
        Ok(())
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_program_can_be_pinned() -> Result<()> {
        let running = std::env::current_exe()?;
        drop(pin_executable(&running, running_executable())?);
        Ok(())
    }

    #[test]
    fn a_replaced_program_is_never_elevated_and_a_pinned_one_cannot_be_moved() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let program = fixture.path().join("firewol.exe");
        std::fs::write(&program, "first")?;
        let original = identity(&open_item(&program)?)?;
        let pinned = pin_executable(&program, Some(original))?;
        assert!(std::fs::rename(&program, fixture.path().join("moved.exe")).is_err());
        drop(pinned);
        std::fs::rename(&program, fixture.path().join("moved.exe"))?;
        std::fs::write(&program, "planted")?;
        let error = pin_executable(&program, Some(original)).unwrap_err();
        assert!(format!("{error:#}").contains("changed since it started"));
        assert!(pin_executable(&program, None).is_err());
        Ok(())
    }

    #[test]
    fn the_helper_connects_without_lending_its_token() -> Result<()> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let name = format!("{PREFIX}{stamp:032x}");
        let _server = Pipe::listen(&name)?;
        let sender = Sender::connect(&name, std::process::id())?;
        assert!(sender.connected());
        assert!(Sender::connect(&name, std::process::id() + 1).is_err());
        Ok(())
    }
}
