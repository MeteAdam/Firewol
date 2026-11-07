#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod contract;
mod folders;
mod icons;

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use contract::{
    ClipboardState, IndexState, ItemRef, Navigation, OperationDone, Page, SessionState, Startup,
    Step, TransferPhase, TransferStatus,
};
use firewol_core::explorer::{Operation, Session, TransferState, data_dir, home};
use folders::{Folders, Notice, Notify, item_id, lock, segment, text};
use tauri::{
    AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window,
    WindowEvent,
};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    ICoreWebView2_19,
};
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
    report: Option<PathBuf>,
}

/// The clipboard, file operations and settings shared by every window.
struct Files(Session);

#[derive(Clone)]
struct WindowRequest {
    path: String,
    split: bool,
    requested: Instant,
}

#[tauri::command]
fn startup(
    window: Window,
    state: State<'_, App>,
    files: State<'_, Files>,
) -> Result<Startup, String> {
    let label = window.label().to_owned();
    let request = lock(&state.windows).get(&label).cloned();
    let main = request.is_none();
    let path = match &request {
        Some(request) => request.path.clone(),
        None => match std::env::var_os("FIREWOL_OPEN") {
            Some(path) => path.to_string_lossy().into_owned(),
            None => text(&home().map_err(message)?),
        },
    };
    let started = request.as_ref().map_or_else(
        || *STARTED.get_or_init(Instant::now),
        |request| request.requested,
    );
    Ok(Startup {
        label,
        path,
        split: request.map_or_else(|| std::env::var_os("FIREWOL_SPLIT").is_some(), |r| r.split),
        report: state.report.is_some(),
        self_test: main
            && state.report.is_some()
            && std::env::var_os("FIREWOL_SELF_TEST").is_some(),
        extra_windows: if main {
            setting("FIREWOL_SPIKE_WINDOWS", 1).saturating_sub(1)
        } else {
            0
        },
        idle_ms: setting("FIREWOL_SPIKE_IDLE_MS", 10_000),
        session: session_state(&files.0),
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
    app: AppHandle,
    window: Window,
    state: State<'_, App>,
    pane: String,
    path: Option<String>,
    step: Step,
) -> Result<Navigation, String> {
    let folders = state.folders.clone();
    let notify = notifier(&app);
    let owner = window.label().to_owned();
    blocking(move || folders.navigate(&owner, &pane, path.as_deref(), step, notify)).await
}

#[tauri::command]
fn close_pane(state: State<'_, App>, pane: String) {
    state.folders.close_pane(&pane);
}

#[tauri::command]
async fn list_page(state: State<'_, App>, id: u64, offset: u32) -> Result<Page, String> {
    let folders = state.folders.clone();
    blocking(move || folders.page(id, offset)).await
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

#[tauri::command]
fn session(files: State<'_, Files>) -> SessionState {
    session_state(&files.0)
}

#[tauri::command]
fn set_clipboard(
    app: AppHandle,
    files: State<'_, Files>,
    items: Vec<ItemRef>,
    cut: bool,
) -> Result<SessionState, String> {
    let items = items.iter().map(item_id).collect::<anyhow::Result<_>>();
    files
        .0
        .set_clipboard(items.map_err(message)?, cut)
        .map_err(message)?;
    Ok(broadcast(&app, &files.0))
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
fn toggle_pin(
    app: AppHandle,
    files: State<'_, Files>,
    path: String,
) -> Result<SessionState, String> {
    files.0.toggle_pin(Path::new(&path)).map_err(message)?;
    Ok(broadcast(&app, &files.0))
}

fn session_state(session: &Session) -> SessionState {
    SessionState {
        pins: session
            .settings()
            .pins
            .iter()
            .map(|pin| segment(pin))
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
) -> Result<(), String> {
    let label = format!(
        "window-{}",
        state.next_window.fetch_add(1, Ordering::Relaxed) + 1
    );
    lock(&state.windows).insert(
        label.clone(),
        WindowRequest {
            path,
            split,
            requested: Instant::now(),
        },
    );
    build_window(&app, &label).map_err(message)?;
    Ok(())
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
            report: std::env::var_os("FIREWOL_SPIKE_REPORT").map(PathBuf::from),
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
            app.manage(Files(session));
            build_window(app.handle(), "main")?;
            Ok(())
        })
        .on_window_event(move |window, event| match event {
            WindowEvent::Destroyed => closing.close_owned(window.label()),
            WindowEvent::Focused(focused) => memory_target(window, *focused),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            startup,
            navigate,
            close_pane,
            list_page,
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
            open_window,
            save_report
        ])
        .run(tauri::generate_context!())
        .expect("Firewol failed to start");
}
