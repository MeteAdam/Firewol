// Built-in context-menu actions and their enable rules, ported from firewol-win32 explorer/menu.rs.
// The saved order, visibility, labels and custom actions live in core settings (`settings.menu`).
import type { ElevationView, Entry, MenuActionView, MenuEntryView } from "./contract";
import type { IconName } from "./Icon.svelte";

export type BuiltIn = Exclude<MenuActionView, { custom: unknown }>;
export type Custom = Extract<MenuActionView, { custom: unknown }>;
type Details = { label: string; icon: IconName; shortcut: string; group: number };

/** Key order matches `default_menu()` in firewol-core settings.rs. */
export const BUILT_IN: Record<BuiltIn, Details> = {
  open: { label: "Open", icon: "open", shortcut: "Enter", group: 0 },
  run_as_administrator: { label: "Run as administrator…", icon: "shield", shortcut: "", group: 0 },
  containing: { label: "Open containing folder", icon: "up", shortcut: "", group: 0 },
  cut: { label: "Cut", icon: "cut", shortcut: "Ctrl+X", group: 1 },
  copy: { label: "Copy", icon: "copy", shortcut: "Ctrl+C", group: 1 },
  paste: { label: "Paste", icon: "paste", shortcut: "Ctrl+V", group: 1 },
  copy_other: { label: "Copy to other pane", icon: "toPane", shortcut: "", group: 1 },
  move_other: { label: "Move to other pane", icon: "transfer", shortcut: "", group: 1 },
  recycle: { label: "Delete", icon: "trash", shortcut: "Del", group: 1 },
  rename: { label: "Rename", icon: "rename", shortcut: "F2", group: 1 },
  folder_style: { label: "Customize this folder…", icon: "palette", shortcut: "", group: 2 },
  undo: { label: "Undo", icon: "undo", shortcut: "Ctrl+Z", group: 3 },
  refresh: { label: "Refresh", icon: "refresh", shortcut: "F5", group: 3 },
};
const CUSTOM: Details = { label: "Run action", icon: "play", shortcut: "", group: 2 };
/** Matches `MAX_ENTRIES` in the Win32 menu; the core accepts up to 64 saved entries. */
export const MAX_ROWS = 33;

export const isCustom = (action: MenuActionView): action is Custom => typeof action === "object";
export const details = (action: MenuActionView) => (isCustom(action) ? CUSTOM : BUILT_IN[action]);
export const labelOf = (entry: MenuEntryView) => entry.label || details(entry.action).label;
export const entryKey = (entry: MenuEntryView) => (isCustom(entry.action) ? `custom:${entry.action.custom.executable}:${entry.label}` : entry.action);
export const defaultMenu = (): MenuEntryView[] =>
  (Object.keys(BUILT_IN) as BuiltIn[]).map((action) => ({ action, visible: true, label: "", icon: "", color: null }));

export type MenuContext = {
  /** The right-clicked item; undefined for the empty-area menu. */
  item?: Entry;
  selectedCount: number;
  searching: boolean;
  /** The open folder has a core identity (paste and refresh targets). */
  current: boolean;
  clipboard: boolean;
  /** No transfer is running. */
  idle: boolean;
  /** The other pane shows a folder. */
  split: boolean;
  elevation: ElevationView;
};
export type MenuRow = { entry: MenuEntryView; index: number; enabled: boolean; divider: boolean };

const ITEM_ONLY = new Set<MenuActionView>(["open", "run_as_administrator", "containing", "copy", "cut", "copy_other", "move_other", "rename", "recycle"]);
const executable = (path: string) => /^([a-z]:|\\\\)\\.+\.exe$/i.test(path);

/** Returns whether a visible entry is enabled, or null when it does not apply here and stays hidden. */
function enabled(action: MenuActionView, context: MenuContext): boolean | null {
  const { item, idle } = context;
  const selected = !!item?.id;
  if (isCustom(action)) {
    return (item ? selected : context.current) && (!action.custom.administrator || (idle && context.elevation.mode !== "standard"));
  }
  if (!item && ITEM_ONLY.has(action)) return null;
  switch (action) {
    case "open": return selected;
    case "run_as_administrator":
      if (!item?.id || item.directory || context.elevation.mode === "standard" || !executable(item.path)) return null;
      return idle && (context.elevation.mode === "all_executables" || context.elevation.executables.some((path) => path.toLowerCase() === item.path.toLowerCase()));
    case "containing": return context.searching ? selected : null;
    case "copy": case "cut": return selected && idle;
    case "copy_other": case "move_other": return context.split ? selected && idle : null;
    case "rename": return selected && idle && context.selectedCount <= 1;
    // The core recycles up to 32 items as one batch with one undo.
    case "recycle": return selected && idle;
    case "paste": return context.current && context.clipboard && idle;
    // Win32 rule: a selected folder item, or the current folder from the empty area.
    case "folder_style": return item ? (item.directory ? selected : null) : context.current;
    case "undo": return idle;
    case "refresh": return context.current;
  }
}

/** The rows a context menu shows, in saved order, with dividers between groups. */
export function menuRows(menu: MenuEntryView[], context: MenuContext): MenuRow[] {
  const rows: MenuRow[] = [];
  let group: number | undefined;
  menu.forEach((entry, index) => {
    if (!entry.visible || rows.length >= MAX_ROWS) return;
    const state = enabled(entry.action, context);
    if (state === null) return;
    const next = details(entry.action).group;
    rows.push({ entry, index, enabled: state, divider: group !== undefined && group !== next });
    group = next;
  });
  return rows;
}

export function parentPath(path: string) {
  const parent = path.replace(/[\\/][^\\/]+[\\/]?$/, "");
  return /^[a-z]:$/i.test(parent) ? `${parent}\\` : parent;
}
