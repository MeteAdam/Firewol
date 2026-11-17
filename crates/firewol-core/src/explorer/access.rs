//! Explorer's "You don't currently have permission to access this folder" and its Continue.
//!
//! Continue asks for administrator consent once, then gives the signed-in account a lasting
//! read-and-list entry on the folder, as Explorer does. The change itself runs in an elevated
//! copy of Firewol (`--grant-access SID PATH`) that opens no window.
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, HANDLE, HLOCAL, HWND, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, GetDriveTypeW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows_core::{HSTRING, PCWSTR, PWSTR, w};

use super::display_path;
use super::elevation::system_directory;
use super::volume::pinned_self;
use crate::recycle::Apartment;

/// True when listing `path` fails only because the account may not.
pub fn is_access_denied(path: &Path) -> bool {
    matches!(fs::read_dir(path), Err(error) if error.raw_os_error() == Some(5))
}

/// The signed-in account, as text such as `S-1-5-21-…`. The elevated helper may run under
/// another administrator account, so it is told whom to let in.
pub fn current_user_sid() -> Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let token = HANDLE(token.as_raw_handle());
    let mut length = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut length) };
    let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        )?
    };
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text)? };
    let sid = unsafe { text.to_string() };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
    }
    Ok(sid?)
}

/// Only a real folder on a fixed drive of this PC, reached without a link, may be changed.
fn check_folder(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "Choose a folder on this PC");
    let shown = display_path(path);
    let text = shown.to_string_lossy();
    ensure!(
        text.as_bytes().get(1) == Some(&b':') && text.as_bytes().get(2) == Some(&b'\\'),
        "Only a folder on a drive of this PC can be opened this way"
    );
    let root: Vec<u16> = OsStr::new(&text[..3]).encode_wide().chain([0]).collect();
    ensure!(
        unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == DRIVE_FIXED,
        "Only a folder on a fixed drive of this PC can be opened this way"
    );
    let metadata = fs::symlink_metadata(path).context("Read the folder")?;
    ensure!(metadata.is_dir(), "This is not a folder");
    ensure!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
        "This folder is a link; open the folder it points to instead"
    );
    Ok(())
}

/// Adds a lasting entry that lets `sid` read and list `path` and everything below it. This is
/// what Explorer's Continue does; the person agreed to it through Windows' consent prompt.
pub fn grant_read_access(path: &Path, sid: &str) -> Result<()> {
    check_folder(path)?;
    ensure!(
        sid.starts_with("S-1-") && sid.len() < 200,
        "Not an account identifier"
    );
    let wide = |text: &OsStr| text.encode_wide().chain([0]).collect::<Vec<u16>>();
    let target = wide(display_path(path).as_os_str());
    let mut account = PSID::default();
    unsafe { ConvertStringSidToSidW(&HSTRING::from(sid), &mut account)? };
    let result = (|| -> Result<()> {
        let mut current: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            GetNamedSecurityInfoW(
                PCWSTR(target.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut current),
                None,
                &mut descriptor,
            )
            .ok()
            .context("Read the folder's permissions")?;
        }
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_GENERIC_READ.0 | FILE_GENERIC_EXECUTE.0,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: PWSTR(account.0.cast()),
                ..Default::default()
            },
        };
        let mut updated: *mut ACL = std::ptr::null_mut();
        let merged = unsafe { SetEntriesInAclW(Some(&[entry]), Some(current), &mut updated) };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
        merged
            .ok()
            .context("Add the account to the folder's permissions")?;
        let written = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(target.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(updated),
                None,
            )
        };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(updated.cast())));
        }
        written.ok().context("Save the folder's permissions")
    })();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(account.0)));
    }
    result
}

/// The elevated side: `--grant-access SID PATH`. Returns the process exit code.
pub fn run_grant_helper(sid: &str, path: &Path) -> u32 {
    match grant_read_access(path, sid) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

/// Asks Windows for administrator consent and lets the signed-in account into `path`. Checked
/// again afterwards: an explicit deny entry still wins over the new one.
pub fn request_access(path: &Path, owner: HWND) -> Result<()> {
    check_folder(path)?;
    let sid = current_user_sid()?;
    let _sta = Apartment::sta()?;
    let (executable, pinned) = pinned_self()?;
    let wide = |text: &OsStr| text.encode_wide().chain([0]).collect::<Vec<u16>>();
    let program = wide(executable.as_os_str());
    let shown = display_path(path);
    let text = shown.to_string_lossy();
    ensure!(!text.contains('"'), "This folder name cannot be passed on");
    // A trailing backslash would escape the closing quote.
    let quoted = format!("\"{}\"", text.trim_end_matches('\\'));
    let arguments = wide(OsStr::new(&format!("--grant-access {sid} {quoted}")));
    let directory = wide(system_directory()?.as_os_str());
    let mut request = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        hwnd: owner,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(program.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        lpDirectory: PCWSTR(directory.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(error) = unsafe { ShellExecuteExW(&mut request) } {
        if error.code() == ERROR_CANCELLED.to_hresult() {
            bail!("Windows elevation was cancelled");
        }
        return Err(error).context("Request Windows administrator consent");
    }
    drop(pinned);
    ensure!(
        !request.hProcess.is_invalid(),
        "The permission helper did not start"
    );
    let process = request.hProcess;
    let code = unsafe {
        let waited = WaitForSingleObject(process, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        if waited == WAIT_OBJECT_0 { code } else { 1 }
    };
    ensure!(code == 0, "Windows did not change the folder's permissions");
    ensure!(
        !is_access_denied(path),
        "Access is still denied: a rule on this folder blocks your account"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A folder the test owns: its owner may always change who else gets in, without consent.
    #[test]
    fn continue_lets_the_account_list_a_folder_it_could_not() -> Result<()> {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("closed");
        fs::create_dir(&folder)?;
        fs::write(folder.join("inside.txt"), b"x")?;
        let sid = current_user_sid()?;
        // No inherited entries and none for this account: listing is refused.
        let closed = Command::new("icacls")
            .arg(&folder)
            .args(["/inheritance:r", "/grant:r", "*S-1-5-32-544:(OI)(CI)F"])
            .output()?;
        ensure!(closed.status.success(), "Close the test folder");
        let restore = || {
            let _ = Command::new("icacls")
                .arg(&folder)
                .args(["/reset", "/T"])
                .output();
        };
        let result = (|| -> Result<()> {
            ensure!(is_access_denied(&folder), "The test folder is still open");
            grant_read_access(&folder, &sid)?;
            ensure!(
                !is_access_denied(&folder),
                "Continue did not let the account in"
            );
            ensure!(
                fs::read_dir(&folder)?.count() == 1,
                "The folder lists its file"
            );
            Ok(())
        })();
        restore();
        result
    }

    #[test]
    fn only_local_folders_are_changed() {
        assert!(check_folder(Path::new(r"\\server\share\folder")).is_err());
        assert!(check_folder(Path::new("relative")).is_err());
        assert!(grant_read_access(Path::new(r"C:\Windows"), "not-a-sid").is_err());
    }
}
