#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod contract;
mod folders;
mod icon_protocol;
mod settings;
mod transfers;
mod volumes;

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::Context as _;
use contract::{
    ClipboardState, DriveSpace, DriveView, FolderStyleView, IndexState, ItemRef, Navigation,
    OperationDone, Page, Pin, SearchCount, SearchRequest, Segment, SessionState, SettingsView,
    ShellMenuItem, Startup, Step, Subfolders, TransferStatus, VolumeState,
};
use firewol_core::explorer::icons::{self, Icons};
use firewol_core::explorer::shell_menu::{ShellItem, ShellMenus, Target};
use firewol_core::explorer::{
    ItemId, MAX_TRANSFER_ITEMS, MenuAction, Operation, Session, actions, custom_icon, data_dir,
    display_path, home,
};
use folders::{Folders, Notice, Notify, item_id, item_ref, lock, segment, text};
use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window,
    WindowEvent,
};
use transfers::Transfers;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    ICoreWebView2_19,
};
use windows::Win32::Foundation::HWND;
use windows_core::Interface;

static STARTED: OnceLock<Instant> = OnceLock::new();
// Every window shares one WebView2 environment, so they must all use the same arguments.
// Low-end device mode caps Chromium's raster and cache pools; process-per-site lets all
// windows share one renderer. Together they keep four windows below the M4 memory budget.
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
    --enable-low-end-device-mode --process-per-site";

struct App {
    folders: Arc<Folders>,
    icons: Arc<Icons>,
    menus: Arc<ShellMenus>,
    windows: Mutex<HashMap<String, WindowRequest>>,
    next_window: AtomicU32,
    window_count: AtomicU32,
    report: Option<PathBuf>,
}

/// The clipboard, file operations and settings shared by every window.
struct Files(Arc<Session>);

#[derive(Clone)]
struct WindowRequest {
    path: String,
    split: bool,
    requested: Instant,
    adopt: Option<contract::TabSnapshot>,
}

#[tauri::command]
async fn startup(
    window: Window,
    state: State<'_, App>,
    files: State<'_, Files>,
) -> Result<Startup, String> {
    let label = window.label().to_owned();
    let request = lock(&state.windows).get(&label).cloned();
    let main = request.is_none();
    let path = match &request {
        Some(request) => request.path.clone(),
        None => match std::env::var_os("FIREWOL_OPEN").filter(|value| !value.is_empty()) {
            Some(path) => path.to_string_lossy().into_owned(),
            None => {
                let files = files.0.clone();
                blocking(move || {
                    let last = files
                        .settings()
                        .last_folder
                        .and_then(|path| firewol_core::explorer::resolve_directory(&path).ok());
                    Ok(text(&last.map(Ok).unwrap_or_else(home)?))
                })
                .await?
            }
        },
    };
    let started = request.as_ref().map_or_else(
        || *STARTED.get_or_init(Instant::now),
        |request| request.requested,
    );
    let session = {
        let files = files.0.clone();
        blocking(move || Ok(session_state(&files))).await?
    };
    Ok(Startup {
        label,
        adopt: request.as_ref().and_then(|request| request.adopt.clone()),
        path,
        split: request.map_or_else(
            || std::env::var_os("FIREWOL_SPLIT").is_some_and(|value| !value.is_empty()),
            |r| r.split,
        ),
        report: state.report.is_some(),
        self_test: state.report.is_some()
            && std::env::var("FIREWOL_SELF_TEST").is_ok_and(|value| value == "1"),
        extra_windows: if main {
            setting("FIREWOL_SPIKE_WINDOWS", 1).saturating_sub(1)
        } else {
            0
        },
        idle_ms: setting("FIREWOL_SPIKE_IDLE_MS", 10_000),
        combined: state
            .report
            .as_ref()
            .and(std::env::var_os("FIREWOL_SPIKE_COMBINED"))
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string_lossy().into_owned()),
        session,
        locale: user_locale(),
        since_start_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

fn user_locale() -> String {
    let mut name = [0u16; 85];
    let length = unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut name) };
    usize::try_from(length)
        .ok()
        .filter(|&length| length > 1)
        .map_or_else(
            || "en-US".into(),
            |length| String::from_utf16_lossy(&name[..length - 1]),
        )
}

#[tauri::command]
async fn navigate(
    window: Window,
    state: State<'_, App>,
    files: State<'_, Files>,
    pane: String,
    path: Option<String>,
    step: Step,
    sort: contract::SortView,
) -> Result<Navigation, String> {
    let folders = state.folders.clone();
    let notify = notifier(window.app_handle());
    let owner = window.label().to_owned();
    let files = files.0.clone();
    blocking(move || {
        let navigation = folders.navigate(&owner, &pane, path.as_deref(), step, sort, notify)?;
        let path = PathBuf::from(&navigation.folder.path);
        if files.settings().last_folder.as_ref() != Some(&path) {
            files.update_settings(|settings| settings.last_folder = Some(path))?;
        }
        Ok(navigation)
    })
    .await
}

#[tauri::command]
fn close_pane(window: Window, state: State<'_, App>, pane: String) {
    state.folders.close_pane(window.label(), &pane);
}

#[tauri::command]
fn suspend_pane(window: Window, state: State<'_, App>, pane: String) -> Result<(), String> {
    state
        .folders
        .suspend(window.label(), &pane)
        .map_err(message)
}

#[tauri::command]
async fn list_page(
    state: State<'_, App>,
    id: u64,
    offset: u32,
    sort: contract::SortView,
) -> Result<Page, String> {
    let folders = state.folders.clone();
    blocking(move || folders.page(id, offset, sort)).await
}

#[tauri::command]
async fn folder_progress(
    state: State<'_, App>,
    id: u64,
) -> Result<contract::FolderProgress, String> {
    let folders = state.folders.clone();
    blocking(move || folders.progress(id)).await
}

#[tauri::command]
async fn locate_selection(
    state: State<'_, App>,
    id: u64,
    items: Vec<ItemRef>,
) -> Result<Vec<Option<u32>>, String> {
    let folders = state.folders.clone();
    blocking(move || folders.locate(id, &items)).await
}

#[tauri::command]
async fn start_index(app: AppHandle, state: State<'_, App>, id: u64) -> Result<IndexState, String> {
    let folders = state.folders.clone();
    let notify = notifier(&app);
    blocking(move || folders.index(id, notify)).await
}

#[tauri::command]
async fn search(state: State<'_, App>, request: SearchRequest) -> Result<Page, String> {
    let folders = state.folders.clone();
    blocking(move || folders.search(request)).await
}

#[tauri::command]
async fn search_count(
    state: State<'_, App>,
    request: SearchRequest,
) -> Result<SearchCount, String> {
    let folders = state.folders.clone();
    blocking(move || folders.search_count(request)).await
}

/// The drive letter of `path` when volume search can index it (a local NTFS volume).
#[tauri::command]
async fn ntfs_drive(path: String) -> Result<Option<String>, String> {
    blocking(move || {
        Ok(firewol_core::explorer::volume::ntfs_drive(Path::new(&path)).map(String::from))
    })
    .await
}

#[tauri::command]
fn volume_states(state: State<'_, App>) -> Vec<VolumeState> {
    state.folders.volumes.states()
}

/// Starts the NTFS index for `drive`; Windows asks for administrator approval for the helper.
#[tauri::command]
async fn start_volume_index(
    app: AppHandle,
    state: State<'_, App>,
    drive: String,
) -> Result<VolumeState, String> {
    let volumes = state.folders.volumes.clone();
    let notify: volumes::VolumeNotify = Arc::new(move |volume| {
        let _ = app.emit(contract::VOLUME_STATE, volume);
    });
    blocking(move || {
        let drive = volumes::drive(&drive)?;
        let state = volumes.start(drive, notify.clone())?;
        notify(state.clone());
        Ok(state)
    })
    .await
}

#[tauri::command]
fn stop_volume_index(app: AppHandle, state: State<'_, App>, drive: String) -> Result<(), String> {
    let letter = volumes::drive(&drive).map_err(message)?;
    state.folders.volumes.stop(letter);
    let _ = app.emit(
        contract::VOLUME_STATE,
        VolumeState {
            drive: letter.to_string(),
            ready: false,
            running: false,
            count: 0,
            error: None,
        },
    );
    Ok(())
}

// Session state opens each pin to learn its identity, so these commands stay off the main thread.
#[tauri::command]
async fn session(files: State<'_, Files>) -> Result<SessionState, String> {
    let files = files.0.clone();
    blocking(move || Ok(session_state(&files))).await
}

#[tauri::command]
async fn set_clipboard(
    app: AppHandle,
    files: State<'_, Files>,
    items: Vec<ItemRef>,
    cut: bool,
) -> Result<SessionState, String> {
    let items = items
        .iter()
        .map(item_id)
        .collect::<anyhow::Result<_>>()
        .map_err(message)?;
    let files = files.0.clone();
    blocking(move || {
        files.set_clipboard(items, cut)?;
        Ok(broadcast(&app, &files))
    })
    .await
}

#[tauri::command]
fn paste(
    app: AppHandle,
    files: State<'_, Files>,
    transfers: State<'_, Arc<Transfers>>,
    into: ItemRef,
) -> Result<(), String> {
    let into = item_id(&into).map_err(message)?;
    watched(&app, &files, &transfers, |session| session.paste(into))
}

/// Submits a transfer or undo with its progress view prepared first.
fn watched(
    app: &AppHandle,
    files: &Files,
    transfers: &Arc<Transfers>,
    submit: impl FnOnce(&Session) -> anyhow::Result<firewol_core::explorer::TransferControl>,
) -> Result<(), String> {
    let generation = transfers.prepare();
    match submit(&files.0) {
        Ok(control) => {
            transfers.attach(generation, control, files.0.clone(), transfer_emitter(app));
            Ok(())
        }
        Err(error) => {
            transfers.abandon(generation);
            Err(message(error))
        }
    }
}

fn transfer_emitter(app: &AppHandle) -> transfers::Emit {
    let app = app.clone();
    Arc::new(move |status| {
        let _ = app.emit(contract::TRANSFER_PROGRESS, status);
    })
}

#[tauri::command]
fn transfer(
    app: AppHandle,
    files: State<'_, Files>,
    transfers: State<'_, Arc<Transfers>>,
    items: Vec<ItemRef>,
    into: ItemRef,
    move_items: bool,
) -> Result<(), String> {
    let items = items.iter().map(item_id).collect::<anyhow::Result<_>>();
    let into = item_id(&into).map_err(message)?;
    let items = items.map_err(message)?;
    watched(&app, &files, &transfers, |session| {
        session.transfer(items, into, move_items)
    })
}

#[tauri::command]
fn rename(files: State<'_, Files>, item: ItemRef, name: String) -> Result<(), String> {
    let item = item_id(&item).map_err(message)?;
    files
        .0
        .run(Operation::Rename {
            item,
            name: name.into(),
        })
        .map_err(message)
}

/// Recycles the selection as one batch; a single undo restores every item.
#[tauri::command]
fn recycle(files: State<'_, Files>, items: Vec<ItemRef>) -> Result<(), String> {
    let items = items
        .iter()
        .map(item_id)
        .collect::<anyhow::Result<_>>()
        .map_err(message)?;
    files.0.run(Operation::Recycle { items }).map_err(message)
}

/// The folders inside `path`, for a breadcrumb chevron; at most 256 in natural order.
#[tauri::command]
async fn subfolders(path: String) -> Result<Subfolders, String> {
    blocking(move || {
        let (folders, more) = firewol_core::explorer::subfolders(Path::new(&path), 256)?;
        Ok(Subfolders {
            folders: folders
                .iter()
                .map(|folder| Segment {
                    icon: icons::folder_icon(folder),
                    ..segment(folder)
                })
                .collect(),
            more,
        })
    })
    .await
}

/// Every drive with readable media, for the sidebar.
#[tauri::command]
async fn drives() -> Result<Vec<DriveView>, String> {
    blocking(|| {
        Ok(firewol_core::explorer::drives()
            .into_iter()
            .map(|drive| DriveView {
                name: drive.name,
                path: text(&drive.path),
                item: ItemId::at(drive.path.clone()).ok().as_ref().map(item_ref),
                icon: icons::folder_icon(&drive.path),
            })
            .filter(|drive| drive.item.is_some())
            .collect())
    })
    .await
}

/// Free space of the drive holding `path`, for the status bar.
#[tauri::command]
async fn drive_space(path: String) -> Result<DriveSpace, String> {
    blocking(move || {
        let (free, total) = firewol_core::explorer::free_space(Path::new(&path))?;
        Ok(DriveSpace { free, total })
    })
    .await
}

#[tauri::command]
fn undo(
    app: AppHandle,
    files: State<'_, Files>,
    transfers: State<'_, Arc<Transfers>>,
) -> Result<(), String> {
    watched(&app, &files, &transfers, Session::undo)
}

/// The running transfer, or the last one's result.
#[tauri::command]
fn transfer_status(transfers: State<'_, Arc<Transfers>>) -> Option<TransferStatus> {
    transfers.current()
}

#[tauri::command]
fn pause_transfer(files: State<'_, Files>) {
    if let Some(control) = files.0.control() {
        control.toggle_pause();
    }
}

#[tauri::command]
fn cancel_transfer(files: State<'_, Files>) {
    if let Some(control) = files.0.control() {
        control.cancel();
    }
}

#[tauri::command]
async fn toggle_pin(
    app: AppHandle,
    files: State<'_, Files>,
    path: String,
) -> Result<SessionState, String> {
    let files = files.0.clone();
    blocking(move || {
        files.toggle_pin(Path::new(&path))?;
        Ok(broadcast(&app, &files))
    })
    .await
}

/// Moves a pin before the pin `before`, or to the end when `before` is `null`.
#[tauri::command]
async fn move_pin(
    app: AppHandle,
    files: State<'_, Files>,
    path: String,
    before: Option<String>,
) -> Result<SessionState, String> {
    let files = files.0.clone();
    blocking(move || {
        files.move_pin(Path::new(&path), before.as_deref().map(Path::new))?;
        Ok(broadcast(&app, &files))
    })
    .await
}

#[tauri::command]
async fn get_settings(files: State<'_, Files>) -> Result<contract::SettingsView, String> {
    let files = files.0.clone();
    blocking(move || settings::view(&files.settings())).await
}

#[tauri::command]
async fn update_settings(
    app: AppHandle,
    files: State<'_, Files>,
    patch: contract::SettingsPatch,
) -> Result<contract::SettingsView, String> {
    let files = files.0.clone();
    blocking(move || {
        let view = settings::update(&files, patch)?;
        app.emit(contract::SETTINGS_CHANGED, &view)?;
        Ok(view)
    })
    .await
}

/// Saves or clears a folder's style and sends the new settings to every window.
#[tauri::command]
async fn set_folder_style(
    app: AppHandle,
    files: State<'_, Files>,
    folder: ItemRef,
    style: Option<FolderStyleView>,
) -> Result<SettingsView, String> {
    let files = files.0.clone();
    let folder = item_id(&folder).map_err(message)?;
    blocking(move || {
        let view = settings::set_folder_style(&files, &folder, style)?;
        app.emit(contract::SETTINGS_CHANGED, &view)?;
        Ok(view)
    })
    .await
}

#[tauri::command]
fn operation_busy(files: State<'_, Files>) -> bool {
    files.0.busy()
}

/// The window handle travels as an integer because `HWND` is not `Send`.
fn owner(window: &Window) -> Result<isize, String> {
    Ok(window.hwnd().map_err(message)?.0 as isize)
}

/// Runs the stored custom action at `index`. The executable is looked up in the saved menu, never
/// taken from the interface; `executable` only confirms the menu did not change since it was shown.
#[tauri::command]
async fn run_menu_action(
    window: Window,
    files: State<'_, Files>,
    index: usize,
    executable: String,
    target: contract::ItemRef,
) -> Result<(), String> {
    let files = files.0.clone();
    let owner = owner(&window)?;
    let target = item_id(&target).map_err(message)?;
    blocking(move || {
        let settings = files.settings();
        let entry = settings
            .menu
            .get(index)
            .filter(|entry| entry.visible)
            .context("This menu action is no longer available")?;
        let MenuAction::Custom {
            executable: stored,
            administrator,
        } = &entry.action
        else {
            anyhow::bail!("This menu entry is not a custom action");
        };
        anyhow::ensure!(
            *stored == executable,
            "The menu changed. Open it again and retry."
        );
        actions::run_custom(
            Path::new(stored),
            *administrator,
            &target,
            &settings.elevation,
            HWND(owner as _),
        )
    })
    .await
}

#[tauri::command]
async fn run_as_administrator(
    window: Window,
    files: State<'_, Files>,
    target: contract::ItemRef,
) -> Result<(), String> {
    let files = files.0.clone();
    let owner = owner(&window)?;
    let target = item_id(&target).map_err(message)?;
    blocking(move || {
        actions::run_as_administrator(&target, &files.settings().elevation, HWND(owner as _))
    })
    .await
}

/// Tells every window that an item's Windows icon changed, after dropping the cached images.
fn icon_changed(app: &AppHandle, icons: &Icons, key: String) {
    icons.forget(key.clone());
    let _ = app.emit(contract::ICON_CHANGED, key);
}

/// Windows' Change Icon dialog for one folder or shortcut. Returns false when it was cancelled.
#[tauri::command]
async fn pick_custom_icon(
    app: AppHandle,
    window: Window,
    state: State<'_, App>,
    item: ItemRef,
) -> Result<bool, String> {
    let owner = owner(&window)?;
    let item = item_id(&item).map_err(message)?;
    let icons = state.icons.clone();
    blocking(move || {
        let chosen = custom_icon::pick(owner, &item)?;
        let picked = chosen.is_some();
        if let Some(key) = chosen {
            icon_changed(&app, &icons, key);
        }
        Ok(picked)
    })
    .await
}

#[tauri::command]
async fn reset_custom_icon(
    app: AppHandle,
    state: State<'_, App>,
    item: ItemRef,
) -> Result<(), String> {
    let item = item_id(&item).map_err(message)?;
    let icons = state.icons.clone();
    blocking(move || {
        let key = custom_icon::reset(&item)?;
        icon_changed(&app, &icons, key);
        Ok(())
    })
    .await
}

fn shell_view(item: ShellItem) -> ShellMenuItem {
    ShellMenuItem {
        id: item.id,
        label: item.label,
        icon: item.icon,
        children: item.children.into_iter().map(shell_view).collect(),
        disabled: item.disabled,
        separator: item.separator,
    }
}

/// Opens the Windows menu of `items`, or of the empty area of the folder `items[0]`.
#[tauri::command]
async fn shell_menu_open(
    window: Window,
    state: State<'_, App>,
    items: Vec<ItemRef>,
    background: bool,
) -> Result<Vec<ShellMenuItem>, String> {
    let owner = owner(&window)?;
    let label = window.label().to_owned();
    let menus = state.menus.clone();
    let items = items
        .iter()
        .map(item_id)
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(message)?;
    blocking(move || {
        anyhow::ensure!(
            (1..=MAX_TRANSFER_ITEMS).contains(&items.len()),
            "Select between 1 and {MAX_TRANSFER_ITEMS} items"
        );
        let mut paths = Vec::with_capacity(items.len());
        for item in &items {
            drop(item.verify()?);
            paths.push(PathBuf::from(display_path(item.path())));
        }
        let target = if background {
            Target::Background(paths.swap_remove(0))
        } else {
            Target::Items(paths)
        };
        Ok(menus
            .open(&label, owner, target)?
            .into_iter()
            .map(shell_view)
            .collect())
    })
    .await
}

#[tauri::command]
async fn shell_menu_invoke(
    window: Window,
    state: State<'_, App>,
    id: u32,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let label = window.label().to_owned();
    let menus = state.menus.clone();
    blocking(move || menus.invoke(&label, id, (x, y))).await
}

/// "Show more options": the same menu, drawn by Windows at the cursor.
#[tauri::command]
async fn shell_menu_native(window: Window, state: State<'_, App>) -> Result<(), String> {
    let label = window.label().to_owned();
    let menus = state.menus.clone();
    blocking(move || menus.native(&label)).await
}

#[tauri::command]
async fn shell_menu_close(window: Window, state: State<'_, App>) -> Result<(), String> {
    let label = window.label().to_owned();
    let menus = state.menus.clone();
    blocking(move || {
        menus.close(&label);
        Ok(())
    })
    .await
}

fn session_state(session: &Session) -> SessionState {
    SessionState {
        pins: session
            .settings()
            .pins
            .iter()
            .map(|pin| {
                let segment = segment(pin);
                Pin {
                    name: segment.name,
                    path: segment.path,
                    item: ItemId::at(pin.clone()).ok().as_ref().map(item_ref),
                    icon: icons::folder_icon(pin),
                }
            })
            .collect(),
        clipboard: session.clipboard().map(|clipboard| ClipboardState {
            count: clipboard.items.len() as u32,
            cut: clipboard.cut,
        }),
    }
}

/// Sends the session state to every window and returns it.
fn broadcast(app: &AppHandle, session: &Session) -> SessionState {
    let state = session_state(session);
    let _ = app.emit(contract::SESSION_CHANGED, state.clone());
    state
}

// Building a window from a synchronous command deadlocks on Windows, so this one is async.
#[tauri::command]
async fn open_window(
    app: AppHandle,
    state: State<'_, App>,
    path: String,
    split: bool,
) -> Result<String, String> {
    let label = reserve_window(&state)?;
    lock(&state.windows).insert(
        label.clone(),
        WindowRequest {
            path,
            split,
            requested: Instant::now(),
            adopt: None,
        },
    );
    if let Err(error) = build_window(&app, &label) {
        lock(&state.windows).remove(&label);
        state.window_count.fetch_sub(1, Ordering::Relaxed);
        return Err(message(error));
    }
    Ok(label)
}

fn reserve_window(state: &App) -> Result<String, String> {
    state
        .window_count
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            (count < 4).then_some(count + 1)
        })
        .map_err(|_| "Firewol supports up to four windows".to_owned())?;
    Ok(format!(
        "window-{}",
        state.next_window.fetch_add(1, Ordering::Relaxed) + 1
    ))
}

#[tauri::command]
async fn adopt_pane(
    app: AppHandle,
    window: Window,
    state: State<'_, App>,
    tab: contract::TabSnapshot,
) -> Result<String, String> {
    let label = reserve_window(&state)?;
    let pane = match &tab {
        contract::TabSnapshot::Folder { pane, .. } => Some(pane.clone()),
        _ => None,
    };
    if let Some(pane) = &pane
        && let Err(error) = state.folders.adopt(window.label(), &label, pane)
    {
        state.window_count.fetch_sub(1, Ordering::Relaxed);
        return Err(message(error));
    }
    let path = match &tab {
        contract::TabSnapshot::Folder { path, .. } => path.clone(),
        _ => String::new(),
    };
    lock(&state.windows).insert(
        label.clone(),
        WindowRequest {
            path,
            split: false,
            requested: Instant::now(),
            adopt: Some(tab),
        },
    );
    if let Err(error) = build_window(&app, &label) {
        lock(&state.windows).remove(&label);
        state.window_count.fetch_sub(1, Ordering::Relaxed);
        if let Some(pane) = pane {
            let _ = state.folders.adopt(&label, window.label(), &pane);
        }
        return Err(message(error));
    }
    Ok(label)
}

fn build_window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    let mut arguments = BROWSER_ARGS.to_owned();
    if let Ok(extra) = std::env::var("FIREWOL_WEBVIEW_ARGS") {
        arguments.push(' ');
        arguments.push_str(&extra);
    }
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("Firewol")
        .inner_size(1280.0, 800.0)
        .min_inner_size(640.0, 420.0)
        .decorations(false)
        .additional_browser_args(&arguments)
        .build()
}

/// Asks WebView2 to release caches while a window is in the background.
fn memory_target(window: &Window, focused: bool) {
    if std::env::var_os("FIREWOL_KEEP_MEMORY").is_some() {
        return;
    }
    let Some(webview) = window.app_handle().get_webview_window(window.label()) else {
        return;
    };
    let level = if focused {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    };
    let _ = webview.with_webview(move |platform| unsafe {
        if let Ok(core) = platform.controller().CoreWebView2()
            && let Ok(core) = core.cast::<ICoreWebView2_19>()
        {
            let _ = core.SetMemoryUsageTargetLevel(level);
        }
    });
}

/// Report mode only: `<label>.json`, or `<label>-<name>.json` for a later stage of the run.
#[tauri::command]
fn save_report(
    window: Window,
    state: State<'_, App>,
    report: serde_json::Value,
    name: Option<String>,
) -> Result<(), String> {
    let dir = state.report.as_ref().ok_or("Report mode is disabled")?;
    let target = match name {
        Some(name) if report_name(&name) => dir.join(format!("{}-{name}.json", window.label())),
        Some(_) => return Err("Report names use lowercase letters and hyphens".into()),
        None => dir.join(format!("{}.json", window.label())),
    };
    let staged = target.with_extension("json.tmp");
    serde_json::to_writer_pretty(File::create_new(&staged).map_err(message)?, &report)
        .map_err(message)?;
    fs::rename(staged, target).map_err(message)
}

fn report_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

/// Report mode only: waits until the measuring script creates `<name>.go` in the report folder.
#[tauri::command]
async fn await_signal(state: State<'_, App>, name: String) -> Result<(), String> {
    let dir = state.report.clone().ok_or("Report mode is disabled")?;
    if !report_name(&name) {
        return Err("Signal names use lowercase letters and hyphens".into());
    }
    blocking(move || {
        let signal = dir.join(format!("{name}.go"));
        let started = Instant::now();
        while !signal.exists() {
            anyhow::ensure!(
                started.elapsed().as_secs() < 600,
                "No {name} signal within 10 minutes"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Ok(())
    })
    .await
}

fn notifier(app: &AppHandle) -> Notify {
    let app = app.clone();
    Arc::new(move |notice| {
        let _ = match notice {
            Notice::Progress(progress) => app.emit(contract::FOLDER_PROGRESS, progress),
            Notice::Index(state) => app.emit(contract::INDEX_STATE, state),
        };
    })
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || task().map_err(|error| format!("{error:#}")))
        .await
        .map_err(message)?
}

fn setting(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn message(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

/// Runs the elevated read-only NTFS helper when Firewol started itself for volume search.
/// It must return before Tauri starts, so the elevated process never opens a window.
fn ntfs_helper() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|arg| arg != "--ntfs-helper") {
        return None;
    }
    let result = (|| -> anyhow::Result<()> {
        let [_, drive, serial, pipe, parent] = args.as_slice() else {
            anyhow::bail!("Usage: --ntfs-helper DRIVE SERIAL PIPE PARENT");
        };
        firewol_core::explorer::volume::run_helper(
            drive.to_string_lossy().parse()?,
            serial.to_string_lossy().parse()?,
            &pipe.to_string_lossy(),
            parent.to_string_lossy().parse()?,
        )
    })();
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    })
}

fn main() {
    if let Some(code) = ntfs_helper() {
        std::process::exit(code);
    }
    STARTED.get_or_init(Instant::now);
    let folders = Arc::new(Folders::default());
    let closing = folders.clone();
    let shell_icons = Arc::new(Icons::start().expect("Start Shell icon worker"));
    let served_icons = shell_icons.clone();
    let shell_menus = Arc::new(ShellMenus::default());
    let served_menus = shell_menus.clone();
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("icon", move |_context, request, responder| {
            icon_protocol::respond(&served_icons, &served_menus, request, responder);
        })
        .manage(App {
            folders,
            icons: shell_icons,
            menus: shell_menus,
            windows: Mutex::default(),
            next_window: AtomicU32::new(0),
            window_count: AtomicU32::new(1),
            report: std::env::var_os("FIREWOL_SPIKE_REPORT")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
        })
        .setup(|app| {
            let data = data_dir()?;
            std::thread::spawn(move || firewol_core::explorer::remove_stale_indexes(&data));
            let handle = app.handle().clone();
            let transfers = Arc::new(Transfers::default());
            app.manage(transfers.clone());
            let session = Session::open(data_dir()?, move |done| {
                if let Some(status) = transfers.finish(&done.message, done.succeeded) {
                    let _ = handle.emit(contract::TRANSFER_PROGRESS, status);
                }
                let _ = handle.emit(
                    contract::OPERATION_DONE,
                    OperationDone {
                        message: done.message,
                        succeeded: done.succeeded,
                        refresh: done.refresh,
                    },
                );
            })?;
            // Best effort: a settings file that cannot be written must not stop the app.
            let _ = session.seed_pins();
            let session = Arc::new(session);
            let devices = app.handle().clone();
            let _ = firewol_core::explorer::watch_devices(move || {
                let _ = devices.emit(contract::DRIVES_CHANGED, ());
            });
            let styles = session.clone();
            app.state::<App>()
                .folders
                .use_styles(move || styles.settings().folders);
            app.manage(Files(session));
            build_window(app.handle(), "main")?;
            Ok(())
        })
        .on_window_event(move |window, event| match event {
            WindowEvent::Destroyed => {
                closing.close_owned(window.label());
                let state = window.state::<App>();
                lock(&state.windows).remove(window.label());
                state.window_count.fetch_sub(1, Ordering::Relaxed);
            }
            WindowEvent::Focused(focused) => memory_target(window, *focused),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            startup,
            navigate,
            close_pane,
            suspend_pane,
            adopt_pane,
            list_page,
            folder_progress,
            locate_selection,
            start_index,
            search,
            search_count,
            ntfs_drive,
            volume_states,
            start_volume_index,
            stop_volume_index,
            session,
            set_clipboard,
            paste,
            transfer,
            rename,
            recycle,
            subfolders,
            drive_space,
            drives,
            undo,
            transfer_status,
            pause_transfer,
            cancel_transfer,
            toggle_pin,
            move_pin,
            get_settings,
            update_settings,
            set_folder_style,
            operation_busy,
            run_menu_action,
            run_as_administrator,
            pick_custom_icon,
            reset_custom_icon,
            shell_menu_open,
            shell_menu_invoke,
            shell_menu_native,
            shell_menu_close,
            open_window,
            save_report,
            await_signal
        ])
        .run(tauri::generate_context!())
        .expect("Firewol failed to start");
}
