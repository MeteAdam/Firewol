#![cfg(windows)]

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use std::sync::atomic::{AtomicU32, Ordering};

use firewol_picker::transport::{Outcome, Pick, PickRequest, run_pick, run_pick_with};
use firewol_picker::{DllGetClassObject, set_launcher_for_tests, table_length};
use windows::Win32::Foundation::ERROR_CANCELLED;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IClassFactory,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyDescriptionList, IPropertyStore, PSGetPropertyDescriptionListFromString,
};
use windows::Win32::UI::Shell::{
    FDE_OVERWRITE_RESPONSE, FDE_SHAREVIOLATION_RESPONSE, FOS_ALLOWMULTISELECT, FileOpenDialog,
    FileSaveDialog, IFileDialog, IFileDialogEvents, IFileDialogEvents_Impl, IFileOpenDialog,
    IFileSaveDialog, IShellItem, SIGDN_FILESYSPATH,
};
use windows_core::{GUID, HRESULT, Interface, Ref, implement, s, w};

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

fn sent_request() -> String {
    SEEN.get()
        .expect("seen")
        .lock()
        .expect("lock")
        .take()
        .expect("launched")
        .join()
        .expect("thread")
}

static CHECKED: AtomicU32 = AtomicU32::new(0);
static CHECKED_PATH: Mutex<String> = Mutex::new(String::new());

/// Plays a program that checks the chosen name in `OnFileOk`, as Windows' window would let it.
#[implement(IFileDialogEvents)]
struct Checker;

impl IFileDialogEvents_Impl for Checker_Impl {
    fn OnFileOk(&self, dialog: Ref<IFileDialog>) -> windows_core::Result<()> {
        CHECKED.fetch_add(1, Ordering::SeqCst);
        let chosen = unsafe {
            dialog
                .ok()?
                .GetResult()?
                .GetDisplayName(SIGDN_FILESYSPATH)?
        };
        *CHECKED_PATH.lock().expect("lock") = unsafe { chosen.to_string() }?;
        unsafe { CoTaskMemFree(Some(chosen.0.cast())) };
        Ok(())
    }
    fn OnFolderChanging(
        &self,
        _: Ref<IFileDialog>,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn OnFolderChange(&self, _: Ref<IFileDialog>) -> windows_core::Result<()> {
        Ok(())
    }
    fn OnSelectionChange(&self, _: Ref<IFileDialog>) -> windows_core::Result<()> {
        Ok(())
    }
    fn OnShareViolation(
        &self,
        _: Ref<IFileDialog>,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<FDE_SHAREVIOLATION_RESPONSE> {
        Ok(FDE_SHAREVIOLATION_RESPONSE(0))
    }
    fn OnTypeChange(&self, _: Ref<IFileDialog>) -> windows_core::Result<()> {
        Ok(())
    }
    fn OnOverwrite(
        &self,
        _: Ref<IFileDialog>,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<FDE_OVERWRITE_RESPONSE> {
        Ok(FDE_OVERWRITE_RESPONSE(0))
    }
}

fn display_path(
    item: windows_core::Result<IShellItem>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let name = unsafe { item?.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = PathBuf::from(unsafe { name.to_string() }?);
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    Ok(path)
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
        // Two answers come back below, which only a program allowing several items accepts.
        dialog.SetOptions(options | FOS_ALLOWMULTISELECT)?;
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
    let sent = sent_request();
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

    // A Save window answers with a file that does not exist yet and the type chosen for it.
    let save: IFileSaveDialog = unsafe { factory(&FileSaveDialog)?.CreateInstance(None)? };
    let pictures = [
        COMDLG_FILTERSPEC {
            pszName: w!("PNG"),
            pszSpec: w!("*.png"),
        },
        COMDLG_FILTERSPEC {
            pszName: w!("JPEG"),
            pszSpec: w!("*.jpg;*.jpeg"),
        },
    ];
    let checker: IFileDialogEvents = Checker.into();
    let cookie = unsafe {
        save.SetFileTypes(&pictures)?;
        save.SetFileTypeIndex(1)?;
        save.SetDefaultExtension(w!("png"))?;
        save.SetFileName(w!("image"))?;
        save.SetOkButtonLabel(w!("Export"))?;
        // Paint asks for Windows' property fields; that no longer means Windows' window.
        let mut list: *mut c_void = null_mut();
        PSGetPropertyDescriptionListFromString(
            w!("prop:System.Keywords"),
            &IPropertyDescriptionList::IID,
            &mut list,
        )?;
        let fields = IPropertyDescriptionList::from_raw(list);
        save.SetCollectedProperties(&fields, true)?;
        save.Advise(&checker)?
    };
    let target = fixture.path().join("image.jpg");
    set_reply(format!(
        "ready\ntypeIndex\t2\nok\n{}\nend\n",
        target.display()
    ));
    unsafe { save.Show(None) }?;
    let sent = sent_request();
    for line in [
        "mode\tsave\n",
        "name\timage\n",
        "defaultExt\tpng\n",
        "okLabel\tExport\n",
        "overwrite\t1\n",
        "testCreate\t1\n",
        "type\tJPEG\t*.jpg;*.jpeg\n",
    ] {
        assert!(sent.contains(line), "{line:?} in {sent}");
    }
    assert!(
        !target.exists(),
        "Firewol answers; the program writes the file"
    );
    assert_eq!(display_path(unsafe { save.GetResult() })?, target);
    assert_eq!(unsafe { save.GetFileTypeIndex() }?, 2);
    let name = unsafe { save.GetFileName() }?;
    assert_eq!(unsafe { name.to_string() }?, "image.jpg");
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    assert_eq!(display_path(unsafe { save.GetFolder() })?, fixture.path());
    // The program checked the name once and saw Firewol's answer, as with Windows' window.
    assert_eq!(CHECKED.load(Ordering::SeqCst), 1);
    assert_eq!(PathBuf::from(&*CHECKED_PATH.lock()?), target);
    unsafe { save.Unadvise(cookie) }?;
    // Through the base interface too, and the rest of the Save interface is Windows' own.
    let base: IFileDialog = save.cast()?;
    assert_eq!(display_path(unsafe { base.GetResult() })?, target);
    // `GetProperties`, past the slots any other dialog interface has, still answers.
    type GetProperties = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
    let get_properties: GetProperties = unsafe {
        let table = *(save.as_raw() as *const *const usize);
        std::mem::transmute::<usize, GetProperties>(*table.add(30))
    };
    let mut store: *mut c_void = null_mut();
    unsafe { get_properties(save.as_raw(), &mut store) }.ok()?;
    // Firewol has no property fields, so the program gets an empty set to apply.
    let store = unsafe { IPropertyStore::from_raw(store) };
    assert_eq!(unsafe { store.GetCount() }?, 0);

    // A reply naming a folder that does not exist means Windows' window; nothing is shown here
    // because the test cannot click it, so only the transport is checked.
    let saving = PickRequest {
        save: true,
        types: vec![("PNG".into(), "*.png".into())],
        type_index: 1,
        ..PickRequest::default()
    };
    let missing = run_pick(&saving, Duration::from_secs(5), |pipe| {
        drop(fake_firewol(
            pipe,
            "ready\nok\nC:\\no\\such\\folder\\image.png\nend\n".to_owned(),
        ));
        true
    });
    assert_eq!(missing, Outcome::Failed);
    let fine = fixture.path().join("new.png");
    let answered = run_pick(&saving, Duration::from_secs(5), |pipe| {
        drop(fake_firewol(
            pipe,
            format!("ready\ntypeIndex\t9\nok\n{}\nend\n", fine.display()),
        ));
        true
    });
    assert_eq!(
        answered,
        Outcome::Picked(Pick {
            paths: vec![fine],
            type_index: None,
        })
    );

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
        ..PickRequest::default()
    }
    .to_wire();
    assert_eq!(
        wire,
        "mode\topen\nmulti\t1\ntypeIndex\t2\ntitle\tBad title here\nfolder\tC:\\Users\ntype\tPictures\t*.png;*.jpg\n\n"
    );
}

/// Windows' own factory, straight from comdlg32, so no registry entry can put Firewol in front.
fn windows_factory(class: &GUID) -> windows_core::Result<IClassFactory> {
    type GetClassObject =
        unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
    let module = unsafe { LoadLibraryW(w!("comdlg32.dll"))? };
    let function = unsafe { GetProcAddress(module, s!("DllGetClassObject")) }
        .ok_or_else(windows_core::Error::from_thread)?;
    let get: GetClassObject = unsafe { std::mem::transmute(function) };
    let mut object: *mut c_void = null_mut();
    unsafe { get(class, &IClassFactory::IID, &mut object) }.ok()?;
    Ok(unsafe { IClassFactory::from_raw(object) })
}

unsafe fn table(object: *mut c_void) -> *const usize {
    unsafe { *object.cast::<*const usize>() }
}

/// Windows' classes call their own virtual methods through the table Firewol swaps; a copy cut
/// at the interface's last slot crashed Notepad and PickerHost on 2026-11-10.
#[test]
fn a_hooked_dialog_keeps_every_slot_of_windows_table() -> TestResult {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    for (class, save) in [(FileSaveDialog, true), (FileOpenDialog, false)] {
        let real: IFileDialog = unsafe { windows_factory(&class)?.CreateInstance(None)? };
        let real_table = unsafe { table(real.as_raw()) };
        let length = table_length(real_table.cast(), 0);
        let interface_slots = if save { 32 } else { 29 };
        println!(
            "{} dialog: Windows' table has {length} code entries",
            if save { "Save" } else { "Open" }
        );
        assert!(
            length >= interface_slots,
            "the scan stopped inside the interface at {length}"
        );

        let hooked: IFileDialog = unsafe { factory(&class)?.CreateInstance(None)? };
        let copy = unsafe { table(hooked.as_raw()) };
        let original = unsafe { *copy.sub(1) } as *const usize;
        assert_ne!(copy, original, "the dialog was not hooked");
        for slot in interface_slots..length {
            assert_eq!(
                unsafe { *copy.add(slot) },
                unsafe { *original.add(slot) },
                "slot {slot} differs"
            );
        }
    }
    Ok(())
}

static RESTORED: AtomicU32 = AtomicU32::new(0);

/// Firewol closes its window only after `back`, and `back` comes only after the program's window
/// is enabled and in front again; otherwise Windows hands the focus to another program.
#[test]
fn the_program_takes_the_focus_back_before_firewol_closes() {
    let (sender, heard) = std::sync::mpsc::channel();
    let outcome = run_pick_with(
        &request(),
        Duration::from_secs(5),
        |pipe| {
            let pipe = pipe.to_owned();
            let sender = sender.clone();
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
                file.write_all(b"ready\ncancel\n").expect("reply");
                let mut rest = String::new();
                let _ = file.read_to_string(&mut rest);
                // What the program had done by the time Firewol heard from it.
                let _ = sender.send((rest, RESTORED.load(Ordering::SeqCst)));
            });
            true
        },
        || {
            RESTORED.fetch_add(1, Ordering::SeqCst);
        },
    );
    assert_eq!(outcome, Outcome::Cancelled);
    let (rest, restored) = heard
        .recv_timeout(Duration::from_secs(5))
        .expect("Firewol heard back");
    assert_eq!(rest, "back\n");
    assert_eq!(restored, 1, "the window came back before Firewol was told");
}
