#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod contract;
mod folders;
mod icons;
mod settings;

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use anyhow::Context as _;
use contract::{
    ClipboardState, FolderStyleView, IndexState, ItemRef, Navigation, OperationDone, Page, Pin,
    SessionState, SettingsView, Startup, Step, TransferPhase, TransferStatus,
};
use firewol_core::explorer::{
    ItemId, MenuAction, Operation, Session, TransferState, actions, data_dir, home,
};
use folders::{Folders, Notice, Notify, item_id, item_ref, lock, segment, text};
use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window,
    WindowEvent,
};
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
async fn search(state: State<'_, App>, id: u64, text: String, offset: u32) -> Result<Page, String> {
    let folders = state.folders.clone();
    blocking(move || folders.search(id, text, offset)).await
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
fn paste(files: State<'_, Files>, into: ItemRef) -> Result<(), String> {
    files
        .0
        .paste(item_id(&into).map_err(message)?)
        .map(drop)
        .map_err(message)
}

#[tauri::command]
fn transfer(
    files: State<'_, Files>,
    items: Vec<ItemRef>,
    into: ItemRef,
    move_items: bool,
) -> Result<(), String> {
    let items = items.iter().map(item_id).collect::<anyhow::Result<_>>();
    let into = item_id(&into).map_err(message)?;
    files
        .0
        .transfer(items.map_err(message)?, into, move_items)
        .map(drop)
        .map_err(message)
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

#[tauri::command]
fn recycle(files: State<'_, Files>, item: ItemRef) -> Result<(), String> {
    let item = item_id(&item).map_err(message)?;
    files.0.run(Operation::Recycle { item }).map_err(message)
}

#[tauri::command]
fn undo(files: State<'_, Files>) -> Result<(), String> {
    files.0.undo().map(drop).map_err(message)
}

#[tauri::command]
fn transfer_status(files: State<'_, Files>) -> Option<TransferStatus> {
    files.0.progress().map(|progress| TransferStatus {
        phase: match progress.state {
            TransferState::Running => TransferPhase::Running,
            TransferState::Paused => TransferPhase::Paused,
            TransferState::Cancelled => TransferPhase::Cancelled,
        },
        bytes: progress.bytes,
        finished: progress.finished as u32,
        verifying: progress.verifying as u32,
    })
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

#[tauri::command]
fn save_report(
    window: Window,
    state: State<'_, App>,
    report: serde_json::Value,
) -> Result<(), String> {
    let dir = state.report.as_ref().ok_or("Report mode is disabled")?;
    let target = dir.join(format!("{}.json", window.label()));
    let staged = target.with_extension("json.tmp");
    serde_json::to_writer_pretty(File::create_new(&staged).map_err(message)?, &report)
        .map_err(message)?;
    fs::rename(staged, target).map_err(message)
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

fn main() {
    STARTED.get_or_init(Instant::now);
    let folders = Arc::new(Folders::default());
    let closing = folders.clone();
    let icons = firewol_core::explorer::icons::Icons::start().expect("Start Shell icon worker");
    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("icon", move |_context, request, responder| {
            icons::respond(&icons, request, responder);
        })
        .manage(App {
            folders,
            windows: Mutex::default(),
            next_window: AtomicU32::new(0),
            window_count: AtomicU32::new(1),
            report: std::env::var_os("FIREWOL_SPIKE_REPORT")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let session = Session::open(data_dir()?, move |done| {
                let _ = handle.emit(
                    contract::OPERATION_DONE,
                    OperationDone {
                        message: done.message,
                        succeeded: done.succeeded,
                        refresh: done.refresh,
                    },
                );
            })?;
            let session = Arc::new(session);
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
            session,
            set_clipboard,
            paste,
            transfer,
            rename,
            recycle,
            undo,
            transfer_status,
            pause_transfer,
            cancel_transfer,
            toggle_pin,
            get_settings,
            update_settings,
            set_folder_style,
            operation_busy,
            run_menu_action,
            run_as_administrator,
            open_window,
            save_report
        ])
        .run(tauri::generate_context!())
        .expect("Firewol failed to start");
}
