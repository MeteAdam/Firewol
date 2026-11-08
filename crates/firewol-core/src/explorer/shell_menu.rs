//! Windows' own context-menu commands (VS Code, WinRAR, ...) for items or a folder background.
//!
//! Shell extensions load into this process, as they do in Explorer. Every menu lives on one STA
//! thread with an invisible owner window, because `IContextMenu2/3` submenus are filled through
//! window messages. Each interface window has at most one live menu; it ends when the interface
//! closes it, a new one opens, or after a minute, and the extension DLLs are then released.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAP, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, GetDIBits, GetObjectW, HBITMAP,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoFreeUnusedLibrariesEx, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_SFObject, CMF_DEFAULTONLY, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO,
    CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3, ILFindLastID,
    IShellFolder, IShellItem, SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST, SHBindToParent,
    SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SHParseDisplayName, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, CreateWindowExW, DefWindowProcW, DeleteMenu, DestroyMenu, DispatchMessageW,
    GET_MENU_DEFAULT_ITEM_FLAGS, GetCursorPos, GetMenuDefaultItem, GetMenuItemCount,
    GetMenuItemInfoW, GetMessageW, HMENU, KillTimer, MENUITEMINFOW, MF_BYPOSITION, MFS_DISABLED,
    MFS_GRAYED, MFT_SEPARATOR, MIIM_BITMAP, MIIM_CHECKMARKS, MIIM_FTYPE, MIIM_ID, MIIM_STATE,
    MIIM_STRING, MIIM_SUBMENU, MSG, PostMessageW, RegisterClassW, SW_SHOWNORMAL,
    SetForegroundWindow, SetTimer, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM,
    WM_MENUCHAR, WM_NULL, WM_TIMER, WNDCLASSW, WS_POPUP,
};
use windows_core::{HSTRING, Interface, PCSTR, PCWSTR, PSTR, PWSTR, w};

use super::icons::{bitmap_info, encode_png};
use super::modern_menu::Modern;

const FIRST_ID: u32 = 1;
const LAST_ID: u32 = 0x7FFF;
const LIFETIME: Duration = Duration::from_secs(60);
const SWEEP_MS: u32 = 5_000;
const SWEEP_TIMER: usize = 1;
const WM_TASK: u32 = WM_APP + 1;
const MAX_DEPTH: u32 = 4;
const MAX_LABEL: usize = 512;
const MAX_ICON_EDGE: i32 = 128;
const OPEN_WAIT: Duration = Duration::from_secs(5);
const SEE_MASK_UNICODE: u32 = 0x4000;
/// `SEE_MASK_FLAG_LOG_USAGE`: a launch the user asked for, which Windows counts as recent use.
const LOG_USAGE: u32 = 0x0400_0000;
/// Firewol has its own versions of these commands, so its menu leaves them out.
const FIREWOL_VERBS: [&str; 8] = [
    "open",
    "cut",
    "copy",
    "paste",
    "delete",
    "rename",
    "pintohome",
    "copyaspath",
];
/// Removed from the menu entirely: administrator launches go through Firewol's elevation policy.
const ELEVATING_VERBS: [&str; 2] = ["runas", "runasuser"];

#[derive(Clone, Debug, Default)]
pub struct ShellItem {
    pub id: u32,
    pub label: String,
    /// `menu:` key served by [`ShellMenus::icon`].
    pub icon: Option<String>,
    pub children: Vec<ShellItem>,
    pub disabled: bool,
    pub separator: bool,
    pub verb: String,
}

pub enum Target {
    /// Items that share one parent folder.
    Items(Vec<PathBuf>),
    /// The empty area of a folder.
    Background(PathBuf),
}

type Task = Box<dyn FnOnce(HWND) + Send>;
type IconStore = Arc<Mutex<HashMap<String, Arc<Vec<u8>>>>>;

struct Worker {
    tasks: Sender<Task>,
    window: isize,
}

#[derive(Default)]
pub struct ShellMenus {
    worker: OnceLock<Result<Worker, String>>,
    icons: IconStore,
}

#[derive(Clone)]
struct Handlers {
    second: Option<IContextMenu2>,
    third: Option<IContextMenu3>,
}

impl Handlers {
    fn forward(&self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if let Some(handler) = &self.third {
            let mut result = LRESULT(0);
            unsafe { handler.HandleMenuMsg2(message, wparam, lparam, Some(&mut result)) }.ok()?;
            return Some(result);
        }
        let handler = self.second.as_ref()?;
        unsafe { handler.HandleMenuMsg(message, wparam, lparam) }.ok()?;
        Some(LRESULT(0))
    }
}

struct Session {
    menu: IContextMenu,
    handlers: Handlers,
    popup: HMENU,
    owner: isize,
    opened: Instant,
    generation: u64,
    commands: HashSet<u32>,
    modern: Modern,
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.popup);
        }
    }
}

thread_local! {
    static TASKS: RefCell<Option<Receiver<Task>>> = const { RefCell::new(None) };
    static SESSIONS: RefCell<HashMap<String, Session>> = RefCell::new(HashMap::new());
    /// Receives the owner window's menu messages while a native menu is open.
    static TRACKING: RefCell<Option<Handlers>> = const { RefCell::new(None) };
    /// True while a task runs: Shell dialogs pump messages, and tasks must not nest inside them.
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static STORE: RefCell<Option<IconStore>> = const { RefCell::new(None) };
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ShellMenus {
    /// Started on the first menu, so an app that never opens one pays nothing.
    fn worker(&self) -> Result<&Worker> {
        self.worker
            .get_or_init(|| start(self.icons.clone()).map_err(|error| format!("{error:#}")))
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    fn call<T: Send + 'static>(
        &self,
        wait: Option<Duration>,
        task: impl FnOnce(HWND) -> T + Send + 'static,
    ) -> Result<T> {
        let worker = self.worker()?;
        let (reply, result) = mpsc::sync_channel(1);
        worker
            .tasks
            .send(Box::new(move |window| {
                let _ = reply.send(task(window));
            }))
            .ok()
            .context("The Shell menu worker stopped")?;
        unsafe {
            PostMessageW(
                Some(HWND(worker.window as _)),
                WM_TASK,
                WPARAM(0),
                LPARAM(0),
            )?;
        }
        match wait {
            Some(wait) => result
                .recv_timeout(wait)
                .ok()
                .context("A Shell extension did not answer"),
            None => result.recv().ok().context("The Shell menu worker stopped"),
        }
    }

    /// Builds the menu for `target` and returns the commands Firewol should show.
    pub fn open(&self, label: &str, owner: isize, target: Target) -> Result<Vec<ShellItem>> {
        let label = label.to_owned();
        self.call(Some(OPEN_WAIT), move |window| {
            open(window, label, owner, target)
        })?
    }

    pub fn invoke(&self, label: &str, id: u32, point: (i32, i32)) -> Result<()> {
        let label = label.to_owned();
        self.call(None, move |_| invoke(&label, id, point))?
    }

    /// Shows the same menu natively at the cursor and runs what the user picks.
    pub fn native(&self, label: &str) -> Result<()> {
        let label = label.to_owned();
        self.call(None, move |window| native(window, &label))?
    }

    /// Runs the default Windows command of `paths`, as a double-click does in Explorer.
    pub fn open_default(&self, owner: isize, paths: Vec<PathBuf>) -> Result<()> {
        self.call(None, move |window| {
            by_folder(paths)
                .into_iter()
                .try_for_each(|group| open_group(window, owner, group))
        })?
    }

    pub fn close(&self, label: &str) {
        let label = label.to_owned();
        if self.worker.get().is_some() {
            let _ = self.call(Some(OPEN_WAIT), move |_| finish(take(&label)));
        }
    }

    pub fn icon(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        lock(&self.icons).get(key).cloned()
    }
}

fn start(icons: IconStore) -> Result<Worker> {
    let (tasks, receiver) = mpsc::channel::<Task>();
    let (ready, window) = mpsc::sync_channel::<Result<isize, String>>(1);
    std::thread::Builder::new()
        .name("shell-menus".into())
        .spawn(move || {
            let created = unsafe { create_window() };
            match created {
                Ok(window) => {
                    TASKS.set(Some(receiver));
                    STORE.set(Some(icons));
                    let _ = ready.send(Ok(window.0 as isize));
                    pump();
                }
                Err(error) => {
                    let _ = ready.send(Err(format!("{error:#}")));
                }
            }
        })
        .context("Start the Shell menu worker")?;
    let window = window
        .recv()
        .context("The Shell menu worker stopped")?
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(Worker { tasks, window })
}

/// A native popup must be the foreground owner to close when the user clicks elsewhere, which a
/// message-only window can never be; this window is a popup that is simply never shown.
unsafe fn create_window() -> Result<HWND> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("Start the Shell menu apartment")?;
        let module = GetModuleHandleW(None)?;
        let class = w!("FirewolShellMenu");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        });
        Ok(CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        )?)
    }
}

fn pump() {
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TASK => {
            run_tasks(window);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == SWEEP_TIMER => {
            sweep(window);
            LRESULT(0)
        }
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR => {
            let forwarded = TRACKING.with_borrow(|handlers| {
                handlers
                    .as_ref()
                    .and_then(|handlers| handlers.forward(message, wparam, lparam))
            });
            forwarded.unwrap_or_else(|| unsafe { DefWindowProcW(window, message, wparam, lparam) })
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn run_tasks(window: HWND) {
    if BUSY.get() {
        return;
    }
    BUSY.set(true);
    while let Some(task) =
        TASKS.with_borrow(|tasks| tasks.as_ref().and_then(|tasks| tasks.try_recv().ok()))
    {
        task(window);
    }
    BUSY.set(false);
}

fn take(label: &str) -> Option<Session> {
    SESSIONS.with_borrow_mut(|sessions| sessions.remove(label))
}

/// Ends a menu: destroys it, forgets its icons and lets idle extension DLLs unload.
fn finish(session: Option<Session>) {
    let Some(session) = session else {
        return;
    };
    let prefix = format!("menu:{}:", session.generation);
    drop(session);
    STORE.with_borrow(|store| {
        if let Some(store) = store {
            lock(store).retain(|key, _| !key.starts_with(&prefix));
        }
    });
    unsafe { CoFreeUnusedLibrariesEx(0, None) };
}

fn sweep(window: HWND) {
    let expired: Vec<String> = SESSIONS.with_borrow(|sessions| {
        sessions
            .iter()
            .filter(|(_, session)| session.opened.elapsed() >= LIFETIME)
            .map(|(label, _)| label.clone())
            .collect()
    });
    for label in expired {
        finish(take(&label));
    }
    if SESSIONS.with_borrow(HashMap::is_empty) {
        unsafe {
            let _ = KillTimer(Some(window), SWEEP_TIMER);
        }
    }
}

pub(super) struct Pidl(pub(super) *mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as _)) };
    }
}

pub(super) fn parse(path: &Path) -> Result<Pidl> {
    let mut pidl = std::ptr::null_mut();
    unsafe {
        SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None)
            .with_context(|| format!("Find {} in the Shell", path.display()))?;
    }
    Ok(Pidl(pidl))
}

fn context_menu(target: &Target, owner: HWND) -> Result<IContextMenu> {
    unsafe {
        match target {
            Target::Background(folder) => {
                let item: IShellItem =
                    SHCreateItemFromParsingName(&HSTRING::from(folder.as_os_str()), None)?;
                let shell: IShellFolder = item.BindToHandler(None, &BHID_SFObject)?;
                Ok(shell.CreateViewObject(owner)?)
            }
            Target::Items(paths) => {
                let first = paths.first().context("Select at least one item")?;
                ensure!(
                    paths.iter().all(|path| path.parent() == first.parent()),
                    "These items are in different folders"
                );
                let items = paths
                    .iter()
                    .map(|path| parse(path))
                    .collect::<Result<Vec<_>>>()?;
                let parent: IShellFolder = SHBindToParent(items[0].0, None)?;
                let children: Vec<*const ITEMIDLIST> = items
                    .iter()
                    .map(|item| ILFindLastID(item.0).cast_const())
                    .collect();
                Ok(parent.GetUIObjectOf(owner, &children, None)?)
            }
        }
    }
}

fn open(window: HWND, label: String, owner: isize, target: Target) -> Result<Vec<ShellItem>> {
    finish(take(&label));
    let menu = context_menu(&target, window)?;
    let popup = unsafe { CreatePopupMenu()? };
    let handlers = Handlers {
        second: menu.cast().ok(),
        third: menu.cast().ok(),
    };
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let mut session = Session {
        menu,
        handlers,
        popup,
        owner,
        opened: Instant::now(),
        generation,
        commands: HashSet::new(),
        modern: Modern::default(),
    };
    unsafe {
        session
            .menu
            .QueryContextMenu(popup, 0, FIRST_ID, LAST_ID, CMF_NORMAL)
            .ok()
            .context("Read the Windows menu")?;
    }
    let mut next_icon = 0;
    let classic = walk(&mut session, popup, 0, &mut next_icon);
    // Windows 11 lists its packaged commands above the classic ones.
    let (modern, mut items) = Modern::load(&target, &mut session.commands);
    session.modern = modern;
    items.extend(classic);
    let items = tidy(items);
    SESSIONS.with_borrow_mut(|sessions| sessions.insert(label, session));
    unsafe { SetTimer(Some(window), SWEEP_TIMER, SWEEP_MS, None) };
    Ok(items)
}

/// The command's language-independent name, when its handler has one.
fn verb(session: &Session, id: u32) -> String {
    if !(FIRST_ID..=LAST_ID).contains(&id) {
        return String::new();
    }
    let mut name = [0u16; 128];
    let found = unsafe {
        session.menu.GetCommandString(
            (id - FIRST_ID) as usize,
            GCS_VERBW,
            None,
            PSTR(name.as_mut_ptr().cast()),
            name.len() as u32,
        )
    };
    if found.is_err() {
        return String::new();
    }
    let end = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end]).to_lowercase()
}

/// Drops the accelerator hint after a tab and turns `&&` into `&` and other `&` into nothing.
pub(super) fn clean_label(text: &str) -> String {
    let text = text.split('\t').next().unwrap_or_default();
    let mut label = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '&' {
            label.push(character);
        } else if characters.peek() == Some(&'&') {
            label.push('&');
            characters.next();
        }
    }
    label.trim().to_owned()
}

/// Walks one menu level from its end, so removing an item keeps earlier positions valid.
fn walk(session: &mut Session, menu: HMENU, depth: u32, next_icon: &mut u32) -> Vec<ShellItem> {
    let count = unsafe { GetMenuItemCount(Some(menu)) };
    let mut items = Vec::new();
    for position in (0..count.max(0) as u32).rev() {
        let mut text = [0u16; MAX_LABEL];
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE
                | MIIM_ID
                | MIIM_STATE
                | MIIM_STRING
                | MIIM_SUBMENU
                | MIIM_BITMAP
                | MIIM_CHECKMARKS,
            dwTypeData: PWSTR(text.as_mut_ptr()),
            cch: (MAX_LABEL - 1) as u32,
            ..Default::default()
        };
        if unsafe { GetMenuItemInfoW(menu, position, true, &mut info) }.is_err() {
            continue;
        }
        if info.fType.contains(MFT_SEPARATOR) {
            items.push(ShellItem {
                separator: true,
                ..Default::default()
            });
            continue;
        }
        let name = verb(session, info.wID);
        if ELEVATING_VERBS.contains(&name.as_str()) {
            unsafe {
                let _ = DeleteMenu(menu, position, MF_BYPOSITION);
            }
            continue;
        }
        if FIREWOL_VERBS.contains(&name.as_str()) {
            continue;
        }
        let label = clean_label(&String::from_utf16_lossy(
            &text[..(info.cch as usize).min(MAX_LABEL - 1)],
        ));
        let mut children = Vec::new();
        if !info.hSubMenu.is_invalid() && depth < MAX_DEPTH {
            session.handlers.forward(
                WM_INITMENUPOPUP,
                WPARAM(info.hSubMenu.0 as usize),
                LPARAM(position as isize),
            );
            children = tidy(walk(session, info.hSubMenu, depth + 1, next_icon));
        }
        // An owner-drawn item without text cannot be shown, and neither can an empty submenu.
        if label.is_empty() || (!info.hSubMenu.is_invalid() && children.is_empty()) {
            continue;
        }
        if info.hSubMenu.is_invalid() {
            session.commands.insert(info.wID);
        }
        let bitmap = if info.hbmpItem.is_invalid() {
            info.hbmpUnchecked
        } else {
            info.hbmpItem
        };
        let icon = store_icon(bitmap, session.generation, next_icon);
        items.push(ShellItem {
            id: info.wID,
            label,
            icon,
            children,
            disabled: info.fState.0 & (MFS_DISABLED.0 | MFS_GRAYED.0) != 0,
            separator: false,
            verb: name,
        });
    }
    items.reverse();
    items
}

/// Removes separators that filtering left at the ends or side by side, and the second copy of
/// a command that registers both as a packaged handler and as a classic extension.
fn tidy(items: Vec<ShellItem>) -> Vec<ShellItem> {
    let mut result: Vec<ShellItem> = Vec::with_capacity(items.len());
    let mut labels = HashSet::new();
    for item in items {
        let repeated = !item.separator && !labels.insert(item.label.clone());
        if repeated || (item.separator && result.last().is_none_or(|last| last.separator)) {
            continue;
        }
        result.push(item);
    }
    if result.last().is_some_and(|last| last.separator) {
        result.pop();
    }
    result
}

fn store_icon(bitmap: HBITMAP, generation: u64, next: &mut u32) -> Option<String> {
    // Values up to 11 are HBMMENU_* constants, and -1 asks for owner drawing.
    if (bitmap.0 as isize) <= 11 {
        return None;
    }
    let png = bitmap_png(bitmap)?;
    let key = format!("menu:{generation}:{next}");
    *next += 1;
    STORE.with_borrow(|store| {
        if let Some(store) = store {
            lock(store).insert(key.clone(), Arc::new(png));
        }
    });
    Some(key)
}

fn bitmap_png(bitmap: HBITMAP) -> Option<Vec<u8>> {
    let mut details = BITMAP::default();
    let copied = unsafe {
        GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&raw mut details).cast()),
        )
    };
    let (width, height) = (details.bmWidth, details.bmHeight.abs());
    if copied == 0 || width <= 0 || height <= 0 || width > MAX_ICON_EDGE || height > MAX_ICON_EDGE {
        return None;
    }
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let mut header = bitmap_info(width as u32);
    header.bmiHeader.biHeight = -height;
    let dc = unsafe { CreateCompatibleDC(None) };
    let rows = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        let _ = DeleteDC(dc);
    }
    if rows != height {
        return None;
    }
    // A bitmap without alpha reads as fully transparent, but it is opaque.
    if pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0) {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    encode_png(&mut pixels, width as u32, height as u32).ok()
}

fn run_command(session: &Session, id: u32, point: (i32, i32)) -> Result<()> {
    if let Some(result) = session.modern.invoke(id) {
        return result.context("Windows could not run this command");
    }
    let offset = (id - FIRST_ID) as usize;
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: SEE_MASK_UNICODE | CMIC_MASK_PTINVOKE,
        hwnd: HWND(session.owner as _),
        lpVerb: PCSTR(offset as *const u8),
        lpVerbW: PCWSTR(offset as *const u16),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: POINT {
            x: point.0,
            y: point.1,
        },
        ..Default::default()
    };
    unsafe {
        session
            .menu
            .InvokeCommand((&raw const info).cast::<CMINVOKECOMMANDINFO>())
    }
    .context("Windows could not run this command")
}

/// One Shell menu covers one folder, and search results can come from many.
fn by_folder(paths: Vec<PathBuf>) -> Vec<Vec<PathBuf>> {
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for path in paths {
        match groups
            .iter_mut()
            .find(|group| group[0].parent() == path.parent())
        {
            Some(group) => group.push(path),
            None => groups.push(vec![path]),
        }
    }
    groups
}

/// Asks Windows for the items' default command only, the way Explorer handles a double-click, so
/// the result follows the user's default apps and handlers that replace the plain "open" verb.
fn open_group(window: HWND, owner: isize, paths: Vec<PathBuf>) -> Result<()> {
    let folder = paths[0]
        .parent()
        .context("This item has no folder")?
        .to_owned();
    let menu = context_menu(&Target::Items(paths.clone()), window)?;
    let popup = unsafe { CreatePopupMenu()? };
    let result = invoke_default(&menu, popup, owner, &folder);
    unsafe {
        let _ = DestroyMenu(popup);
    }
    match result? {
        true => Ok(()),
        false => paths
            .iter()
            .try_for_each(|path| open_without_menu(owner, path)),
    }
}

/// Returns false when the menu has no default command to run.
fn invoke_default(menu: &IContextMenu, popup: HMENU, owner: isize, folder: &Path) -> Result<bool> {
    unsafe {
        menu.QueryContextMenu(popup, 0, FIRST_ID, LAST_ID, CMF_DEFAULTONLY)
            .ok()
            .context("Read the Windows menu")?;
    }
    let id = unsafe { GetMenuDefaultItem(popup, 0, GET_MENU_DEFAULT_ITEM_FLAGS(0)) };
    if !(FIRST_ID..=LAST_ID).contains(&id) {
        return Ok(false);
    }
    let offset = (id - FIRST_ID) as usize;
    let directory = HSTRING::from(folder.as_os_str());
    // No zone-check override: Windows keeps warning about files that came from the internet.
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: SEE_MASK_UNICODE | LOG_USAGE,
        hwnd: HWND(owner as _),
        lpVerb: PCSTR(offset as *const u8),
        lpVerbW: PCWSTR(offset as *const u16),
        lpDirectoryW: PCWSTR(directory.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { menu.InvokeCommand((&raw const info).cast::<CMINVOKECOMMANDINFO>()) }
        .context("Windows could not open this item")?;
    Ok(true)
}

/// For a type without a default command: any verb Windows finds, else its app picker.
fn open_without_menu(owner: isize, path: &Path) -> Result<()> {
    let file = HSTRING::from(path.as_os_str());
    let execute = |verb: PCWSTR, flags: u32| {
        let mut request = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST | flags,
            hwnd: HWND(owner as _),
            lpVerb: verb,
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut request) }
    };
    execute(PCWSTR::null(), SEE_MASK_FLAG_NO_UI)
        .or_else(|_| execute(w!("openas"), 0))
        .context("Windows could not open this item")
}

fn invoke(label: &str, id: u32, point: (i32, i32)) -> Result<()> {
    let session = take(label).context("This menu is no longer open")?;
    let allowed = session.commands.contains(&id);
    let result = if allowed {
        run_command(&session, id, point)
    } else {
        Err(anyhow::anyhow!("This command is not available"))
    };
    finish(Some(session));
    result
}

fn native(window: HWND, label: &str) -> Result<()> {
    let session = take(label).context("This menu is no longer open")?;
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point)?;
        let _ = SetForegroundWindow(window);
    }
    TRACKING.set(Some(session.handlers.clone()));
    let chosen = unsafe {
        TrackPopupMenuEx(
            session.popup,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            point.x,
            point.y,
            window,
            None,
        )
    };
    TRACKING.set(None);
    unsafe {
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
    }
    let chosen = chosen.0 as u32;
    let result = if chosen == 0 {
        Ok(())
    } else {
        run_command(&session, chosen, (point.x, point.y))
    };
    finish(Some(session));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(items: &[ShellItem]) -> Vec<&ShellItem> {
        items
            .iter()
            .flat_map(|item| std::iter::once(item).chain(flatten(&item.children)))
            .collect()
    }

    #[test]
    fn a_text_file_has_commands_but_never_an_elevation_verb() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let file = fixture.path().join("note.txt");
        std::fs::write(&file, "x")?;
        let menus = ShellMenus::default();
        let items = menus.open("test", 0, Target::Items(vec![file]))?;
        assert!(items.iter().any(|item| !item.separator), "{items:?}");
        for item in flatten(&items) {
            assert!(!ELEVATING_VERBS.contains(&item.verb.as_str()), "{item:?}");
            assert!(!FIREWOL_VERBS.contains(&item.verb.as_str()), "{item:?}");
        }
        menus.close("test");
        Ok(())
    }

    #[test]
    fn a_folder_background_has_commands_and_closing_is_clean() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let menus = ShellMenus::default();
        for _ in 0..3 {
            let items = menus.open("test", 0, Target::Background(fixture.path().to_owned()))?;
            assert!(items.iter().any(|item| !item.separator), "{items:?}");
        }
        menus.close("test");
        assert!(lock(&menus.icons).is_empty());
        assert!(menus.invoke("test", 1, (0, 0)).is_err());
        Ok(())
    }

    #[test]
    fn opening_a_file_runs_its_default_command() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let script = fixture.path().join("open me.cmd");
        let marker = fixture.path().join("opened.txt");
        std::fs::write(&script, "@echo opened> \"%~dp0opened.txt\"\r\n")?;
        let menus = ShellMenus::default();
        menus.open_default(0, vec![script])?;
        let started = Instant::now();
        while !marker.exists() {
            ensure!(
                started.elapsed() < Duration::from_secs(20),
                "The script did not run"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    #[test]
    fn search_results_open_one_folder_at_a_time() {
        let path = |text: &str| PathBuf::from(text);
        let groups = by_folder(vec![path(r"C:\a\1"), path(r"C:\b\2"), path(r"C:\a\3")]);
        assert_eq!(
            groups,
            [
                vec![path(r"C:\a\1"), path(r"C:\a\3")],
                vec![path(r"C:\b\2")]
            ]
        );
    }

    #[test]
    fn labels_lose_their_mnemonics_and_accelerators() {
        assert_eq!(clean_label("&Open\tCtrl+O"), "Open");
        assert_eq!(clean_label("Save && &Exit"), "Save & Exit");
    }
}
