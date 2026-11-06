use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
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
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
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

use super::Cancellation;
use crate::recycle::Apartment;

const PREFIX: &str = r"\\.\pipe\Firewol-";
const MAX_PAYLOAD: usize = 64 * 1024;

pub(super) const RESET: u32 = 1;
pub(super) const REFERENCES: u32 = 2;
pub(super) const CHECKPOINT: u32 = 3;
pub(super) const FAILED: u32 = 4;

pub(super) struct Pipe(OwnedHandle);

impl Pipe {
    pub fn launch(drive: char, serial: u32, cancel: &Cancellation) -> Result<Self> {
        let _sta = Apartment::sta()?;
        let guid = unsafe { CoCreateGuid()? }.to_u128();
        let name = format!("{PREFIX}{guid:032x}");
        let pipe = Self::listen(&name)?;
        let program = wide(std::env::current_exe()?.as_os_str());
        let args = wide(OsStr::new(&format!(
            "--ntfs-helper {drive} {serial} {name} {}",
            std::process::id()
        )));
        cancel.check()?;
        let mut request = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(program.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if let Err(error) = unsafe { ShellExecuteExW(&mut request) } {
            if error.code() == ERROR_CANCELLED.to_hresult() {
                bail!("Windows elevation was cancelled. Folder search is available.");
            }
            return Err(error).context("Start the read-only NTFS helper");
        }
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
                FILE_FLAGS_AND_ATTRIBUTES(0),
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
