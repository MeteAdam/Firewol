//! Typed boundary between the Rust core and the Svelte interface.
//! `cargo test -p firewol-tauri` regenerates `src/lib/bindings/` from these types;
//! `src/lib/contract.ts` re-exports them with the event names.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const FOLDER_PROGRESS: &str = "folder-progress";
pub const INDEX_STATE: &str = "index-state";
pub const OPERATION_DONE: &str = "operation-done";
pub const SESSION_CHANGED: &str = "session-changed";
pub const SETTINGS_CHANGED: &str = "settings-changed";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SortColumn {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct SortView {
    pub column: SortColumn,
    pub descending: bool,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum TabSnapshot {
    Folder {
        pane: String,
        path: String,
        query: String,
        scroll: f64,
        sort: SortView,
    },
    Settings {
        pane: String,
    },
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppearanceView {
    #[ts(
        type = "'light_morphism' | 'glassmorphism' | 'neumorphism' | 'neubrutalism' | 'claymorphism' | 'skeuomorphism' | 'metal_morphism'"
    )]
    pub preset: String,
    pub dark: bool,
    pub follow_windows: bool,
    pub background: Option<[u8; 3]>,
    pub surface: Option<[u8; 3]>,
    pub text: Option<[u8; 3]>,
    pub accent: Option<[u8; 3]>,
    pub folder: Option<[u8; 3]>,
    pub folder_label: Option<[u8; 3]>,
    pub caption_left: bool,
    #[ts(type = "'rounded' | 'square' | 'circle' | 'minimal'")]
    pub caption_shape: String,
    pub caption_fill: Option<[u8; 3]>,
    pub caption_hover: Option<[u8; 3]>,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MenuActionView {
    Open,
    RunAsAdministrator,
    Containing,
    Copy,
    Cut,
    Paste,
    CopyOther,
    MoveOther,
    Rename,
    Recycle,
    FolderStyle,
    Undo,
    Refresh,
    Custom {
        executable: String,
        administrator: bool,
    },
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MenuEntryView {
    pub action: MenuActionView,
    pub visible: bool,
    pub label: String,
    pub icon: String,
    pub color: Option<[u8; 3]>,
}

/// A folder's alias, icon colour, icon glyph and tags. Sent to `set_folder_style`, where an
/// empty style (or `null`) removes the customization.
#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderStyleView {
    pub alias: String,
    pub color: Option<[u8; 3]>,
    pub icon: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct StyledFolder {
    pub volume: u32,
    pub index: String,
    pub style: FolderStyleView,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ElevationView {
    #[ts(type = "'standard' | 'allowlist' | 'all_executables'")]
    pub mode: String,
    pub executables: Vec<String>,
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct SettingsView {
    pub version: u32,
    pub theme_migrated: bool,
    pub sidebar: bool,
    pub appearance: AppearanceView,
    pub menu: Vec<MenuEntryView>,
    pub folders: Vec<StyledFolder>,
    pub elevation: ElevationView,
}

#[derive(Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
#[ts(export)]
pub struct SettingsPatch {
    #[ts(optional)]
    pub appearance: Option<AppearanceView>,
    #[ts(optional)]
    pub sidebar: Option<bool>,
    #[ts(optional)]
    pub menu: Option<Vec<MenuEntryView>>,
    #[ts(optional)]
    pub elevation: Option<ElevationView>,
    #[ts(optional)]
    pub allow_all_confirmed: Option<bool>,
    #[ts(optional)]
    pub migrate_appearance: Option<bool>,
}

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
    /// The folder style matched by identity when the page was listed; `null` for files.
    pub style: Option<FolderStyleView>,
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Page {
    pub offset: u32,
    pub items: Vec<Entry>,
    pub has_more: bool,
    pub sorted: bool,
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
    pub sort_ms: f64,
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
    pub sort_ms: f64,
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

/// A pinned folder. `item` is its identity (when the folder exists), which the interface
/// matches against `SettingsView.folders` so aliases and colours follow style edits live.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct Pin {
    pub name: String,
    pub path: String,
    pub item: Option<ItemRef>,
}

/// Emitted as `session-changed` to every window when pins or the clipboard change.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct SessionState {
    pub pins: Vec<Pin>,
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
    pub adopt: Option<TabSnapshot>,
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
