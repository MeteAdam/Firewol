#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use firewol_core::{Fixture, FixtureInfo, Page, PageRequest, SearchRequest, fixture_root};
use serde::{Deserialize, Serialize};
use tauri::State;

struct AppState {
    fixture: Result<Arc<Fixture>, String>,
    report_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct Session {
    fixture: FixtureInfo,
    benchmark: bool,
}

#[derive(Deserialize, Serialize)]
struct Benchmark {
    first_list_ms: f64,
    search_p95_ms: f64,
    scroll_fps: f64,
    search_samples: usize,
}

#[tauri::command]
fn session(state: State<'_, AppState>) -> Result<Session, String> {
    let fixture = state.fixture.as_ref().map_err(Clone::clone)?;
    Ok(Session {
        fixture: fixture.info,
        benchmark: state.report_path.is_some(),
    })
}

#[tauri::command]
async fn list_dir(state: State<'_, AppState>, request: PageRequest) -> Result<Page, String> {
    let fixture = state.fixture.clone()?;
    tauri::async_runtime::spawn_blocking(move || fixture.list(request).map_err(to_message))
        .await
        .map_err(to_message)?
}

#[tauri::command]
async fn search(state: State<'_, AppState>, request: SearchRequest) -> Result<Page, String> {
    let fixture = state.fixture.clone()?;
    tauri::async_runtime::spawn_blocking(move || fixture.search(&request).map_err(to_message))
        .await
        .map_err(to_message)?
}

#[tauri::command]
fn save_benchmark(state: State<'_, AppState>, report: Benchmark) -> Result<(), String> {
    let path = state
        .report_path
        .as_ref()
        .ok_or("Benchmark mode is disabled")?;
    let file = File::create_new(path).map_err(to_message)?;
    serde_json::to_writer(file, &report).map_err(to_message)
}

fn to_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            fixture: fixture_root()
                .and_then(|root| Fixture::open(&root))
                .map(Arc::new)
                .map_err(to_message),
            report_path: std::env::var_os("FIREWOL_M0_REPORT").map(PathBuf::from),
        })
        .invoke_handler(tauri::generate_handler![
            session,
            list_dir,
            search,
            save_benchmark
        ])
        .run(tauri::generate_context!())
        .expect("Firewol desktop prototype failed");
}
