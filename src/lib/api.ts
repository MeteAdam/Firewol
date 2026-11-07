import { invoke } from "@tauri-apps/api/core";
import type {
  IndexState,
  ItemRef,
  Navigation,
  Page,
  SessionState,
  Startup,
  Step,
  TransferStatus,
  SettingsView,
  SettingsPatch,
  FolderStyleView,
  SortView,
  FolderProgress,
  TabSnapshot,
  SearchRequest,
  SearchCount,
  VolumeState,
  Subfolders,
  DriveSpace,
} from "./contract";

export const api = {
  startup: () => invoke<Startup>("startup"),
  getSettings: () => invoke<SettingsView>("get_settings"),
  updateSettings: (patch: SettingsPatch) => invoke<SettingsView>("update_settings", { patch }),
  /** `null` (or an empty style) clears the folder's customization. */
  setFolderStyle: (folder: ItemRef, style: FolderStyleView | null) =>
    invoke<SettingsView>("set_folder_style", { folder, style }),
  operationBusy: () => invoke<boolean>("operation_busy"),
  navigate: (pane: string, path: string | null, step: Step, sort: SortView) =>
    invoke<Navigation>("navigate", { pane, path, step, sort }),
  closePane: (pane: string) => invoke<void>("close_pane", { pane }),
  suspendPane: (pane: string) => invoke<void>("suspend_pane", { pane }),
  adoptPane: (tab: TabSnapshot) => invoke<string>("adopt_pane", { tab }),
  listPage: (id: number, offset: number, sort: SortView) => invoke<Page>("list_page", { id, offset, sort }),
  folderProgress: (id: number) => invoke<FolderProgress>("folder_progress", { id }),
  locateSelection: (id: number, items: ItemRef[]) => invoke<(number | null)[]>("locate_selection", { id, items }),
  startIndex: (id: number) => invoke<IndexState>("start_index", { id }),
  search: (request: SearchRequest) => invoke<Page>("search", { request }),
  searchCount: (request: SearchRequest) => invoke<SearchCount>("search_count", { request }),
  /** The drive letter when `path` is on a local NTFS volume that volume search can index. */
  ntfsDrive: (path: string) => invoke<string | null>("ntfs_drive", { path }),
  volumeStates: () => invoke<VolumeState[]>("volume_states"),
  startVolumeIndex: (drive: string) => invoke<VolumeState>("start_volume_index", { drive }),
  stopVolumeIndex: (drive: string) => invoke<void>("stop_volume_index", { drive }),
  session: () => invoke<SessionState>("session"),
  setClipboard: (items: ItemRef[], cut: boolean) => invoke<SessionState>("set_clipboard", { items, cut }),
  paste: (into: ItemRef) => invoke<void>("paste", { into }),
  transfer: (items: ItemRef[], into: ItemRef, moveItems: boolean) =>
    invoke<void>("transfer", { items, into, moveItems }),
  rename: (item: ItemRef, name: string) => invoke<void>("rename", { item, name }),
  /** One batch: a single undo restores every item. */
  recycle: (items: ItemRef[]) => invoke<void>("recycle", { items }),
  subfolders: (path: string) => invoke<Subfolders>("subfolders", { path }),
  driveSpace: (path: string) => invoke<DriveSpace>("drive_space", { path }),
  undo: () => invoke<void>("undo"),
  transferStatus: () => invoke<TransferStatus | null>("transfer_status"),
  pauseTransfer: () => invoke<void>("pause_transfer"),
  cancelTransfer: () => invoke<void>("cancel_transfer"),
  runMenuAction: (index: number, executable: string, target: ItemRef) => invoke<void>("run_menu_action", { index, executable, target }),
  runAsAdministrator: (target: ItemRef) => invoke<void>("run_as_administrator", { target }),
  togglePin: (path: string) => invoke<SessionState>("toggle_pin", { path }),
  openWindow: (path: string, split: boolean) => invoke<string>("open_window", { path, split }),
  /** `name` saves `<label>-<name>.json` for a later stage of a report run. */
  saveReport: (report: object, name?: string) => invoke<void>("save_report", { report, name: name ?? null }),
  /** Report runs only: resolves once the measuring script creates `<name>.go`. */
  awaitSignal: (name: string) => invoke<void>("await_signal", { name }),
};
