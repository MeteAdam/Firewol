//! Typed boundary between the Rust core and the Svelte interface.
//! `cargo test -p firewol-tauri` regenerates `src/lib/bindings/` from these types;
//! `src/lib/contract.ts` re-exports them with the event names.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const FOLDER_PROGRESS: &str = "folder-progress";
pub const INDEX_STATE: &str = "index-state";
pub const VOLUME_STATE: &str = "volume-state";
pub const TRANSFER_PROGRESS: &str = "transfer-progress";
pub const OPERATION_DONE: &str = "operation-done";
pub const SESSION_CHANGED: &str = "session-changed";
pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const ICON_CHANGED: &str = "icon-changed";
pub const DRIVES_CHANGED: &str = "drives-changed";

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

/// Details column widths in pixels, shared by every folder.
#[derive(Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ColumnWidthsView {
    pub name: u32,
    pub location: u32,
    pub modified: u32,
    pub kind: u32,
    pub size: u32,
    pub tags: u32,
}

#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct SettingsView {
    pub version: u32,
    pub theme_migrated: bool,
    pub sidebar: bool,
    pub grid: bool,
    pub tile: u32,
    /// Pixels, 180 to 360 (default 232).
    pub sidebar_width: u32,
    /// Details row height in pixels: 24, 32 (default), 40 or 52.
    pub row: u32,
    /// The context menu lists commands from Windows and installed apps.
    pub shell_items: bool,
    /// Folders list items marked hidden or system.
    pub show_hidden: bool,
    /// File names show their extension.
    pub show_extensions: bool,
    pub columns: ColumnWidthsView,
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
    pub grid: Option<bool>,
    /// 96, 128 or 160.
    #[ts(optional)]
    pub tile: Option<u32>,
    /// 180 to 360 pixels.
    #[ts(optional)]
    pub sidebar_width: Option<u32>,
    /// 24, 32, 40 or 52.
    #[ts(optional)]
    pub row: Option<u32>,
    #[ts(optional)]
    pub shell_items: Option<bool>,
    #[ts(optional)]
    pub show_hidden: Option<bool>,
    #[ts(optional)]
    pub show_extensions: Option<bool>,
    #[ts(optional)]
    pub columns: Option<ColumnWidthsView>,
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
    /// Shell cache key when the folder has its own Windows icon.
    pub icon: Option<String>,
}

/// The folders a breadcrumb chevron lists: the children of the segment before it.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct Subfolders {
    pub folders: Vec<Segment>,
    /// More folders exist than the list holds.
    pub more: bool,
}

/// Free and total bytes of the drive holding the open folder.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct DriveSpace {
    #[ts(type = "number")]
    pub free: u64,
    #[ts(type = "number")]
    pub total: u64,
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
    /// Shell cache key; only folders with their own Windows icon have one.
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
    /// Shell cache key when the open folder has its own Windows icon.
    pub icon: Option<String>,
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

/// Search filters; an absent field does not filter. Dates are Unix seconds, inclusive.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchFilters {
    pub extension: Option<String>,
    #[ts(type = "number | null")]
    pub min_bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub max_bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub modified_after: Option<i64>,
    #[ts(type = "number | null")]
    pub modified_before: Option<i64>,
}

/// Where a search looks: the open folder and its subfolders, or a whole NTFS volume.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SearchScope {
    Folder,
    Volume { drive: String },
}

#[derive(Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchRequest {
    /// The open folder the searching tab shows.
    #[ts(type = "number")]
    pub id: u64,
    pub text: String,
    pub regex: bool,
    pub filters: SearchFilters,
    pub scope: SearchScope,
    pub offset: u32,
}

/// Matches counted up to `SEARCH_COUNT_CAP`; `more` means there are more than `count`.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct SearchCount {
    pub count: u32,
    pub more: bool,
}

/// Emitted as `volume-state` to every window while an NTFS volume index starts, updates or stops.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct VolumeState {
    pub drive: String,
    pub ready: bool,
    pub running: bool,
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
    /// Shell cache key when the folder has its own Windows icon.
    pub icon: Option<String>,
}

/// One command of the Windows context menu. `id` is valid until the menu closes.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct ShellMenuItem {
    pub id: u32,
    pub label: String,
    /// `menu:` icon key; extension icons that draw themselves have none.
    pub icon: Option<String>,
    pub children: Vec<ShellMenuItem>,
    pub disabled: bool,
    pub separator: bool,
    /// The language-independent name; a class id for packaged commands, empty when unknown.
    pub verb: String,
    /// What a double-click runs, when that is not plain "open".
    pub default: bool,
    /// Where a top-level command goes in Firewol's menu.
    #[ts(
        type = "'apps' | 'favorite' | 'pin' | 'more' | 'terminal' | 'open_with' | 'new' | 'properties'"
    )]
    pub slot: String,
}

/// A folder of the Go To menu.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct GoToFolder {
    pub name: String,
    pub path: String,
}

/// What the context menu needs from the machine; read once per window.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct MenuPlaces {
    pub folders: Vec<GoToFolder>,
    pub windows_terminal: bool,
}

/// The Attributes submenu of one item. Times are Unix seconds.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct ItemAttributesView {
    pub hidden: bool,
    pub system: bool,
    pub read_only: bool,
    pub archive: bool,
    #[ts(type = "number")]
    pub created: i64,
    #[ts(type = "number")]
    pub modified: i64,
    #[ts(type = "number")]
    pub accessed: i64,
}

/// What opening items did. `navigate` is a folder the interface shows itself (a shortcut's target).
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct OpenResult {
    pub navigate: Option<String>,
}

/// A drive in the sidebar's This PC section.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct DriveView {
    /// For example `Local Disk (C:)`.
    pub name: String,
    pub path: String,
    pub item: Option<ItemRef>,
    pub icon: Option<String>,
}

/// Emitted as `session-changed` to every window when pins or the clipboard change.
#[derive(Clone, Serialize, TS)]
#[ts(export)]
pub struct SessionState {
    pub pins: Vec<Pin>,
    pub clipboard: Option<ClipboardState>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferPhase {
    Running,
    Paused,
    Cancelled,
    /// The job ended; `message` and `succeeded` hold its result until the next transfer.
    Finished,
}

/// Emitted as `transfer-progress` to every window every 250 ms while a transfer or undo runs,
/// and once more with its result.
#[derive(Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferStatus {
    pub phase: TransferPhase,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Bytes the file copies will write; 0 when nothing reports bytes (folders, renames).
    #[ts(type = "number")]
    pub total_bytes: u64,
    pub finished: u32,
    /// Items in the job, or 0 when unknown.
    pub total_items: u32,
    pub verifying: u32,
    /// Bytes per second over the last second.
    pub speed: f64,
    /// Bytes per second for each 250 ms tick, oldest first, at most 60.
    pub samples: Vec<f64>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub message: Option<String>,
    pub succeeded: Option<bool>,
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
    /// Gate only (`FIREWOL_SPIKE_COMBINED`): a folder with `source.bin` and `dest`. On the gate's
    /// signal every window loads at once: the main window copies the file, the others index.
    pub combined: Option<String>,
    pub session: SessionState,
    /// The Windows regional format (for example `tr-TR`), which can differ from the display language.
    pub locale: String,
    /// Milliseconds since the process (main window) or this window was requested.
    pub since_start_ms: f64,
}
