//! Typed boundary between the Rust core and the Svelte interface.
//! `cargo test -p firewol-tauri` regenerates `src/lib/bindings/` from these types;
//! `src/lib/contract.ts` re-exports them with the event names.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const FOLDER_PROGRESS: &str = "folder-progress";
pub const INDEX_STATE: &str = "index-state";
pub const OPERATION_DONE: &str = "operation-done";
pub const SESSION_CHANGED: &str = "session-changed";

/// A folder shown in the breadcrumb or the sidebar.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct Segment {
    pub name: String,
    pub path: String,
}

/// A core-issued item identity. File operations act on it, never on displayed text.
/// The file index is a decimal string because it can exceed JavaScript's safe integers.
#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ItemRef {
    pub path: String,
    pub volume: u32,
    pub index: String,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
    /// Shell cache key; folders keep the theme's vector icon.
    pub icon: Option<String>,
    /// File length in bytes; `null` for folders.
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    /// Last write time in Unix seconds.
    #[ts(type = "number | null")]
    pub modified: Option<i64>,
    pub hidden: bool,
    pub id: Option<ItemRef>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Page {
    pub offset: u32,
    pub items: Vec<Entry>,
    pub has_more: bool,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct FolderView {
    #[ts(type = "number")]
    pub id: u64,
    pub path: String,
    pub item: Option<ItemRef>,
    pub segments: Vec<Segment>,
    pub count: u32,
    pub complete: bool,
    pub page: Page,
}

/// Which way a pane moves through its history.
#[derive(Clone, Copy, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Step {
    New,
    Back,
    Forward,
    Reload,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Navigation {
    pub folder: FolderView,
    pub can_back: bool,
    pub can_forward: bool,
}

/// Emitted as `folder-progress` while a folder is still being enumerated.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct FolderProgress {
    #[ts(type = "number")]
    pub id: u64,
    pub count: u32,
    pub complete: bool,
    pub error: Option<String>,
}

/// Emitted as `index-state` when a folder index becomes ready or fails.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct IndexState {
    #[ts(type = "number")]
    pub id: u64,
    pub ready: bool,
    #[ts(type = "number")]
    pub count: u64,
    pub error: Option<String>,
}

/// Emitted as `operation-done` to every window when a file operation finishes.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct OperationDone {
    pub message: String,
    pub succeeded: bool,
    pub refresh: bool,
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct ClipboardState {
    pub count: u32,
    pub cut: bool,
}

/// Emitted as `session-changed` to every window when pins or the clipboard change.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct SessionState {
    pub pins: Vec<Segment>,
    pub clipboard: Option<ClipboardState>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferPhase {
    Running,
    Paused,
    Cancelled,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferStatus {
    pub phase: TransferPhase,
    #[ts(type = "number")]
    pub bytes: u64,
    pub finished: u32,
    pub verifying: u32,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Startup {
    pub label: String,
    pub path: String,
    pub split: bool,
    pub report: bool,
    /// Runs the interface's file-operation self-test (`FIREWOL_SELF_TEST`) and reports it.
    pub self_test: bool,
    pub extra_windows: u32,
    pub idle_ms: u32,
    pub session: SessionState,
    /// The Windows regional format (for example `tr-TR`), which can differ from the display language.
    pub locale: String,
    /// Milliseconds since the process (main window) or this window was requested.
    pub since_start_ms: f64,
}
