// The context menu as data. Windows' commands arrive live for each item and the core says where
// each one goes (`slot_of` in shell_menu.rs); Firewol's own rows are placed around them. The
// layout follows the user's Nilesoft configuration (modify, terminal, file-manage and goto.nss).
import type { TerminalKind, AttributeName } from "./api";
import type { Entry, ItemAttributesView, MenuPlaces, ShellMenuItem, SortColumn, SortView } from "./contract";
import { hex } from "./appearance";
import type { IconName } from "./Icon.svelte";
import { details, isCustom, labelOf, parentPath, type BuiltIn, type MenuRow } from "./menu";

export type MenuCommand = {
  kind: "command";
  label: string;
  icon?: IconName;
  /** A Windows icon key. */
  image?: string | null;
  /** The user's own symbol for a saved row. */
  glyph?: string;
  color?: string;
  keys?: string;
  title?: string;
  disabled?: boolean;
  /** One choice of several, shown highlighted. */
  radio?: boolean;
  /** An on/off row, shown with a tick when true. */
  check?: boolean;
  children?: MenuNode[];
  /** Runs this Windows command. */
  shell?: ShellMenuItem;
  /** The Windows command creates an item that should be selected and renamed. */
  creates?: boolean;
  wait?: boolean;
  run?: () => void;
};
export type MenuNode = { kind: "separator" } | { kind: "info"; label: string; keys: string } | MenuCommand;

export type MenuActions = {
  row(row: MenuRow): void;
  openInNewTab(): void;
  openInNewWindow(): void;
  view(grid: boolean, size?: number): void;
  sort(column: SortColumn): void;
  direction(descending: boolean): void;
  terminal(kind: TerminalKind): void;
  system(uri: string): void;
  goTo(path: string): void;
  copy(text: string): void;
  select(mode: "all" | "invert" | "none"): void;
  changeExtension(): void;
  takeOwnership(): void;
  showHidden(show: boolean): void;
  showExtensions(show: boolean): void;
  attribute(name: AttributeName, enabled: boolean): void;
  newFolder(name: "datetime" | "guid"): void;
  newFile(kind: NewFileKind): void;
  togglePin(path: string): void;
  changeIcon(): void;
  editMenu(): void;
  windowsMenu(): void;
};

export const NEW_FILES = {
  txt: "Hello World!",
  xml: "<root>Hello World!</root>",
  json: "[]",
  html: "<html>\n\t<head>\n\t</head>\n\t<body>Hello World!\n\t</body>\n</html>",
};
export type NewFileKind = keyof typeof NEW_FILES;
export const TILE_SIZES = [[160, "Large tiles", "Ctrl+Shift+2"], [128, "Medium tiles", "Ctrl+Shift+3"], [96, "Small tiles", "Ctrl+Shift+4"]] as const;

export type MenuEnv = {
  /** The right-clicked item; undefined on empty space, where actions use the open folder. */
  item?: Entry;
  selected: Entry[];
  /** The open folder's path. */
  folder?: string;
  /** Saved Firewol rows that apply here. */
  rows: MenuRow[];
  shell: ShellMenuItem[] | "loading" | null;
  places: MenuPlaces | null;
  attributes: ItemAttributesView | null;
  grid: boolean;
  tile: number;
  sort: SortView;
  showHidden: boolean;
  showExtensions: boolean;
  /** The folder is pinned to the sidebar. */
  pinned: boolean;
  searching: boolean;
  /** The elevation policy allows `cmd.exe`, which Take ownership runs. */
  canElevate: boolean;
  /** How many of the last app commands fold into a submenu so the menu fits. */
  fold: number;
  act: MenuActions;
};

const HEAD: BuiltIn[] = ["open", "run_as_administrator", "containing"];
const EDIT: BuiltIn[] = ["cut", "copy", "paste", "copy_other", "move_other", "recycle", "rename"];
const separator: MenuNode = { kind: "separator" };
const command = (label: string, icon: IconName | undefined, run: () => void, extra: Partial<MenuCommand> = {}): MenuCommand => ({ kind: "command", label, icon, run, ...extra });
const submenu = (label: string, icon: IconName, children: MenuNode[], extra: Partial<MenuCommand> = {}): MenuCommand => ({ kind: "command", label, icon, children, ...extra });

/** Separators only between groups that have rows. */
const groups = (...parts: MenuNode[][]): MenuNode[] => parts.filter((part) => part.length > 0).flatMap((part, index) => (index === 0 ? part : [separator, ...part]));

/** Windows' standard rows have no bitmap of their own. */
const SLOT_ICONS: Partial<Record<ShellMenuItem["slot"], IconName>> = { properties: "info", new: "plus", open_with: "window", favorite: "star", pin: "pin" };

function shellNode(item: ShellMenuItem, creates = false): MenuNode {
  if (item.separator) return separator;
  return {
    kind: "command",
    label: item.label,
    image: item.icon,
    icon: SLOT_ICONS[item.slot],
    disabled: item.disabled,
    shell: item,
    creates: creates && item.children.length === 0,
    children: item.children.length > 0 ? item.children.map((child) => shellNode(child, creates)) : undefined,
  };
}

function rowNode(env: MenuEnv, row: MenuRow): MenuCommand {
  const { entry } = row;
  const info = details(entry.action);
  return command(labelOf(entry), info.icon, () => env.act.row(row), { glyph: entry.icon || undefined, color: entry.color ? hex(entry.color) : undefined, keys: info.shortcut || undefined, disabled: !row.enabled });
}

const pick = (env: MenuEnv, actions: BuiltIn[]) => env.rows.filter((row) => !isCustom(row.entry.action) && actions.includes(row.entry.action as BuiltIn));
const customRows = (env: MenuEnv) => env.rows.filter((row) => isCustom(row.entry.action));
const liveShell = (env: MenuEnv) => (Array.isArray(env.shell) ? env.shell : []);
const inSlot = (env: MenuEnv, slot: ShellMenuItem["slot"]) => liveShell(env).filter((item) => item.slot === slot);
const shellSlot = (env: MenuEnv, slot: ShellMenuItem["slot"]) => inSlot(env, slot).map((item) => shellNode(item));
const windowsMenuRow = (env: MenuEnv) => command("Show Windows menu", "menu", env.act.windowsMenu);

/** The app commands, with the last `fold` of them in a submenu when the window is short. */
function appRows(env: MenuEnv, skip?: ShellMenuItem): MenuNode[] {
  if (env.shell === "loading") return [{ kind: "command", label: "Loading Windows commands…", disabled: true, wait: true }];
  const apps = inSlot(env, "apps").filter((item) => item !== skip).map((item) => shellNode(item));
  const fold = Math.min(env.fold, apps.length);
  if (fold < 2) return apps;
  return [...apps.slice(0, apps.length - fold), submenu("More commands", "more", apps.slice(apps.length - fold))];
}

function terminalMenu(env: MenuEnv): MenuCommand {
  const { act } = env;
  return submenu("Terminal", "terminal", [
    command("Command Prompt", "terminal", () => act.terminal("command_prompt")),
    command("Windows PowerShell", "terminal", () => act.terminal("power_shell")),
    ...(env.places?.windows_terminal ? [command("Windows Terminal", "terminal", () => act.terminal("windows_terminal"))] : []),
    ...shellSlot(env, "terminal"),
  ]);
}

function copyPathMenu(env: MenuEnv): MenuCommand {
  const { act, item, selected } = env;
  const copy = (text: string) => command(text, undefined, () => act.copy(text), { title: text });
  if (item && selected.length > 1) {
    const all = selected.map((entry) => entry.path).join("\n");
    return submenu("Copy path", "copy", [command(`Copy (${selected.length}) items selected`, undefined, () => act.copy(all))]);
  }
  const path = item?.path ?? env.folder ?? "";
  const parent = parentPath(path);
  const name = path.split("\\").filter(Boolean).at(-1) ?? "";
  const dot = item && !item.directory ? name.lastIndexOf(".") : -1;
  return submenu("Copy path", "copy", groups(
    [copy(path)],
    parent.length > 3 ? [copy(parent)] : [],
    [copy(name), ...(dot > 0 ? [copy(name.slice(0, dot)), copy(name.slice(dot + 1))] : [])],
  ));
}

function attributesMenu(env: MenuEnv): MenuCommand | undefined {
  const state = env.attributes;
  if (!state || env.selected.length > 1) return undefined;
  const { act } = env;
  const toggle = (label: string, name: AttributeName, on: boolean) => command(label, on ? "check" : undefined, () => act.attribute(name, !on), { check: on });
  const date = (label: string, seconds: number): MenuNode => ({ kind: "info", label, keys: new Date(seconds * 1000).toISOString().slice(0, 10).replaceAll("-", "/") });
  return submenu("Attributes", "info", [
    toggle("Hidden", "hidden", state.hidden),
    toggle("System", "system", state.system),
    toggle("Read-Only", "read_only", state.read_only),
    toggle("Archive", "archive", state.archive),
    separator,
    date("Created", state.created),
    date("Modified", state.modified),
    date("Accessed", state.accessed),
  ]);
}

function fileManageMenu(env: MenuEnv): MenuCommand {
  const { act, item, selected } = env;
  const single = selected.length <= 1;
  const onFile = !!item && !item.directory;
  // A single folder has these two at the top level of its menu (`itemMenu`).
  const style = item?.directory && single ? [] : pick(env, ["folder_style"]).map((row) => rowNode(env, row));
  const shortcut = onFile && /\.lnk$/i.test(item.name);
  const iconRow = single && (item ? shortcut : true) ? [command("Change icon…", "image", act.changeIcon)] : [];
  const attributes = attributesMenu(env);
  return submenu("File manage", "settings", groups(
    [
      copyPathMenu(env),
      ...(onFile && single ? [command("Change extension", "rename", act.changeExtension)] : []),
      ...style,
      submenu("Select", "check", [
        command("All", undefined, () => act.select("all"), { keys: "Ctrl+A" }),
        command("Invert", undefined, () => act.select("invert")),
        command("None", undefined, () => act.select("none")),
      ]),
    ],
    single ? [command("Take ownership", "shield", act.takeOwnership, env.canElevate
      ? { color: "#c42b1c" }
      : { disabled: true, title: "Allow cmd.exe in Settings, Administrator to use this" })] : [],
    [
      submenu("Show/Hide", "search", [
        command("System files", env.showHidden ? "check" : undefined, () => act.showHidden(!env.showHidden), { check: env.showHidden }),
        command("File name extensions", env.showExtensions ? "check" : undefined, () => act.showExtensions(!env.showExtensions), { check: env.showExtensions }),
      ]),
      ...(attributes ? [attributes] : []),
    ],
    item ? [] : [
      submenu("New Folder", "open", [
        command("DateTime", undefined, () => act.newFolder("datetime")),
        command("Guid", undefined, () => act.newFolder("guid")),
      ]),
      submenu("New File", "file", (Object.keys(NEW_FILES) as NewFileKind[]).map((kind) => command(kind.toUpperCase(), undefined, () => act.newFile(kind)))),
      command("Folder Options", "settings", () => act.system("shell:::{6DFD7C5C-2451-11d3-A299-00C04F8EF6AF}")),
    ],
    [
      ...iconRow,
      ...(item || !env.folder ? [] : [command(env.pinned ? "Unpin this folder" : "Pin this folder", "pin", () => act.togglePin(env.folder!))]),
      command("Edit menu…", "menu", act.editMenu),
      ...(item ? [] : [windowsMenuRow(env)]),
    ],
  ));
}

function goToMenu(env: MenuEnv): MenuCommand {
  const { act } = env;
  const folder = (name: string, path: string) => command(name, "open", () => act.goTo(path));
  const page = (label: string, uri: string) => command(label, "settings", () => act.system(uri));
  const folders = (env.places?.folders ?? []).flatMap((place): MenuNode[] => {
    const row = folder(place.name, place.path);
    if (place.name === "ProgramData") return [row, command("Applications", "open", () => act.system("shell:appsfolder"))];
    return place.name === "Desktop" ? [separator, row] : [row];
  });
  return submenu("Go To", "forward", [
    submenu("Folder", "open", folders),
    page("Control Panel", "shell:::{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}"),
    page("All Control Panel Items", "shell:::{ED7BA470-8E54-465E-825C-99712043E01C}"),
    command("Run", "play", () => act.system("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}")),
    separator,
    submenu("Settings", "settings", [
      page("System", "ms-settings:"),
      page("About", "ms-settings:about"),
      page("Your info", "ms-settings:yourinfo"),
      command("System info", "terminal", () => act.terminal("system_information")),
      command("Search", "search", () => act.system("search-ms:")),
      page("USB", "ms-settings:usb"),
      page("Windows Update", "ms-settings:windowsupdate"),
      page("Windows Defender", "ms-settings:windowsdefender"),
      submenu("Apps", "settings", [
        page("Apps & features", "ms-settings:appsfeatures"),
        page("Default apps", "ms-settings:defaultapps"),
        page("Optional features", "ms-settings:optionalfeatures"),
        page("Startup", "ms-settings:startupapps"),
      ]),
      submenu("Personalization", "palette", [
        page("Personalization", "ms-settings:personalization"),
        page("Lock screen", "ms-settings:lockscreen"),
        page("Background", "ms-settings:personalization-background"),
        page("Colors", "ms-settings:colors"),
        page("Themes", "ms-settings:themes"),
        page("Start", "ms-settings:personalization-start"),
        page("Taskbar", "ms-settings:taskbar"),
      ]),
      submenu("Network", "settings", [
        page("Status", "ms-settings:network-status"),
        page("Ethernet", "ms-settings:network-ethernet"),
        page("Connections", "shell:::{7007ACC7-3202-11D1-AAD2-00805FC1270E}"),
      ]),
    ]),
  ]);
}

/** Pictures 5 and 8: what Windows offers for this item, between Firewol's fixed rows. */
export function itemMenu(env: MenuEnv): MenuNode[] {
  const { act, item, selected } = env;
  const folderItem = !!item?.directory && selected.length <= 1;
  // A type whose default is not "open" (Mount, Play) names the top row after it.
  const takeover = selected.length <= 1 ? inSlot(env, "apps").find((entry) => entry.default) : undefined;
  const head = pick(env, HEAD).flatMap((row): MenuNode[] => {
    const node = rowNode(env, row);
    if (row.entry.action !== "open") return [node];
    if (takeover && !row.entry.label) node.label = takeover.label;
    if (!folderItem) return [node];
    return [node, command("Open in new tab", "window", act.openInNewTab, { keys: "Ctrl+Enter", disabled: !row.enabled }), command("Open in new window", "window", act.openInNewWindow, { disabled: !row.enabled })];
  });
  const pinned = folderItem && item ? [command(env.pinned ? "Unpin from sidebar" : "Pin to sidebar", "pin", () => act.togglePin(item.path))] : [];
  const pins = [...pinned, ...shellSlot(env, "pin")];
  const live = Array.isArray(env.shell);
  const customize: MenuNode[] = folderItem ? [...pick(env, ["folder_style"]).map((row) => rowNode(env, row)), command("Change icon…", "image", act.changeIcon)] : [];
  const edit = pick(env, EDIT).filter((row) => row.entry.action !== "paste" || row.enabled).map((row) => rowNode(env, row));
  return groups(
    [
      ...head,
      ...customRows(env).map((row) => rowNode(env, row)),
      ...appRows(env, takeover),
      ...shellSlot(env, "favorite"),
      ...(pins.length > 0 ? [submenu("Pin/Unpin", "pin", pins)] : []),
      ...(live ? [submenu("More options", "more", [...shellSlot(env, "more"), separator, windowsMenuRow(env)])] : []),
    ],
    customize,
    [terminalMenu(env), fileManageMenu(env)],
    [goToMenu(env)],
    [...shellSlot(env, "open_with"), ...edit],
    shellSlot(env, "properties"),
  );
}

/** Pictures 1 to 4: the open folder's own menu. */
export function areaMenu(env: MenuEnv): MenuNode[] {
  const { act } = env;
  const views: MenuNode[] = [
    command("Details", "list", () => act.view(false), { radio: !env.grid, keys: "Ctrl+Shift+6" }),
    ...TILE_SIZES.map(([size, name, keys]) => command(name, "grid", () => act.view(true, size), { radio: env.grid && env.tile === size, keys })),
  ];
  const columns: [SortColumn, string][] = [["name", "Name"], ["modified", "Date modified"], ["type", "Type"], ["size", "Size"]];
  const sorts: MenuNode[] = [
    ...columns.map(([column, name]) => command(name, undefined, () => act.sort(column), { radio: env.sort.column === column, disabled: env.searching })),
    separator,
    command("Ascending", undefined, () => act.direction(false), { radio: !env.sort.descending, disabled: env.searching }),
    command("Descending", undefined, () => act.direction(true), { radio: env.sort.descending, disabled: env.searching }),
  ];
  const created = inSlot(env, "new").map((item) => shellNode(item, true));
  return groups(
    [submenu("View", "grid", views), submenu("Sort by", "sortUp", sorts), ...pick(env, ["refresh"]).map((row) => rowNode(env, row))],
    [terminalMenu(env), fileManageMenu(env)],
    [goToMenu(env)],
    [...pick(env, ["paste", "undo"]).map((row) => rowNode(env, row)), ...customRows(env).map((row) => rowNode(env, row)), ...appRows(env), ...shellSlot(env, "more")],
    created,
    shellSlot(env, "properties"),
  );
}

/** A sidebar pin or drive: Firewol's rows, then what Windows offers for that folder. */
export function placeMenu(env: MenuEnv, place: { open(inNewTab: boolean): void; customize?: () => void; changeIcon?: () => void; pinned: boolean; togglePin(): void; copyPath(): void }): MenuNode[] {
  const live = Array.isArray(env.shell);
  return groups(
    [command("Open", "open", () => place.open(false)), command("Open in new tab", "window", () => place.open(true))],
    [
      ...(place.customize ? [command("Customize folder…", "palette", place.customize)] : []),
      ...(place.changeIcon ? [command("Change icon…", "image", place.changeIcon)] : []),
      command(place.pinned ? "Unpin from sidebar" : "Pin to sidebar", "pin", place.togglePin),
      command("Copy path", "copy", place.copyPath),
    ],
    [...appRows(env), ...shellSlot(env, "favorite"), ...shellSlot(env, "pin")],
    live ? [...shellSlot(env, "more"), ...shellSlot(env, "properties"), windowsMenuRow(env)] : [],
  );
}
