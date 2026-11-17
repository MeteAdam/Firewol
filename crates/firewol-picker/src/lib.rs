//! Lets Firewol answer Windows' Open and Save windows for one user.
//!
//! Registered for the Open and Save classes, this DLL creates the real Windows object and swaps
//! the vtable pointer of that one object for a copy with a few entries replaced. Everything else
//! still runs in Windows' code on Windows' own object, so `Show` can always fall back to the
//! real window: if Firewol is missing, slow to appear, crashes, the program needs custom
//! controls or refuses the chosen name, the person simply gets the normal dialog.
#![cfg(windows)]

pub mod transport;

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use transport::{Outcome, Pick, PickRequest, run_pick_with};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CloseHandle, E_FAIL, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER,
    ERROR_CANCELLED, HANDLE, HMODULE, HWND, S_FALSE, S_OK,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenIsAppContainer};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, WIN32_FIND_DATAW};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree, CreateBindCtx, IClassFactory};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW, GetProcAddress, LoadLibraryW,
};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemDirectoryW};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetCurrentProcess, OpenProcessToken,
    PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSCreateMemoryPropertyStore};
use windows::Win32::UI::Shell::{
    FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT, FOS_NOREADONLYRETURN, FOS_NOTESTFILECREATE,
    FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, FOS_STRICTFILETYPES, IFileDialog, IFileDialog2,
    IFileDialogCustomize, IFileDialogEvents, IFileOpenDialog, IFileSaveDialog, IFileSystemBindData,
    IFileSystemBindData_Impl, IModalWindow, IShellItem, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromIDLists, SHParseDisplayName, SIGDN_FILESYSPATH,
    STR_FILE_SYS_BIND_DATA,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, GetForegroundWindow, SetForegroundWindow,
};
use windows_core::{GUID, HRESULT, HSTRING, IUnknown, Interface, PCWSTR, PWSTR, implement, s};

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
const SLOT_GET_FILE_TYPE_INDEX: usize = 6;
const SLOT_ADVISE: usize = 7;
const SLOT_UNADVISE: usize = 8;
const SLOT_GET_FOLDER: usize = 13;
const SLOT_GET_CURRENT_SELECTION: usize = 14;
const SLOT_GET_FILE_NAME: usize = 16;
const SLOT_SET_TITLE: usize = 17;
const SLOT_SET_OK_BUTTON_LABEL: usize = 18;
const SLOT_GET_RESULT: usize = 20;
const SLOT_SET_DEFAULT_EXTENSION: usize = 22;
/// `IFileOpenDialog::GetResults`. The same position is `SetSaveAsItem` in `IFileSaveDialog` and
/// `SetCancelButtonLabel` in `IFileDialog2`, so it is replaced only on the interface asked for.
const SLOT_GET_RESULTS: usize = 27;
const SLOT_SET_SAVE_AS_ITEM: usize = 27;
const SLOT_GET_PROPERTIES: usize = 30;
/// `IFileOpenDialog` and `IFileDialog2` both have this many; an Open copy is never shorter.
const OPEN_SLOTS: usize = 29;
/// `IFileSaveDialog` has this many, and every interface of a Save object gets a copy at least this long.
const SAVE_SLOTS: usize = 32;
/// No table Windows hands out comes near this; it only bounds a scan that finds no end.
const MAX_SLOTS: usize = 512;

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
type SetText = unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT;
type GetItem = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT;
type GetIndex = unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT;
type GetText = unsafe extern "system" fn(*mut c_void, *mut PWSTR) -> HRESULT;
type Advise = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut u32) -> HRESULT;
type Unadvise = unsafe extern "system" fn(*mut c_void, u32) -> HRESULT;
type SetItem = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;

/// Which interface a pointer was handed out as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `IFileDialog`, `IFileDialog2` or `IModalWindow`: only the slots they all share are replaced.
    Base,
    Open,
    Save,
}

/// What the program told the dialog that Windows offers no way to read back.
#[derive(Default)]
struct DialogState {
    types: Vec<(String, String)>,
    title: Option<String>,
    /// Set when the program asked for its own controls, which only the real window can draw.
    customized: Option<&'static str>,
    picked: Option<Pick>,
    default_extension: Option<String>,
    ok_label: Option<String>,
    save_as: Option<PathBuf>,
    /// Event sinks by cookie. The dialog itself holds them while they are registered.
    sinks: Vec<(u32, usize)>,
}

/// One copied vtable: the slots follow a header word holding the original table.
struct Table {
    original: usize,
    kind: Kind,
    /// The object is a Save window.
    save: bool,
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

/// True when this pointer belongs to a Save window.
unsafe fn is_save(this: *mut c_void) -> bool {
    let current = unsafe { table_of(this) } as usize;
    guarded(tables())
        .iter()
        .any(|table| table.hooked == current && table.save)
}

fn memory_at(address: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let size = unsafe {
        VirtualQuery(
            Some(address as *const c_void),
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    (size != 0 && info.State == MEM_COMMIT).then_some(info)
}

/// True when `address` is inside the code of a loaded module.
fn is_code(address: usize) -> bool {
    const EXECUTE: u32 =
        PAGE_EXECUTE.0 | PAGE_EXECUTE_READ.0 | PAGE_EXECUTE_READWRITE.0 | PAGE_EXECUTE_WRITECOPY.0;
    memory_at(address).is_some_and(|info| info.Type == MEM_IMAGE && info.Protect.0 & EXECUTE != 0)
}

/// How many entries the table at `table` has, never fewer than `minimum`.
///
/// Windows' dialog classes extend an interface's table with their own virtual methods and call
/// them through the same pointer, so a copy limited to the interface's slots sends those calls
/// into whatever follows the copy, and Control Flow Guard ends the program (Notepad, and the
/// PickerHost that serves Snipping Tool, on 2026-11-10). The table runs as long as its entries
/// point at code; a neighbouring table copied along is never called and costs nothing.
pub fn table_length(table: *const c_void, minimum: usize) -> usize {
    let start = table as usize;
    let Some(end) = memory_at(start)
        .filter(|info| info.Protect.0 & (PAGE_NOACCESS.0 | PAGE_GUARD.0) == 0)
        .map(|info| info.BaseAddress as usize + info.RegionSize)
    else {
        return minimum;
    };
    let mut length = 0;
    while length < MAX_SLOTS {
        let at = start + length * size_of::<usize>();
        if at + size_of::<usize>() > end || !is_code(unsafe { *(at as *const usize) }) {
            break;
        }
        length += 1;
    }
    length.max(minimum)
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

fn replaced_slots(kind: Kind, save: bool) -> Vec<(usize, Slot)> {
    let mut replaced: Vec<(usize, Slot)> = vec![
        (SLOT_QUERY_INTERFACE, query_interface_hook as Slot),
        (SLOT_SHOW, show_hook as Slot),
        (SLOT_SET_FILE_TYPES, set_file_types_hook as Slot),
        (SLOT_SET_TITLE, set_title_hook as Slot),
        (SLOT_GET_RESULT, get_result_hook as Slot),
    ];
    if kind == Kind::Open {
        replaced.push((SLOT_GET_RESULTS, get_results_hook as Slot));
    }
    if save {
        replaced.extend([
            (SLOT_GET_FILE_TYPE_INDEX, get_file_type_index_hook as Slot),
            (SLOT_ADVISE, advise_hook as Slot),
            (SLOT_UNADVISE, unadvise_hook as Slot),
            (SLOT_GET_FOLDER, get_folder_hook as Slot),
            (
                SLOT_GET_CURRENT_SELECTION,
                get_current_selection_hook as Slot,
            ),
            (SLOT_GET_FILE_NAME, get_file_name_hook as Slot),
            (SLOT_SET_OK_BUTTON_LABEL, set_ok_button_label_hook as Slot),
            (
                SLOT_SET_DEFAULT_EXTENSION,
                set_default_extension_hook as Slot,
            ),
        ]);
        if kind == Kind::Save {
            replaced.extend([
                (SLOT_SET_SAVE_AS_ITEM, set_save_as_item_hook as Slot),
                (SLOT_GET_PROPERTIES, get_properties_hook as Slot),
            ]);
        }
    }
    replaced
}

/// Points this one object at a copy of its vtable with the slots its interface needs replaced.
unsafe fn hook_dialog(this: *mut c_void, kind: Kind, save: bool) {
    let original = unsafe { real_table(this) };
    let mut tables = guarded(tables());
    let current = unsafe { table_of(this) } as usize;
    // Only ever widen: a pointer already hooked for its own interface keeps that table.
    if tables
        .iter()
        .any(|table| table.hooked == current && (kind == Kind::Base || table.kind == kind))
    {
        return;
    }
    let found = tables
        .iter()
        .find(|table| {
            table.original == original as usize && table.kind == kind && table.save == save
        })
        .map(|table| table.hooked);
    let hooked = found.unwrap_or_else(|| {
        let interface_slots = if save || kind == Kind::Save {
            SAVE_SLOTS
        } else {
            OPEN_SLOTS
        };
        let length = table_length(original.cast(), interface_slots);
        let hooked = make_table(original, length, &replaced_slots(kind, save));
        tables.push(Table {
            original: original as usize,
            kind,
            save,
            hooked,
        });
        hooked
    });
    unsafe { *this.cast::<usize>() = hooked };
}

fn dialog_interface(interface: &GUID) -> Option<Kind> {
    match *interface {
        IFileOpenDialog::IID => Some(Kind::Open),
        IFileSaveDialog::IID => Some(Kind::Save),
        IFileDialog::IID | IFileDialog2::IID | IModalWindow::IID => Some(Kind::Base),
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

/// Runs `change` on what is known about this dialog. Never call back into the dialog inside it.
unsafe fn with_state<T>(this: *mut c_void, change: impl FnOnce(&mut DialogState) -> T) -> T {
    let id = unsafe { identity(this) };
    change(guarded(states()).entry(id).or_default())
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
                with_state(this, |state| state.customized = Some("customized"));
            } else if let Some(kind) = dialog_interface(&requested) {
                hook_dialog(*object, kind, is_save(this));
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
        with_state(this, |state| state.types = types);
    }));
    let original: SetFileTypes = unsafe { real(this, SLOT_SET_FILE_TYPES) };
    unsafe { original(this, count, specs) }
}

unsafe extern "system" fn set_title_hook(this: *mut c_void, title: PCWSTR) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let title = wide_text(title);
        with_state(this, |state| state.title = title);
    }));
    let original: SetText = unsafe { real(this, SLOT_SET_TITLE) };
    unsafe { original(this, title) }
}

unsafe extern "system" fn set_ok_button_label_hook(this: *mut c_void, label: PCWSTR) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let label = wide_text(label).filter(|label| !label.is_empty());
        with_state(this, |state| state.ok_label = label);
    }));
    let original: SetText = unsafe { real(this, SLOT_SET_OK_BUTTON_LABEL) };
    unsafe { original(this, label) }
}

unsafe extern "system" fn set_default_extension_hook(
    this: *mut c_void,
    extension: PCWSTR,
) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let extension = wide_text(extension)
            .map(|text| text.trim_start_matches('.').to_owned())
            .filter(|text| !text.is_empty());
        with_state(this, |state| state.default_extension = extension);
    }));
    let original: SetText = unsafe { real(this, SLOT_SET_DEFAULT_EXTENSION) };
    unsafe { original(this, extension) }
}

unsafe extern "system" fn set_save_as_item_hook(this: *mut c_void, item: *mut c_void) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let path = IShellItem::from_raw_borrowed(&item)
            .and_then(|item| taken_text(item.GetDisplayName(SIGDN_FILESYSPATH)))
            .map(PathBuf::from);
        with_state(this, |state| state.save_as = path);
    }));
    let original: SetItem = unsafe { real(this, SLOT_SET_SAVE_AS_ITEM) };
    unsafe { original(this, item) }
}

/// A program that asked for Windows' property fields under the name (Paint does) reads what the
/// person typed there. Firewol has no such fields, so after its answer the set is empty and the
/// program saves the file without extra tags or authors.
unsafe extern "system" fn get_properties_hook(
    this: *mut c_void,
    store: *mut *mut c_void,
) -> HRESULT {
    if unsafe { picked(this) }.is_some() {
        if store.is_null() {
            return E_POINTER;
        }
        let mut created: *mut c_void = std::ptr::null_mut();
        return match unsafe { PSCreateMemoryPropertyStore(&IPropertyStore::IID, &mut created) } {
            Ok(()) => {
                unsafe { *store = created };
                S_OK
            }
            Err(error) => error.code(),
        };
    }
    let original: GetItem = unsafe { real(this, SLOT_GET_PROPERTIES) };
    unsafe { original(this, store) }
}

unsafe extern "system" fn advise_hook(
    this: *mut c_void,
    sink: *mut c_void,
    cookie: *mut u32,
) -> HRESULT {
    let original: Advise = unsafe { real(this, SLOT_ADVISE) };
    let result = unsafe { original(this, sink, cookie) };
    if result.is_ok() && !sink.is_null() && !cookie.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            let cookie = *cookie;
            with_state(this, |state| state.sinks.push((cookie, sink as usize)));
        }));
    }
    result
}

unsafe extern "system" fn unadvise_hook(this: *mut c_void, cookie: u32) -> HRESULT {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        with_state(this, |state| {
            state.sinks.retain(|(known, _)| *known != cookie)
        });
    }));
    let original: Unadvise = unsafe { real(this, SLOT_UNADVISE) };
    unsafe { original(this, cookie) }
}

unsafe fn taken_text(text: windows_core::Result<PWSTR>) -> Option<String> {
    let text = text.ok()?;
    let value = unsafe { text.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(text.0.cast())) };
    value
}

/// Reads what the program set up. An error names why only the real window can do this job.
unsafe fn describe(this: *mut c_void, id: usize) -> Result<PickRequest, &'static str> {
    let dialog = unsafe { IFileDialog::from_raw_borrowed(&this) }.ok_or("no-dialog")?;
    let save = unsafe { is_save(this) };
    let options: FILEOPENDIALOGOPTIONS =
        unsafe { dialog.GetOptions() }.map_err(|_| "no-options")?;
    let mut states = guarded(states());
    let state = states.entry(id).or_default();
    if let Some(reason) = state.customized {
        return Err(reason);
    }
    let types = state.types.clone();
    let title = state.title.clone();
    let default_extension = state.default_extension.clone();
    let ok_label = state.ok_label.clone();
    let save_as = state.save_as.clone();
    state.picked = None;
    drop(states);
    let folder = unsafe { dialog.GetFolder() }
        .ok()
        .and_then(|item: IShellItem| unsafe { taken_text(item.GetDisplayName(SIGDN_FILESYSPATH)) });
    let file_name = unsafe { taken_text(dialog.GetFileName()) }
        .filter(|name| !name.is_empty())
        .or_else(|| {
            let name = save_as.as_deref()?.file_name()?;
            Some(name.to_string_lossy().into_owned())
        });
    let has = |flag: FILEOPENDIALOGOPTIONS| options.0 & flag.0 != 0;
    Ok(PickRequest {
        folders: !save && has(FOS_PICKFOLDERS),
        multi: !save && has(FOS_ALLOWMULTISELECT),
        save,
        title,
        folder,
        file_name,
        types,
        type_index: unsafe { dialog.GetFileTypeIndex() }.unwrap_or(1),
        default_extension: default_extension.filter(|_| save),
        ok_label,
        save_as: save_as
            .filter(|_| save)
            .map(|path| path.to_string_lossy().into_owned()),
        overwrite_prompt: save && has(FOS_OVERWRITEPROMPT),
        strict_types: save && has(FOS_STRICTFILETYPES),
        no_read_only: save && has(FOS_NOREADONLYRETURN),
        test_create: save && !has(FOS_NOTESTFILECREATE),
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

/// A sandboxed program could neither start Firewol properly nor let it connect; when this cannot
/// be read, the answer is the safe one.
fn in_app_container() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return true;
    }
    let mut value = 0u32;
    let mut length = 0u32;
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenIsAppContainer,
            Some((&raw mut value).cast()),
            size_of::<u32>() as u32,
            &mut length,
        )
    };
    unsafe {
        let _ = CloseHandle(token);
    }
    read.is_err() || value != 0
}

/// Lets the program check the chosen name the way Windows' window would, through the event
/// sinks it registered. Only `S_FALSE` refuses, as the documentation of `OnFileOk` says.
unsafe fn accepted_by_program(this: *mut c_void, id: usize) -> bool {
    let sinks: Vec<usize> = guarded(states())
        .get(&id)
        .map(|state| state.sinks.iter().map(|(_, sink)| *sink).collect())
        .unwrap_or_default();
    if sinks.is_empty() {
        return true;
    }
    // Through the hook, so the dialog the program sees answers with what Firewol picked.
    let mut dialog: *mut c_void = std::ptr::null_mut();
    if unsafe { query_interface_hook(this, &IFileDialog::IID, &mut dialog) }.is_err()
        || dialog.is_null()
    {
        return true;
    }
    let accepted = sinks.into_iter().all(|sink| {
        let sink = sink as *mut c_void;
        unsafe { IFileDialogEvents::from_raw_borrowed(&sink) }
            .is_none_or(|events| unsafe { (events.vtable().OnFileOk)(sink, dialog) } != S_FALSE)
    });
    let release: Release = unsafe { real(dialog, SLOT_RELEASE) };
    unsafe { release(dialog) };
    accepted
}

/// Hands what Firewol chose to Windows' own window, which opens next.
unsafe fn prefill(this: *mut c_void, pick: &Pick) {
    let Some(dialog) = (unsafe { IFileDialog::from_raw_borrowed(&this) }) else {
        return;
    };
    let path = &pick.paths[0];
    if let Some(parent) = path.parent()
        && let Ok(folder) = unsafe {
            SHCreateItemFromParsingName::<_, _, IShellItem>(
                &HSTRING::from(parent.as_os_str()),
                None,
            )
        }
    {
        let _ = unsafe { dialog.SetFolder(&folder) };
    }
    if let Some(name) = path.file_name() {
        let _ = unsafe { dialog.SetFileName(&HSTRING::from(name)) };
    }
    if let Some(index) = pick.type_index {
        let _ = unsafe { dialog.SetFileTypeIndex(index) };
    }
}

/// What happened to one `Show`, with the reason Windows' own window opened when it did.
unsafe fn pick_for(this: *mut c_void, owner: HWND) -> (Outcome, &'static str) {
    if in_app_container() {
        return (Outcome::Failed, "app-container");
    }
    let id = unsafe { identity(this) };
    let request = match unsafe { describe(this, id) } {
        Ok(request) => request,
        Err(reason) => return (Outcome::Failed, reason),
    };
    let launcher = LAUNCHER.get().copied().unwrap_or(launch_firewol);
    // A program without an owner window still gets its window in front back.
    let front = if owner.is_invalid() {
        unsafe { GetForegroundWindow() }
    } else {
        owner
    };
    // Blocks input to the program's window the way a modal dialog does.
    let disabled = !owner.is_invalid() && !unsafe { EnableWindow(owner, false) }.as_bool();
    let outcome = run_pick_with(&request, READY_WITHIN, launcher, || unsafe {
        if disabled {
            let _ = EnableWindow(owner, true);
        }
        // Allowed because Firewol, in front while the person answered, said so first.
        if !front.is_invalid() {
            let _ = SetForegroundWindow(front);
        }
    });
    let pick = match &outcome {
        Outcome::Picked(pick) => pick,
        Outcome::Cancelled => return (outcome, "cancelled"),
        Outcome::Failed => return (outcome, "no-answer"),
    };
    guarded(states()).entry(id).or_default().picked = Some(pick.clone());
    if request.save && !unsafe { accepted_by_program(this, id) } {
        if let Some(state) = guarded(states()).get_mut(&id) {
            state.picked = None;
        }
        unsafe { prefill(this, pick) };
        return (Outcome::Failed, "refused");
    }
    (outcome, "firewol")
}

/// Keeps the dialog log from growing without end; the newest lines matter.
const LOG_LIMIT: u64 = 256 * 1024;

/// One line per window in `%LOCALAPPDATA%\Firewol\file-dialogs.log`, so a report such as "this
/// program still showed Windows' window" can be answered from the reason instead of guesswork.
/// Best effort: a program whose writes are redirected or refused simply leaves no line.
fn log_show(save: bool, reason: &str) {
    let Some(data) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let path = Path::new(&data).join(r"Firewol\file-dialogs.log");
    let full = std::fs::metadata(&path).is_ok_and(|file| file.len() > LOG_LIMIT);
    let mut options = std::fs::OpenOptions::new();
    options.create(true);
    if full {
        options.write(true).truncate(true);
    } else {
        options.append(true);
    }
    let Ok(mut file) = options.open(&path) else {
        return;
    };
    let mut buffer = [0u16; 1024];
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    let program = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
    let time = unsafe { GetLocalTime() };
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}\t{}\t{program}\t{reason}\n",
        time.wYear,
        time.wMonth,
        time.wDay,
        time.wHour,
        time.wMinute,
        time.wSecond,
        if save { "save" } else { "open" },
    );
    let _ = std::io::Write::write_all(&mut file, line.as_bytes());
}

unsafe extern "system" fn show_hook(this: *mut c_void, owner: HWND) -> HRESULT {
    let outcome = catch_unwind(AssertUnwindSafe(|| unsafe { pick_for(this, owner) }));
    let (outcome, reason) = outcome.unwrap_or((Outcome::Failed, "panic"));
    let _ = catch_unwind(|| log_show(unsafe { is_save(this) }, reason));
    match outcome {
        Outcome::Picked(_) => S_OK,
        Outcome::Cancelled => ERROR_CANCELLED.to_hresult(),
        Outcome::Failed => {
            let original: Show = unsafe { real(this, SLOT_SHOW) };
            unsafe { original(this, owner) }
        }
    }
}

unsafe fn picked(this: *mut c_void) -> Option<Pick> {
    let id = unsafe { identity(this) };
    guarded(states())
        .get(&id)
        .and_then(|state| state.picked.clone())
}

/// Tells the Shell what a file about to be saved will be, since it does not exist yet.
#[implement(IFileSystemBindData)]
struct NewFile(WIN32_FIND_DATAW);

impl IFileSystemBindData_Impl for NewFile_Impl {
    fn SetFindData(&self, _data: *const WIN32_FIND_DATAW) -> windows_core::Result<()> {
        Ok(())
    }

    fn GetFindData(&self, data: *mut WIN32_FIND_DATAW) -> windows_core::Result<()> {
        if data.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *data = self.0 };
        Ok(())
    }
}

/// The Shell item of `path`, including a file a Save window is about to create.
fn shell_item(path: &Path) -> windows_core::Result<IShellItem> {
    let name = HSTRING::from(path.as_os_str());
    if path.exists() {
        return unsafe { SHCreateItemFromParsingName(&name, None) };
    }
    let mut find: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    find.dwFileAttributes = FILE_ATTRIBUTE_NORMAL.0;
    if let Some(file) = path.file_name() {
        let wide: Vec<u16> = file.encode_wide().take(find.cFileName.len() - 1).collect();
        find.cFileName[..wide.len()].copy_from_slice(&wide);
    }
    let bind = unsafe { CreateBindCtx(0)? };
    let data: IFileSystemBindData = NewFile(find).into();
    unsafe { bind.RegisterObjectParam(STR_FILE_SYS_BIND_DATA, &data)? };
    unsafe { SHCreateItemFromParsingName(&name, &bind) }
}

unsafe fn answer_item(item: *mut *mut c_void, path: Option<&Path>) -> HRESULT {
    if item.is_null() {
        return E_POINTER;
    }
    let Some(path) = path else {
        return E_FAIL;
    };
    match shell_item(path) {
        Ok(found) => {
            unsafe { *item = found.into_raw() };
            S_OK
        }
        Err(error) => error.code(),
    }
}

unsafe extern "system" fn get_result_hook(this: *mut c_void, item: *mut *mut c_void) -> HRESULT {
    if let Some(pick) = unsafe { picked(this) } {
        return unsafe { answer_item(item, pick.paths.first().map(PathBuf::as_path)) };
    }
    let original: GetItem = unsafe { real(this, SLOT_GET_RESULT) };
    unsafe { original(this, item) }
}

unsafe extern "system" fn get_current_selection_hook(
    this: *mut c_void,
    item: *mut *mut c_void,
) -> HRESULT {
    if let Some(pick) = unsafe { picked(this) } {
        return unsafe { answer_item(item, pick.paths.first().map(PathBuf::as_path)) };
    }
    let original: GetItem = unsafe { real(this, SLOT_GET_CURRENT_SELECTION) };
    unsafe { original(this, item) }
}

unsafe extern "system" fn get_folder_hook(this: *mut c_void, item: *mut *mut c_void) -> HRESULT {
    if let Some(pick) = unsafe { picked(this) } {
        return unsafe { answer_item(item, pick.paths.first().and_then(|path| path.parent())) };
    }
    let original: GetItem = unsafe { real(this, SLOT_GET_FOLDER) };
    unsafe { original(this, item) }
}

unsafe extern "system" fn get_file_type_index_hook(this: *mut c_void, index: *mut u32) -> HRESULT {
    if let Some(chosen) = unsafe { picked(this) }.and_then(|pick| pick.type_index) {
        if index.is_null() {
            return E_POINTER;
        }
        unsafe { *index = chosen };
        return S_OK;
    }
    let original: GetIndex = unsafe { real(this, SLOT_GET_FILE_TYPE_INDEX) };
    unsafe { original(this, index) }
}

/// Text the caller frees with `CoTaskMemFree`, as the Shell's own getters return it.
fn task_text(text: &str) -> Option<PWSTR> {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let memory = unsafe { CoTaskMemAlloc(wide.len() * size_of::<u16>()) }.cast::<u16>();
    if memory.is_null() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), memory, wide.len()) };
    Some(PWSTR(memory))
}

unsafe extern "system" fn get_file_name_hook(this: *mut c_void, name: *mut PWSTR) -> HRESULT {
    if let Some(pick) = unsafe { picked(this) } {
        if name.is_null() {
            return E_POINTER;
        }
        let text = pick
            .paths
            .first()
            .and_then(|path| path.file_name())
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_default();
        return match task_text(&text) {
            Some(copy) => {
                unsafe { *name = copy };
                S_OK
            }
            None => E_OUTOFMEMORY,
        };
    }
    let original: GetText = unsafe { real(this, SLOT_GET_FILE_NAME) };
    unsafe { original(this, name) }
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
    if let Some(pick) = unsafe { picked(this) } {
        if items.is_null() {
            return E_POINTER;
        }
        return match item_array(&pick.paths) {
            Ok(array) => {
                unsafe { *items = array };
                S_OK
            }
            Err(error) => error.code(),
        };
    }
    let original: GetItem = unsafe { real(this, SLOT_GET_RESULTS) };
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
    /// It makes Save windows.
    save: bool,
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
    let (real, save) = unsafe {
        let factory = &*this.cast::<Factory>();
        (factory.real, factory.save)
    };
    let create: CreateInstance = unsafe { self::real(real, SLOT_CREATE_INSTANCE) };
    let result = unsafe { create(real, outer, interface, object) };
    if result.is_ok() && !interface.is_null() && !object.is_null() && outer.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            if let Some(kind) = dialog_interface(&*interface) {
                let id = identity(*object);
                let mut known = guarded(states());
                // An old dialog's address can come back; it must not leave its answers behind.
                known.remove(&id);
                if known.len() > 64 {
                    known.clear();
                }
                drop(known);
                hook_dialog(*object, kind, save);
            }
        }));
    }
    result
}

/// Puts Firewol's factory in front of Windows' one, or leaves Windows' own in place.
unsafe fn wrap_factory(
    real: *mut c_void,
    interface: &GUID,
    object: *mut *mut c_void,
    save: bool,
) -> HRESULT {
    let factory = Box::into_raw(Box::new(Factory {
        table: &FACTORY_TABLE,
        references: AtomicU32::new(1),
        real,
        save,
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
    if interface.is_null() || object.is_null() {
        return unsafe { system(class, interface, object) };
    }
    let mut real: *mut c_void = std::ptr::null_mut();
    let result = unsafe { system(class, &IClassFactory::IID, &mut real) };
    if result.is_err() {
        return result;
    }
    unsafe { wrap_factory(real, &*interface, object, *requested == FILE_SAVE_DIALOG) }
}

/// Objects keep pointing at tables and functions here, so this file stays loaded.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
