//! Typed boundary between the Rust core and the Svelte interface.
//! `src/lib/contract.ts` mirrors every type and event name in this file.

use serde::Serialize;

pub const FOLDER_PROGRESS: &str = "folder-progress";
pub const INDEX_STATE: &str = "index-state";

/// A folder shown in the breadcrumb or the sidebar.
#[derive(Clone, Serialize)]
pub struct Segment {
    pub name: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
    /// File length in bytes; `None` for folders and unreadable items.
    pub size: Option<u64>,
    /// Last write time in Unix seconds.
    pub modified: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub offset: u32,
    pub items: Vec<Entry>,
    pub has_more: bool,
}

#[derive(Serialize)]
pub struct FolderView {
    pub id: u64,
    pub path: String,
    pub segments: Vec<Segment>,
    pub count: u32,
    pub complete: bool,
    pub page: Page,
}

/// Emitted as `folder-progress` while a folder is still being enumerated.
#[derive(Clone, Serialize)]
pub struct FolderProgress {
    pub id: u64,
    pub count: u32,
    pub complete: bool,
    pub error: Option<String>,
}

/// Emitted as `index-state` when a folder index becomes ready or fails.
#[derive(Clone, Serialize)]
pub struct IndexState {
    pub id: u64,
    pub ready: bool,
    pub count: u64,
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Startup {
    pub label: String,
    pub path: String,
    pub split: bool,
    pub report: bool,
    pub extra_windows: u32,
    pub idle_ms: u32,
    pub pins: Vec<Segment>,
    /// The Windows regional format (for example `tr-TR`), which can differ from the display language.
    pub locale: String,
    /// Milliseconds since the process (main window) or this window was requested.
    pub since_start_ms: f64,
}
