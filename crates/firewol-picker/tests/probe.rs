#![cfg(windows)]

use std::ffi::c_void;
use std::ptr::null_mut;

use firewol_picker::DllGetClassObject;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, IClassFactory};
use windows::Win32::UI::Shell::{FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog};
use windows_core::{GUID, Interface};

fn factory(class: &GUID) -> windows_core::Result<IClassFactory> {
    let mut object: *mut c_void = null_mut();
    unsafe { DllGetClassObject(class, &IClassFactory::IID, &mut object) }.ok()?;
    Ok(unsafe { IClassFactory::from_raw(object) })
}

#[test]
fn hands_back_the_real_windows_dialogs_and_notes_the_program() -> anyhow_free::Result {
    let fixture = tempfile::tempdir()?;
    std::fs::create_dir(fixture.path().join("Firewol"))?;
    // This test is the only one in its process, so changing the environment is safe.
    unsafe { std::env::set_var("LOCALAPPDATA", fixture.path()) };
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;

    let open: IFileOpenDialog = unsafe { factory(&FileOpenDialog)?.CreateInstance(None)? };
    let save: IFileSaveDialog = unsafe { factory(&FileSaveDialog)?.CreateInstance(None)? };
    // A real dialog answers calls; a stub would not know its options.
    assert!(unsafe { open.GetOptions() }.is_ok());
    assert!(unsafe { save.GetOptions() }.is_ok());

    let log = std::fs::read_to_string(fixture.path().join(r"Firewol\file-dialog-probe.log"))?;
    let lines: Vec<_> = log.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("open\t") && lines[0].contains("probe"));
    assert!(lines[1].starts_with("save\t"));

    let other = GUID::from_u128(1);
    let mut object: *mut c_void = null_mut();
    let refused = unsafe { DllGetClassObject(&other, &IClassFactory::IID, &mut object) };
    assert!(refused.is_err() && object.is_null());
    Ok(())
}

mod anyhow_free {
    pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
}
