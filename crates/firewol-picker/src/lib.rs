//! Lets Firewol answer Windows' Open window for one user.
//!
//! Registered for the Open class only, this DLL creates the real Windows object and swaps the
//! vtable pointer of that one object for a copy with a few entries replaced. Everything else
//! still runs in Windows' code on Windows' own object, so `Show` can always fall back to the
//! real window: if Firewol is missing, slow to appear, crashes or the program needs custom
//! controls, the person simply gets the normal dialog.
#![cfg(windows)]

pub mod transport;

use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use transport::{Outcome, PickRequest, run_pick};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CloseHandle, E_NOINTERFACE, E_POINTER, ERROR_CANCELLED, HMODULE,
    HWND, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{CoTaskMemFree, IClassFactory};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW, GetProcAddress, LoadLibraryW,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT, FOS_PICKFOLDERS, IFileDialog,
    IFileDialogCustomize, IFileOpenDialog, IModalWindow, IShellItem, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromIDLists, SHParseDisplayName, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, SetForegroundWindow,
};
use windows_core::{GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR, PWSTR, s};

const FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xdc1c5a9c_e88a_4dde_a5a1_60f82a20aef7);
const FILE_SAVE_DIALOG: GUID = GUID::from_u128(0xc0b4e2f3_ba21_4773_8dba_335ec946eb8b);
/// Firewol's window must be up by then; a cold start takes about half a second.
const READY_WITHIN: Duration = Duration::from_secs(15);

// Positions in the COM vtables of IClassFactory and of the IFileDialog family.
const SLOT_QUERY_INTERFACE: usize = 0;
const SLOT_RELEASE: usize = 2;
const SLOT_CREATE_INSTANCE: usize = 3;
const SLOT_LOCK_SERVER: usize = 4;
const SLOT_SHOW: usize = 3;
const SLOT_SET_FILE_TYPES: usize = 4;
const SLOT_SET_TITLE: usize = 17;
const SLOT_GET_RESULT: usize = 20;
const SLOT_GET_RESULTS: usize = 27;
/// `IFileOpenDialog` and `IFileDialog2` both have this many; the copy is never shorter.
const DIALOG_SLOTS: usize = 29;

type Slot = *const c_void;
type GetClassObject =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
type QueryInterface =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;
type CreateInstance =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
type Show = unsafe extern "system" fn(*mut c_void, HWND) -> HRESULT;
type SetFileTypes =
    unsafe extern "system" fn(*mut c_void, u32, *const COMDLG_FILTERSPEC) -> HRESULT;
type SetTitle = unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT;
type GetResult = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;

/// What the program told the dialog that Windows offers no way to read back.
#[derive(Default)]
struct DialogState {
    types: Vec<(String, String)>,
    title: Option<String>,
    /// The program asked for its own controls, which only the real window can draw.
    customized: bool,
    picked: Option<Vec<PathBuf>>,
}

/// One copied vtable: the slots follow a header word holding the original table.
struct Table {
    original: usize,
    open: bool,
    hooked: usize,
}

fn guarded<T>(mutex: &'static Mutex<T>) -> MutexGuard<'static, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn tables() -> &'static Mutex<Vec<Table>> {
    static TABLES: OnceLock<Mutex<Vec<Table>>> = OnceLock::new();
    TABLES.get_or_init(Mutex::default)
}

fn states() -> &'static Mutex<HashMap<usize, DialogState>> {
    static STATES: OnceLock<Mutex<HashMap<usize, DialogState>>> = OnceLock::new();
    STATES.get_or_init(Mutex::default)
}

unsafe fn table_of(this: *mut c_void) -> *const Slot {
    unsafe { *this.cast::<*const Slot>() }
}

/// The table Windows gave this object, whether or not it has been replaced.
unsafe fn real_table(this: *mut c_void) -> *const Slot {
    let current = unsafe { table_of(this) } as usize;
    let known = guarded(tables())
        .iter()
        .any(|table| table.hooked == current);
    if known {
        unsafe { *(current as *const usize).sub(1) as *const Slot }
    } else {
        current as *const Slot
    }
}

unsafe fn real<F: Copy>(this: *mut c_void, slot: usize) -> F {
    let function = unsafe { *real_table(this).add(slot) };
    unsafe { std::mem::transmute_copy(&function) }
}

fn make_table(original: *const Slot, length: usize, replaced: &[(usize, Slot)]) -> usize {
    let mut slots = vec![original as usize];
    slots.extend((0..length).map(|slot| unsafe { *original.add(slot) } as usize));
    for (slot, function) in replaced {
        slots[slot + 1] = *function as usize;
    }
    // Lives as long as the DLL, which never unloads: objects keep pointing at it.
    let leaked: &'static mut [usize] = Vec::leak(slots);
    unsafe { leaked.as_ptr().add(1) as usize }
}

/// Points this one object at a copy of its vtable. `open` adds the slot only `IFileOpenDialog` has.
unsafe fn hook_dialog(this: *mut c_void, open: bool) {
    let original = unsafe { real_table(this) };
    let mut tables = guarded(tables());
    let current = unsafe { table_of(this) } as usize;
    // Only ever widen: a pointer already hooked as the open dialog keeps that table.
    if tables
        .iter()
        .any(|table| table.hooked == current && (table.open || !open))
    {
        return;
    }
    let found = tables
        .iter()
        .find(|table| table.original == original as usize && table.open == open)
        .map(|table| table.hooked);
    let hooked = found.unwrap_or_else(|| {
        let mut replaced: Vec<(usize, Slot)> = vec![
            (SLOT_QUERY_INTERFACE, query_interface_hook as Slot),
            (SLOT_SHOW, show_hook as Slot),
            (SLOT_SET_FILE_TYPES, set_file_types_hook as Slot),
            (SLOT_SET_TITLE, set_title_hook as Slot),
            (SLOT_GET_RESULT, get_result_hook as Slot),
        ];
        if open {
            replaced.push((SLOT_GET_RESULTS, get_results_hook as Slot));
        }
        let hooked = make_table(original, DIALOG_SLOTS, &replaced);
        tables.push(Table {
            original: original as usize,
            open,
            hooked,
        });
        hooked
    });
    unsafe { *this.cast::<usize>() = hooked };
}

fn dialog_interface(interface: &GUID) -> Option<bool> {
    match *interface {
        IFileOpenDialog::IID => Some(true),
        IFileDialog::IID | IModalWindow::IID => Some(false),
        _ => None,
    }
}

/// A dialog's interfaces can sit at different addresses; `IUnknown` names the object.
unsafe fn identity(this: *mut c_void) -> usize {
    let query: QueryInterface = unsafe { real(this, SLOT_QUERY_INTERFACE) };
    let mut unknown: *mut c_void = std::ptr::null_mut();
    if unsafe { query(this, &IUnknown::IID, &mut unknown) }.is_err() || unknown.is_null() {
        return this as usize;
    }
    let release: Release = unsafe { real(unknown, SLOT_RELEASE) };
    unsafe { release(unknown) };
    unknown as usize
}

unsafe extern "system" fn query_interface_hook(
    this: *mut c_void,
    interface: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    let original: QueryInterface = unsafe { real(this, SLOT_QUERY_INTERFACE) };
    let result = unsafe { original(this, interface, object) };
    if result.is_ok() && !interface.is_null() && !object.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            let requested = *interface;
            if requested == IFileDialogCustomize::IID {
                let id = identity(this);
                guarded(states()).entry(id).or_default().customized = true;
            } else if let Some(open) = dialog_interface(&requested) {
                hook_dialog(*object, open);
            }
        }));
    }
    result
}

unsafe fn wide_text(text: PCWSTR) -> Option<String> {
    (!text.is_null())
        .then(|| unsafe { text.to_string() }.ok())
        .flatten()
}

unsafe extern "system" fn set_file_types_hook(
    this: *mut c_void,
    count: u32,
    specs: *const COMDLG_FILTERSPEC,
) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        if specs.is_null() {
            return;
        }
        let types = std::slice::from_raw_parts(specs, count as usize)
            .iter()
            .filter_map(|spec| Some((wide_text(spec.pszName)?, wide_text(spec.pszSpec)?)))
            .collect();
        let id = identity(this);
        guarded(states()).entry(id).or_default().types = types;
    }));
    let original: SetFileTypes = unsafe { real(this, SLOT_SET_FILE_TYPES) };
    unsafe { original(this, count, specs) }
}

unsafe extern "system" fn set_title_hook(this: *mut c_void, title: PCWSTR) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let id = identity(this);
        guarded(states()).entry(id).or_default().title = wide_text(title);
    }));
    let original: SetTitle = unsafe { real(this, SLOT_SET_TITLE) };
    unsafe { original(this, title) }
}

unsafe fn taken_text(text: windows_core::Result<PWSTR>) -> Option<String> {
    let text = text.ok()?;
    let value = unsafe { text.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(text.0.cast())) };
    value
}

/// Reads what the program set up. `None` means only the real window can do this job.
unsafe fn describe(this: *mut c_void, id: usize) -> Option<PickRequest> {
    let dialog = unsafe { IFileDialog::from_raw_borrowed(&this) }?;
    let options: FILEOPENDIALOGOPTIONS = unsafe { dialog.GetOptions() }.ok()?;
    let mut states = guarded(states());
    let state = states.entry(id).or_default();
    if state.customized {
        return None;
    }
    let (types, title) = (state.types.clone(), state.title.clone());
    state.picked = None;
    drop(states);
    let folder = unsafe { dialog.GetFolder() }
        .ok()
        .and_then(|item: IShellItem| unsafe { taken_text(item.GetDisplayName(SIGDN_FILESYSPATH)) });
    Some(PickRequest {
        folders: options.0 & FOS_PICKFOLDERS.0 != 0,
        multi: options.0 & FOS_ALLOWMULTISELECT.0 != 0,
        title,
        folder,
        file_name: unsafe { taken_text(dialog.GetFileName()) }.filter(|name| !name.is_empty()),
        types,
        type_index: unsafe { dialog.GetFileTypeIndex() }.unwrap_or(1),
    })
}

type Launcher = fn(&str) -> bool;
static LAUNCHER: OnceLock<Launcher> = OnceLock::new();

/// Lets a test stand in for Firewol. Not used by anything shipped.
pub fn set_launcher_for_tests(launcher: Launcher) {
    let _ = LAUNCHER.set(launcher);
}

fn own_folder() -> Option<PathBuf> {
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(own_folder as *const () as *const u16),
            &mut module,
        )
    }
    .ok()?;
    let mut buffer = [0u16; 1024];
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    Path::new(&String::from_utf16_lossy(&buffer[..length]))
        .parent()
        .map(Path::to_owned)
}

/// Starts Firewol with no handles inherited from the calling program.
fn launch_firewol(pipe: &str) -> bool {
    let Some(executable) = own_folder().map(|folder| folder.join("firewol-tauri.exe")) else {
        return false;
    };
    if !executable.is_file() {
        return false;
    }
    let line = format!("\"{}\" --pick {pipe}", executable.display());
    let mut command: Vec<u16> = line.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // The picker has to come to the front, and only the program in front may allow that.
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    let started = unsafe {
        CreateProcessW(
            &HSTRING::from(executable.as_os_str()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT,
            None,
            PCWSTR::null(),
            &startup,
            &mut process,
        )
    };
    if started.is_err() {
        return false;
    }
    unsafe {
        let _ = CloseHandle(process.hThread);
        let _ = CloseHandle(process.hProcess);
    }
    true
}

unsafe fn pick_for(this: *mut c_void, owner: HWND) -> Outcome {
    let id = unsafe { identity(this) };
    let Some(request) = (unsafe { describe(this, id) }) else {
        return Outcome::Failed;
    };
    let launcher = LAUNCHER.get().copied().unwrap_or(launch_firewol);
    // Blocks input to the program's window the way a modal dialog does.
    let disabled = !owner.is_invalid() && !unsafe { EnableWindow(owner, false) }.as_bool();
    let outcome = run_pick(&request, READY_WITHIN, launcher);
    if disabled {
        unsafe {
            let _ = EnableWindow(owner, true);
            let _ = SetForegroundWindow(owner);
        }
    }
    if let Outcome::Picked(paths) = &outcome {
        guarded(states()).entry(id).or_default().picked = Some(paths.clone());
    }
    outcome
}

unsafe extern "system" fn show_hook(this: *mut c_void, owner: HWND) -> HRESULT {
    let outcome = catch_unwind(AssertUnwindSafe(|| unsafe { pick_for(this, owner) }));
    match outcome.unwrap_or(Outcome::Failed) {
        Outcome::Picked(_) => S_OK,
        Outcome::Cancelled => ERROR_CANCELLED.to_hresult(),
        Outcome::Failed => {
            let original: Show = unsafe { real(this, SLOT_SHOW) };
            unsafe { original(this, owner) }
        }
    }
}

unsafe fn picked_paths(this: *mut c_void) -> Option<Vec<PathBuf>> {
    let id = unsafe { identity(this) };
    guarded(states())
        .get(&id)
        .and_then(|state| state.picked.clone())
}

unsafe extern "system" fn get_result_hook(this: *mut c_void, item: *mut *mut c_void) -> HRESULT {
    if let Some(paths) = unsafe { picked_paths(this) } {
        if item.is_null() {
            return E_POINTER;
        }
        let name = HSTRING::from(paths[0].as_os_str());
        let created: windows_core::Result<IShellItem> =
            unsafe { SHCreateItemFromParsingName(&name, None) };
        return match created {
            Ok(shell_item) => {
                unsafe { *item = shell_item.into_raw() };
                S_OK
            }
            Err(error) => error.code(),
        };
    }
    let original: GetResult = unsafe { real(this, SLOT_GET_RESULT) };
    unsafe { original(this, item) }
}

fn item_array(paths: &[PathBuf]) -> windows_core::Result<*mut c_void> {
    let mut lists: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
    let created = (|| {
        for path in paths {
            let mut list: *mut ITEMIDLIST = std::ptr::null_mut();
            unsafe {
                SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut list, 0, None)?;
            }
            lists.push(list);
        }
        unsafe { SHCreateShellItemArrayFromIDLists(&lists) }
    })();
    for list in &lists {
        unsafe { CoTaskMemFree(Some(list.cast())) };
    }
    Ok(created?.into_raw())
}

unsafe extern "system" fn get_results_hook(this: *mut c_void, items: *mut *mut c_void) -> HRESULT {
    if let Some(paths) = unsafe { picked_paths(this) } {
        if items.is_null() {
            return E_POINTER;
        }
        return match item_array(&paths) {
            Ok(array) => {
                unsafe { *items = array };
                S_OK
            }
            Err(error) => error.code(),
        };
    }
    let original: GetResult = unsafe { real(this, SLOT_GET_RESULTS) };
    unsafe { original(this, items) }
}

#[repr(C)]
struct FactoryTable {
    query_interface: QueryInterface,
    add_ref: Release,
    release: Release,
    create_instance: CreateInstance,
    lock_server: unsafe extern "system" fn(*mut c_void, windows_core::BOOL) -> HRESULT,
}

/// Windows' class factory may live in read-only memory, so this small one stands in front of it.
#[repr(C)]
struct Factory {
    table: *const FactoryTable,
    references: AtomicU32,
    real: *mut c_void,
}

static FACTORY_TABLE: FactoryTable = FactoryTable {
    query_interface: factory_query_interface,
    add_ref: factory_add_ref,
    release: factory_release,
    create_instance: factory_create_instance,
    lock_server: factory_lock_server,
};

unsafe extern "system" fn factory_query_interface(
    this: *mut c_void,
    interface: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if interface.is_null() || object.is_null() {
        return E_POINTER;
    }
    unsafe {
        if *interface == IUnknown::IID || *interface == IClassFactory::IID {
            factory_add_ref(this);
            *object = this;
            S_OK
        } else {
            *object = std::ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn factory_add_ref(this: *mut c_void) -> u32 {
    unsafe {
        (*this.cast::<Factory>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}

unsafe extern "system" fn factory_release(this: *mut c_void) -> u32 {
    let factory = this.cast::<Factory>();
    let left = unsafe { (*factory).references.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        unsafe {
            let real = (*factory).real;
            let release: Release = self::real(real, SLOT_RELEASE);
            release(real);
            drop(Box::from_raw(factory));
        }
    }
    left
}

unsafe extern "system" fn factory_lock_server(
    this: *mut c_void,
    lock: windows_core::BOOL,
) -> HRESULT {
    let real = unsafe { (*this.cast::<Factory>()).real };
    let forward: unsafe extern "system" fn(*mut c_void, windows_core::BOOL) -> HRESULT =
        unsafe { self::real(real, SLOT_LOCK_SERVER) };
    unsafe { forward(real, lock) }
}

unsafe extern "system" fn factory_create_instance(
    this: *mut c_void,
    outer: *mut c_void,
    interface: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    let real = unsafe { (*this.cast::<Factory>()).real };
    let create: CreateInstance = unsafe { self::real(real, SLOT_CREATE_INSTANCE) };
    let result = unsafe { create(real, outer, interface, object) };
    if result.is_ok() && !interface.is_null() && !object.is_null() && outer.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            if let Some(open) = dialog_interface(&*interface) {
                let id = identity(*object);
                let mut known = guarded(states());
                // An old dialog's address can come back; it must not leave its answers behind.
                known.remove(&id);
                if known.len() > 64 {
                    known.clear();
                }
                drop(known);
                hook_dialog(*object, open);
            }
        }));
    }
    result
}

/// Puts Firewol's factory in front of Windows' one, or leaves Windows' own in place.
unsafe fn wrap_factory(real: *mut c_void, interface: &GUID, object: *mut *mut c_void) -> HRESULT {
    let factory = Box::into_raw(Box::new(Factory {
        table: &FACTORY_TABLE,
        references: AtomicU32::new(1),
        real,
    }));
    let result = unsafe { factory_query_interface(factory.cast(), interface, object) };
    unsafe { factory_release(factory.cast()) };
    result
}

/// Windows' own implementation, loaded by full path: the registry now names this file instead.
fn system_get_class_object() -> Option<GetClassObject> {
    static FUNCTION: OnceLock<Option<GetClassObject>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        let mut buffer = [0u16; 260];
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("comdlg32.dll");
        let module: HMODULE = unsafe { LoadLibraryW(&HSTRING::from(path.as_os_str())) }.ok()?;
        let function = unsafe { GetProcAddress(module, s!("DllGetClassObject")) }?;
        // The export has this signature by the COM contract.
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, GetClassObject>(function)
        })
    })
}

/// # Safety
/// Called by COM with valid pointers, as for any in-process server.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const GUID,
    interface: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    let Some(requested) = (unsafe { class.as_ref() }) else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    if *requested != FILE_OPEN_DIALOG && *requested != FILE_SAVE_DIALOG {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let Some(system) = system_get_class_object() else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    // Save windows stay Windows' own.
    if *requested != FILE_OPEN_DIALOG || interface.is_null() || object.is_null() {
        return unsafe { system(class, interface, object) };
    }
    let mut real: *mut c_void = std::ptr::null_mut();
    let result = unsafe { system(class, &IClassFactory::IID, &mut real) };
    if result.is_err() {
        return result;
    }
    unsafe { wrap_factory(real, &*interface, object) }
}

/// Objects keep pointing at tables and functions here, so this file stays loaded.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
