use std::ffi::OsStr;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, Prefix};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Foundation::{
    ERROR_CANCELLED, ERROR_HANDLE_EOF, GENERIC_READ, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetVolumeInformationW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{FSCTL_ENUM_USN_DATA, MFT_ENUM_DATA_V0};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, w};

use crate::file_identity::identity;

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

/// Probe a disposable file the normal user can open. The helper reports only
/// whether its exact file reference exists on the NTFS volume.
pub fn probe(root: &Path) -> Result<()> {
    let stage = tempfile::tempdir_in(root)?;
    let file = File::create_new(stage.path().join("usn-probe.txt"))?;
    let id = identity(&file)?;
    let absolute = stage.path().canonicalize()?;
    let drive = match absolute.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => char::from(letter),
            _ => bail!("NTFS probe requires a drive letter"),
        },
        _ => bail!("NTFS probe requires an absolute path"),
    };
    let executable = std::env::current_exe()?;
    let program = wide(executable.as_os_str());
    let args = wide(OsStr::new(&format!(
        "ntfs-helper {drive} {} {}",
        id.volume, id.index
    )));
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
            bail!("Windows elevation was cancelled");
        }
        return Err(error).context("Start read-only NTFS helper");
    }
    ensure!(
        !request.hProcess.is_invalid(),
        "NTFS helper process handle was unavailable"
    );
    let process = unsafe { OwnedHandle::from_raw_handle(request.hProcess.0) };
    let handle = HANDLE(process.as_raw_handle());
    ensure!(
        unsafe { WaitForSingleObject(handle, 30_000) } == WAIT_OBJECT_0,
        "NTFS helper did not exit within 30 seconds"
    );
    let mut code = 0;
    unsafe { GetExitCodeProcess(handle, &mut code)? };
    ensure!(code == 0, "NTFS helper failed with exit code {code}");
    Ok(())
}

pub(crate) fn is_elevated() -> Result<bool> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut bytes = 0;
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast()),
            size_of_val(&elevation) as u32,
            &mut bytes,
        )?;
    }
    Ok(elevation.TokenIsElevated != 0)
}

pub(crate) fn volume_serial(drive: char) -> Result<u32> {
    ensure!(drive.is_ascii_alphabetic(), "Invalid drive letter");
    let root = wide(OsStr::new(&format!("{drive}:\\")));
    let mut filesystem = [0; 16];
    let mut serial = 0;
    unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            None,
            Some(&mut serial),
            None,
            None,
            Some(&mut filesystem),
        )?;
    }
    ensure!(
        filesystem.starts_with(&['N' as u16, 'T' as u16, 'F' as u16, 'S' as u16, 0]),
        "The selected volume must use NTFS"
    );
    Ok(serial)
}

pub(crate) fn open_volume(drive: char, expected_volume: u32) -> Result<OwnedHandle> {
    ensure!(
        is_elevated()?,
        "The NTFS helper requires an elevated Windows token"
    );
    ensure!(
        volume_serial(drive)? == expected_volume,
        "The selected NTFS volume changed"
    );
    let device = wide(OsStr::new(&format!(r"\\.\{drive}:")));
    let handle = unsafe {
        CreateFileW(
            PCWSTR(device.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .context("Open NTFS volume for read-only access")?;
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

pub fn find_reference(drive: char, expected_volume: u32, index: u64) -> Result<()> {
    let volume = open_volume(drive, expected_volume)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut found = false;
    enumerate(&volume, |records| {
        ensure!(
            Instant::now() < deadline,
            "NTFS enumeration exceeded the probe time budget"
        );
        for record in records {
            if record?.reference == index {
                found = true;
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    ensure!(
        found,
        "The fixture file reference was absent from the NTFS enumeration"
    );
    Ok(())
}

pub(crate) fn enumerate(
    volume: &OwnedHandle,
    mut receive: impl FnMut(Records<'_>) -> Result<bool>,
) -> Result<()> {
    let mut input = MFT_ENUM_DATA_V0 {
        StartFileReferenceNumber: 0,
        LowUsn: 0,
        HighUsn: i64::MAX,
    };
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let mut bytes = 0;
        let result = unsafe {
            DeviceIoControl(
                HANDLE(volume.as_raw_handle()),
                FSCTL_ENUM_USN_DATA,
                Some(std::ptr::from_ref(&input).cast()),
                size_of_val(&input) as u32,
                Some(buffer.as_mut_ptr().cast()),
                buffer.len() as u32,
                Some(&mut bytes),
                None,
            )
        };
        match result {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_HANDLE_EOF.to_hresult() => {
                return Ok(());
            }
            Err(error) => return Err(error).context("FSCTL_ENUM_USN_DATA"),
        }
        let block = buffer
            .get(..bytes as usize)
            .context("Invalid NTFS output size")?;
        let (cursor, records) = block.split_at_checked(8).context("Truncated NTFS cursor")?;
        if !receive(Records(records))? {
            return Ok(());
        }
        let next = u64::from_le_bytes(cursor.try_into()?);
        ensure!(
            next > input.StartFileReferenceNumber,
            "NTFS enumeration stopped advancing"
        );
        input.StartFileReferenceNumber = next;
    }
}

pub(crate) struct Records<'a>(pub &'a [u8]);

pub(crate) struct Record {
    pub reference: u64,
    pub reason: u32,
    pub attributes: u32,
}

impl Iterator for Records<'_> {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let result = parse_record(self.0);
        match result {
            Ok((record, remaining)) => {
                self.0 = remaining;
                Some(Ok(record))
            }
            Err(error) => {
                self.0 = &[];
                Some(Err(error))
            }
        }
    }
}

fn parse_record(records: &[u8]) -> Result<(Record, &[u8])> {
    let header = records.get(..60).context("Truncated USN record header")?;
    let length = u32::from_le_bytes(header[..4].try_into()?) as usize;
    let major = u16::from_le_bytes(header[4..6].try_into()?);
    ensure!(
        major == 2 && length >= 60 && length <= records.len() && length.is_multiple_of(8),
        "Invalid USN v2 record"
    );
    Ok((
        Record {
            reference: u64::from_le_bytes(header[8..16].try_into()?),
            reason: u32::from_le_bytes(header[40..44].try_into()?),
            attributes: u32::from_le_bytes(header[52..56].try_into()?),
        },
        &records[length..],
    ))
}
