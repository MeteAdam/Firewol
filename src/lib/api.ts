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
  DriveView,
  ShellMenuItem,
  OpenResult,
  FolderDisplay,
  FolderGroup,
  DefaultAppsView,
  MenuPlaces,
  ItemAttributesView,
} from "./contract";

export type TerminalKind = "command_prompt" | "power_shell" | "windows_terminal" | "system_information";
export type AttributeName = "hidden" | "system" | "read_only" | "archive";

export const api = {
  startup: () => invoke<Startup>("startup"),
  getSettings: () => invoke<SettingsView>("get_settings"),
  updateSettings: (patch: SettingsPatch) => invoke<SettingsView>("update_settings", { patch }),
  /** `null` (or an empty style) clears the folder's customization. */
  setFolderStyle: (folder: ItemRef, style: FolderStyleView | null) =>
    invoke<SettingsView>("set_folder_style", { folder, style }),
  setFolderView: (folder: ItemRef, view: FolderDisplay) => invoke<void>("set_folder_view", { folder, view }),
  operationBusy: () => invoke<boolean>("operation_busy"),
  navigate: (pane: string, path: string | null, step: Step, sort: SortView) =>
    invoke<Navigation>("navigate", { pane, path, step, sort }),
  closePane: (pane: string) => invoke<void>("close_pane", { pane }),
  suspendPane: (pane: string) => invoke<void>("suspend_pane", { pane }),
  adoptPane: (tab: TabSnapshot) => invoke<string>("adopt_pane", { tab }),
  listPage: (id: number, offset: number, sort: SortView) => invoke<Page>("list_page", { id, offset, sort }),
  folderProgress: (id: number) => invoke<FolderProgress>("folder_progress", { id }),
  defaultApps: () => invoke<DefaultAppsView>("default_apps"),
  rememberTabs: (tabs: string[], active: number) => invoke<void>("remember_tabs", { tabs, active }),
  setDefaultFolders: (enabled: boolean) => invoke<DefaultAppsView>("set_default_folders", { enabled }),
  setDialogProbe: (enabled: boolean) => invoke<DefaultAppsView>("set_dialog_probe", { enabled }),
  folderGroups: (id: number) => invoke<FolderGroup[]>("folder_groups", { id }),
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
  drives: () => invoke<DriveView[]>("drives"),
  undo: () => invoke<void>("undo"),
  transferStatus: () => invoke<TransferStatus | null>("transfer_status"),
  pauseTransfer: () => invoke<void>("pause_transfer"),
  cancelTransfer: () => invoke<void>("cancel_transfer"),
  runMenuAction: (index: number, executable: string, target: ItemRef) => invoke<void>("run_menu_action", { index, executable, target }),
  runAsAdministrator: (target: ItemRef) => invoke<void>("run_as_administrator", { target }),
  /** Windows' Change Icon dialog; false means it was cancelled. */
  pickCustomIcon: (item: ItemRef) => invoke<boolean>("pick_custom_icon", { item }),
  resetCustomIcon: (item: ItemRef) => invoke<void>("reset_custom_icon", { item }),
  /** Runs Windows' default action for files; `navigate` is a folder to show instead. */
  openItems: (items: ItemRef[]) => invoke<OpenResult>("open_items", { items }),
  /** `background` opens the menu of the empty area of the folder `items[0]`. */
  shellMenuOpen: (items: ItemRef[], background: boolean) => invoke<ShellMenuItem[]>("shell_menu_open", { items, background }),
  shellMenuInvoke: (id: number, x: number, y: number) => invoke<void>("shell_menu_invoke", { id, x, y }),
  /** A command from the New menu; resolves with the item it created. */
  shellMenuInvokeNew: (id: number, x: number, y: number, folder: ItemRef) => invoke<ItemRef | null>("shell_menu_invoke_new", { id, x, y, folder }),
  menuPlaces: () => invoke<MenuPlaces>("menu_places"),
  openTerminal: (kind: TerminalKind, target: ItemRef) => invoke<void>("open_terminal", { kind, target }),
  openSystemUri: (uri: string) => invoke<void>("open_system_uri", { uri }),
  itemAttributes: (item: ItemRef) => invoke<ItemAttributesView>("item_attributes", { item }),
  setItemAttribute: (item: ItemRef, attribute: AttributeName, enabled: boolean) => invoke<void>("set_item_attribute", { item, attribute, enabled }),
  createFolder: (into: ItemRef, name: string) => invoke<ItemRef>("create_folder", { into, name }),
  createFile: (into: ItemRef, name: string, contents: string) => invoke<ItemRef>("create_file", { into, name, contents }),
  /** Windows asks for approval; goes through the elevation policy. */
  takeOwnership: (item: ItemRef) => invoke<void>("take_ownership", { item }),
  shellMenuNative: () => invoke<void>("shell_menu_native"),
  shellMenuClose: () => invoke<void>("shell_menu_close"),
  togglePin: (path: string) => invoke<SessionState>("toggle_pin", { path }),
  /** `before` is the pin to land in front of; `null` moves to the end. */
  movePin: (path: string, before: string | null) => invoke<SessionState>("move_pin", { path, before }),
  openWindow: (path: string, split: boolean) => invoke<string>("open_window", { path, split }),
  /** `name` saves `<label>-<name>.json` for a later stage of a report run. */
  saveReport: (report: object, name?: string) => invoke<void>("save_report", { report, name: name ?? null }),
  /** Report runs only: resolves once the measuring script creates `<name>.go`. */
  awaitSignal: (name: string) => invoke<void>("await_signal", { name }),
};
