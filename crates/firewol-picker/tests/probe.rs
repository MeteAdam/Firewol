#![cfg(windows)]

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use firewol_picker::transport::{Outcome, PickRequest, run_pick};
use firewol_picker::{DllGetClassObject, set_launcher_for_tests};
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, IClassFactory};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
    SIGDN_FILESYSPATH,
};
use windows_core::{GUID, Interface, w};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn factory(class: &GUID) -> windows_core::Result<IClassFactory> {
    let mut object: *mut c_void = null_mut();
    unsafe { DllGetClassObject(class, &IClassFactory::IID, &mut object) }.ok()?;
    Ok(unsafe { IClassFactory::from_raw(object) })
}

/// Plays Firewol: connects, reads the request up to its blank line, then sends `reply`.
fn fake_firewol(pipe: &str, reply: String) -> std::thread::JoinHandle<String> {
    let pipe = pipe.to_owned();
    std::thread::spawn(move || {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&pipe)
            .expect("connect");
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\n\n") {
            file.read_exact(&mut byte).expect("request");
            request.push(byte[0]);
        }
        file.write_all(reply.as_bytes()).expect("reply");
        String::from_utf8(request).expect("text")
    })
}

static REPLY: OnceLock<Mutex<String>> = OnceLock::new();
static SEEN: OnceLock<Mutex<Option<std::thread::JoinHandle<String>>>> = OnceLock::new();

fn test_launcher(pipe: &str) -> bool {
    let reply = REPLY.get().expect("reply").lock().expect("lock").clone();
    *SEEN.get().expect("seen").lock().expect("lock") = Some(fake_firewol(pipe, reply));
    true
}

fn set_reply(text: String) {
    *REPLY.get_or_init(Mutex::default).lock().expect("lock") = text;
}

/// COM objects stay on this one thread, as in a program's interface thread.
#[test]
fn the_real_dialog_object_answers_with_what_firewol_picked() -> TestResult {
    SEEN.get_or_init(Mutex::default);
    set_launcher_for_tests(test_launcher);
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    let fixture = tempfile::tempdir()?;
    let first = fixture.path().join("first.txt");
    let second = fixture.path().join("second.txt");
    std::fs::write(&first, b"1")?;
    std::fs::write(&second, b"2")?;

    let dialog: IFileOpenDialog = unsafe { factory(&FileOpenDialog)?.CreateInstance(None)? };
    // Everything not replaced is still Windows' own code on Windows' own object.
    let options = unsafe { dialog.GetOptions() }?;
    let types = [COMDLG_FILTERSPEC {
        pszName: w!("Text"),
        pszSpec: w!("*.txt"),
    }];
    unsafe {
        dialog.SetOptions(options)?;
        dialog.SetFileTypes(&types)?;
        dialog.SetFileTypeIndex(1)?;
        dialog.SetTitle(w!("Choose a note"))?;
    }

    set_reply(format!(
        "ready\nok\n{}\n{}\nend\n",
        first.display(),
        second.display()
    ));
    unsafe { dialog.Show(None) }?;
    let sent = SEEN
        .get()
        .expect("seen")
        .lock()?
        .take()
        .expect("launched")
        .join()
        .expect("thread");
    assert!(sent.contains("mode\topen\n"), "{sent}");
    assert!(sent.contains("title\tChoose a note\n"), "{sent}");
    assert!(sent.contains("type\tText\t*.txt\n"), "{sent}");

    let results = unsafe { dialog.GetResults() }?;
    assert_eq!(unsafe { results.GetCount() }?, 2);
    let name = unsafe { results.GetItemAt(1)?.GetDisplayName(SIGDN_FILESYSPATH)? };
    assert_eq!(PathBuf::from(unsafe { name.to_string() }?), second);

    // A program holding the base interface sees the same answer.
    let base: IFileDialog = dialog.cast()?;
    let single = unsafe { base.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)? };
    assert_eq!(PathBuf::from(unsafe { single.to_string() }?), first);

    // Cancelling in Firewol is what cancelling a Windows dialog reports.
    let other: IFileOpenDialog = unsafe { factory(&FileOpenDialog)?.CreateInstance(None)? };
    set_reply("ready\ncancel\n".to_owned());
    let cancelled = unsafe { other.Show(None) }.expect_err("cancelled");
    assert_eq!(cancelled.code(), ERROR_CANCELLED.to_hresult());
    SEEN.get().expect("seen").lock()?.take();

    // Save windows are not Firewol's: the class comes back untouched and usable.
    let save: IFileSaveDialog = unsafe { factory(&FileSaveDialog)?.CreateInstance(None)? };
    assert!(unsafe { save.GetOptions() }.is_ok());

    let unknown = GUID::from_u128(1);
    let mut object: *mut c_void = null_mut();
    let refused = unsafe { DllGetClassObject(&unknown, &IClassFactory::IID, &mut object) };
    assert!(refused.is_err() && object.is_null());
    Ok(())
}

fn request() -> PickRequest {
    PickRequest {
        type_index: 1,
        ..PickRequest::default()
    }
}

#[test]
fn no_firewol_means_the_windows_dialog() {
    let outcome = run_pick(&request(), Duration::from_secs(5), |_| false);
    assert_eq!(outcome, Outcome::Failed);
}

#[test]
fn a_firewol_that_never_appears_means_the_windows_dialog() {
    let outcome = run_pick(&request(), Duration::from_millis(300), |_| true);
    assert_eq!(outcome, Outcome::Failed);
}

#[test]
fn a_firewol_that_dies_before_answering_means_the_windows_dialog() {
    let outcome = run_pick(&request(), Duration::from_secs(5), |pipe| {
        drop(fake_firewol(pipe, "ready\n".to_owned()));
        true
    });
    assert_eq!(outcome, Outcome::Failed);
}

#[test]
fn a_path_that_does_not_exist_means_the_windows_dialog() {
    let outcome = run_pick(&request(), Duration::from_secs(5), |pipe| {
        drop(fake_firewol(
            pipe,
            "ready\nok\nC:\\no\\such\\file.txt\nend\n".to_owned(),
        ));
        true
    });
    assert_eq!(outcome, Outcome::Failed);
}

#[test]
fn the_wire_format_matches_what_firewol_parses() {
    let wire = PickRequest {
        folders: false,
        multi: true,
        title: Some("Bad\ttitle\nhere".into()),
        folder: Some(r"C:\Users".into()),
        file_name: None,
        types: vec![("Pictures".into(), "*.png;*.jpg".into())],
        type_index: 2,
    }
    .to_wire();
    assert_eq!(
        wire,
        "mode\topen\nmulti\t1\ntypeIndex\t2\ntitle\tBad title here\nfolder\tC:\\Users\ntype\tPictures\t*.png;*.jpg\n\n"
    );
}
