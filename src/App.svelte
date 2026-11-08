<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, tick, untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { api } from "./lib/api";
  import { edgeScroll } from "./lib/autoscroll";
  import {
    FOLDER_PROGRESS,
    INDEX_STATE,
    VOLUME_STATE,
    TRANSFER_PROGRESS,
    OPERATION_DONE,
    SESSION_CHANGED,
    SETTINGS_CHANGED,
    ICON_CHANGED,
    DRIVES_CHANGED,
    type SettingsView,
    type SettingsPatch,
    type AppearanceView,
    type Entry,
    type FolderStyleView,
    type ItemRef,
    type FolderProgress,
    type IndexState,
    type OperationDone,
    type SessionState,
    type Startup,
    type TransferStatus,
    type VolumeState,
    type DriveSpace,
    type DriveView,
    type ShellMenuItem,
  } from "./lib/contract";
  import FileIcon from "./lib/FileIcon.svelte";
  import FileList from "./lib/FileList.svelte";
  import FolderIcon from "./lib/FolderIcon.svelte";
  import FolderStyleSheet from "./lib/FolderStyleSheet.svelte";
  import { formatCount, formatSize, useLocale } from "./lib/format";
  import { iconChanged } from "./lib/icons.svelte";
  import Icon from "./lib/Icon.svelte";
  import { NO_FILTERS, Pane, SELECT_ALL_LIMIT, itemKey } from "./lib/pane.svelte";
  import { PRESETS, colors, hex, presetClass } from "./lib/appearance";
  import { details, isCustom, labelOf, menuRows, parentPath, splitRows, type MenuContext, type MenuRow } from "./lib/menu";
  import Breadcrumb from "./lib/Breadcrumb.svelte";
  import SearchBox from "./lib/SearchBox.svelte";
  import Sheet from "./lib/Sheet.svelte";
  import Settings from "./lib/Settings.svelte";
  import { styleKey, styleMap, tint } from "./lib/styles";
  import TabBar from "./lib/TabBar.svelte";
  import TransferPill from "./lib/TransferPill.svelte";
  import { volumes } from "./lib/volumes.svelte";
  import { PaneSlot, folderTab, restore, snapshot, type Tab } from "./lib/tabs.svelte";

  type Check = (step: string, ok: boolean, detail?: string) => void;

  const QUERIES = ["ré", "cord", "port", "résumé", "record-0000"];

  const appWindow = getCurrentWindow();
  let startup = $state<Startup>();
  let session = $state.raw<SessionState>({ pins: [], clipboard: null });
  let drives = $state.raw<DriveView[]>([]);
  let drivesTimer: ReturnType<typeof setTimeout> | undefined;
  let slots = $state<PaneSlot[]>([]);
  const panes = $derived(slots.flatMap(slot => slot.active?.kind === "folder" ? [slot.active.pane] : []));
  let lists = $state<FileList[]>([]);
  let paneCounter = 0;
  let activeIndex = $state(0);
  let settings = $state.raw<SettingsView>();
  const windowsDark = new MediaQuery("(prefers-color-scheme: dark)");
  const sidebar = $derived(settings?.sidebar ?? true);
  const gridView = $derived(settings?.grid ?? false);
  const tile = $derived(settings?.tile ?? 128);
  const TILES = [[160, "Large tiles", "Ctrl+Shift+2"], [128, "Medium tiles", "Ctrl+Shift+3"], [96, "Small tiles", "Ctrl+Shift+4"]] as const;
  const rowHeight = $derived(settings?.row ?? 32);
  const ROWS = [24, 32, 40, 52];
  const preset = $derived(presetClass(settings?.appearance.preset ?? "light_morphism"));
  const dark = $derived((preset === "light" || preset === "neu") && (settings?.appearance.follow_windows ? windowsDark.current : settings?.appearance.dark));
  let closeWarning = $state(false);
  let closeWhenFinished = false;
  let allowClose = false;
  let editing = $state(false);
  let address = $state("");
  /** A sidebar row: a pinned folder or a drive. */
  type Place = { path: string; item: ItemRef | null; name: string };
  type MenuKind = "item" | "area" | "more" | "pin" | "drive";
  let menu = $state<{ kind: MenuKind; x: number; y: number; target?: Place }>();
  /** Windows' commands for the open menu: arriving, listed, or absent (off or too slow). */
  let shellMenu = $state.raw<ShellMenuItem[] | "loading" | null>(null);
  let submenus = $state.raw<{ items: ShellMenuItem[]; x: number; y: number; row: HTMLElement }[]>([]);
  let shellRequest = 0;
  let shellLive = false;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  const SHELL_WAIT_MS = 1500;
  let toast = $state<{ message: string; undo: boolean; error: boolean }>();
  /** The running transfer, or the last result until dismissed; sent by the core every 250 ms. */
  let transfer = $state.raw<TransferStatus | null>(null);
  const transferRunning = $derived(!!transfer && transfer.phase !== "finished");
  let fatal = $state("");
  let searchBox: SearchBox | undefined = $state();
  /** The NTFS drive waiting for in-window confirmation before Windows asks for approval. */
  let volumeAsk = $state<{ drive: string; pane: Pane }>();
  let settingsSection = $state("appearance");
  let folderSheet = $state<{ item: ItemRef; name: string; path: string }>();
  /** Items waiting for the in-window Recycle Bin confirmation. */
  let recycleAsk = $state.raw<Entry[]>();
  /** Explorer asks before opening more than this many items at once. */
  const OPEN_WITHOUT_ASKING = 15;
  let openAsk = $state.raw<{ entries: Entry[]; inNewTab: boolean }>();
  let menuElement: HTMLElement | undefined = $state();
  /** Where focus returns when Escape closes the context menu. */
  let menuReturn: HTMLElement | null = null;
  // Sidebar width: dragged live, saved to the core when the drag ends (DESIGN: 232, 180–360).
  const SIDEBAR_MIN = 180;
  const SIDEBAR_MAX = 360;
  let sidebarDrag = $state<number>();
  let pcOpen = $state(stored("pc-open", "true") === "true");
  const sidebarWidth = $derived(sidebarDrag ?? settings?.sidebar_width ?? 232);
  /** Free space of the open folder's drive, for the status bar. */
  let space = $state.raw<DriveSpace & { path: string }>();
  let spaceRequest = 0;
  let addressField: HTMLInputElement | undefined = $state();
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  const activeTab = $derived(slots[activeIndex]?.active);
  const active = $derived(activeTab?.kind === "folder" ? activeTab.pane : undefined);
  const otherTab = $derived(slots[activeIndex === 0 ? 1 : 0]?.active);
  const other = $derived(otherTab?.kind === "folder" ? otherTab.pane : undefined);
  // Tabs, the title and pins look styles up by identity, so they follow edits at once.
  const styles = $derived(styleMap(settings));
  const styleOf = (id?: ItemRef | null) => (id ? styles.get(styleKey(id)) : undefined);
  const paneLabel = (pane: Pane) => styleOf(pane.identity)?.alias || pane.label;
  const tabStyle = (tab: Tab) => (tab.kind === "folder" ? styleOf(tab.pane.identity) : undefined);
  const title = $derived(activeTab?.kind === "settings" ? "Settings" : active ? paneLabel(active) : "Firewol");
  // Settings alone in the window replaces the folder chrome, as in the approved mockup.
  const settingsOnly = $derived(slots.length === 1 && activeTab?.kind === "settings");
  const focused = $derived(active?.focusedEntry);
  const selectedCount = $derived(active?.selection.size ?? 0);
  const pinned = $derived(session.pins.some((pin) => pin.path === active?.folder?.path));

  $effect(() => {
    document.title = `${title} – Firewol`;
  });

  // The status bar's free space follows the open folder and every finished file operation.
  const spacePath = $derived(active?.folder?.path);
  $effect(() => {
    if (spacePath) void loadSpace(spacePath);
  });

  async function loadSpace(path: string) {
    const request = ++spaceRequest;
    try {
      const result = await api.driveSpace(path);
      if (request === spaceRequest) space = { ...result, path };
    } catch {
      if (request === spaceRequest) space = undefined;
    }
  }

  // Listed rows carry their style from the core, so shown pages are re-read when styles change.
  let stylesSeen: string | undefined;
  $effect(() => {
    const next = JSON.stringify(settings?.folders ?? []);
    untrack(() => {
      if (stylesSeen !== undefined && stylesSeen !== next) for (const pane of panes) void pane.restyle();
      stylesSeen = next;
    });
  });

  function stored(key: string, fallback: string) {
    try {
      return localStorage.getItem(`firewol.${key}`) ?? fallback;
    } catch {
      return fallback;
    }
  }

  async function saveSettings(patch: SettingsPatch) {
    try { settings = withPending(await api.updateSettings(patch)); }
    catch (reason) { notify(String(reason), { error: true }); }
  }

  // Appearance edits (colour wheel drags) show at once and reach the core at most every 150 ms.
  let pendingAppearance: AppearanceView | undefined;
  let appearanceTimer: ReturnType<typeof setTimeout> | undefined;
  const withPending = (view: SettingsView) => (pendingAppearance ? { ...view, appearance: pendingAppearance } : view);

  function previewAppearance(appearance: AppearanceView) {
    if (!settings) return;
    pendingAppearance = appearance;
    settings = { ...settings, appearance };
    clearTimeout(appearanceTimer);
    appearanceTimer = setTimeout(() => void flushAppearance(), 150);
  }

  async function flushAppearance() {
    clearTimeout(appearanceTimer);
    const appearance = pendingAppearance;
    if (!appearance) return;
    try {
      const saved = await api.updateSettings({ appearance });
      if (pendingAppearance === appearance) pendingAppearance = undefined;
      settings = withPending(saved);
    } catch (reason) {
      pendingAppearance = undefined;
      notify(String(reason), { error: true });
      settings = await api.getSettings();
    }
  }

  function choosePreset(value: AppearanceView["preset"]) {
    if (settings) void saveSettings({ appearance: { ...settings.appearance, preset: value } });
  }

  function toggleDark() {
    if (settings) void saveSettings({ appearance: { ...settings.appearance, dark: !dark, follow_windows: false } });
  }

  function toggleSidebar() { void saveSettings({ sidebar: !sidebar }); }

  const clampSidebar = (width: number) => Math.round(Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, width)));

  async function saveSidebarWidth(width: number) {
    sidebarDrag = width;
    if (width !== settings?.sidebar_width) await saveSettings({ sidebar_width: width });
    if (sidebarDrag === width) sidebarDrag = undefined;
  }

  /** Drags the sidebar edge; the width is saved once, when the button comes up. */
  function resizeSidebar(event: PointerEvent) {
    if (event.button !== 0) return;
    const handle = event.currentTarget as HTMLElement;
    const from = event.clientX;
    const start = sidebarWidth;
    try { handle.setPointerCapture(event.pointerId); } catch { /* Synthetic pointers have no capture. */ }
    const move = (next: PointerEvent) => (sidebarDrag = clampSidebar(start + next.clientX - from));
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      if (sidebarDrag !== undefined) void saveSidebarWidth(sidebarDrag);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
    event.preventDefault();
  }

  function sidebarKey(event: KeyboardEvent) {
    const step = event.shiftKey ? 32 : 8;
    const next = event.key === "ArrowLeft" ? sidebarWidth - step
      : event.key === "ArrowRight" ? sidebarWidth + step
      : event.key === "Home" ? SIDEBAR_MIN
      : event.key === "End" ? SIDEBAR_MAX
      : undefined;
    if (next === undefined) return;
    event.preventDefault();
    void saveSidebarWidth(clampSidebar(next));
  }
  /** Details, or tiles of `size` pixels; saved for every folder tab and window. */
  function setView(grid: boolean, size = tile) {
    menu = undefined;
    if (grid !== gridView || size !== tile) void saveSettings({ grid, tile: size });
  }
  /** Ctrl+wheel must cancel WebView2's page zoom, which needs a non-passive listener. */
  function nonPassiveWheel(node: HTMLElement, handler: (event: WheelEvent) => void) {
    node.addEventListener("wheel", handler, { passive: false });
    return { destroy: () => node.removeEventListener("wheel", handler) };
  }

  /** Ctrl+wheel steps through the row heights in details view and the tile sizes in grid view. */
  function zoom(event: WheelEvent) {
    if (!event.ctrlKey) return;
    const step = (sizes: number[], now: number) => sizes[Math.max(0, Math.min(sizes.length - 1, sizes.indexOf(now) + (event.deltaY < 0 ? 1 : -1)))];
    if (gridView) setView(true, step(TILES.map(([size]) => size as number).reverse(), tile));
    else if (step(ROWS, rowHeight) !== rowHeight) void saveSettings({ row: step(ROWS, rowHeight) });
  }

  /** Page zoom must not work anywhere in the window, so this runs before any other handler. */
  function blockPageZoom(event: WheelEvent) {
    if (event.ctrlKey) event.preventDefault();
  }

  async function loadSettings() {
    settings = await api.getSettings();
    if (!settings.theme_migrated) {
      const old = PRESETS.find(([, css]) => css === stored("preset", ""));
      settings = await api.updateSettings({ appearance: old ? { ...settings.appearance, preset: old[0], dark: stored("dark", "false") === "true" } : settings.appearance, migrate_appearance: true });
    }
    try { localStorage.removeItem("firewol.preset"); localStorage.removeItem("firewol.dark"); } catch { /* A locked WebView profile can still use core settings. */ }
  }

  /** Every folder tab's pane, including tabs in the background. */
  const allPanes = () => slots.flatMap(slot => slot.tabs.flatMap(tab => tab.kind === "folder" ? [tab.pane] : []));

  function volumeChanged(state: VolumeState) {
    volumes.update(state);
    allPanes().forEach(pane => pane.volumeChanged(state));
    if (state.error) notify(state.error, { error: true });
  }

  async function startVolume() {
    const ask = volumeAsk;
    volumeAsk = undefined;
    if (!ask) return;
    try {
      volumes.update(await api.startVolumeIndex(ask.drive));
      ask.pane.setVolume(ask.drive);
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  function resultText(pane: Pane) {
    if (!pane.searchReady) return "Waiting for the index";
    const count = pane.resultCount;
    if (!count) return pane.rows ? "Counting results…" : "Searching…";
    return `${formatCount(count.count)}${count.more ? "+" : ""} ${count.count === 1 && !count.more ? "result" : "results"}`;
  }

  function newPane() {
    return new Pane(`${startup?.label ?? "main"}:${++paneCounter}`);
  }

  function notify(message: string, options: { undo?: boolean; error?: boolean } = {}) {
    clearTimeout(toastTimer);
    toast = { message, undo: options.undo ?? false, error: options.error ?? false };
    toastTimer = setTimeout(() => (toast = undefined), options.error ? 8000 : 6000);
  }

  /** Runs a command that starts a file operation; failures to start show at once. */
  async function start(action: Promise<void>) {
    menu = undefined;
    try {
      await action;
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  let doneWaiters: ((done: OperationDone) => void)[] = [];

  /** Resolves with the next operation result (used by the self-test). */
  function nextDone() {
    return new Promise<OperationDone>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("No operation result within 30 s")), 30_000);
      doneWaiters.push((done) => {
        clearTimeout(timer);
        resolve(done);
      });
    });
  }

  function finished(done: OperationDone) {
    if (closeWhenFinished) void appWindow.close();
    const waiters = doneWaiters;
    doneWaiters = [];
    waiters.forEach((resolve) => resolve(done));
    if (done.refresh) for (const pane of panes) void pane.refresh();
    if (spacePath) void loadSpace(spacePath);
    notify(done.message, { undo: done.succeeded, error: !done.succeeded });
    void api.session().then((state) => (session = state));
  }

  /** Enter and the menu's Open act on the selection; a double-click on the item under it. */
  function open(entry: Entry, inNewTab: boolean, selection = false) {
    menu = undefined;
    const pane = active;
    if (!pane) return;
    const entries = selection && entry.id && pane.selection.has(itemKey(entry.id)) ? pane.selectedEntries() : [entry];
    if (entries.length > MAX_ITEMS) notify(`Open at most ${MAX_ITEMS} items at a time`, { error: true });
    else if (entries.length > OPEN_WITHOUT_ASKING) openAsk = { entries, inNewTab };
    else void openEntries(pane, entries, inNewTab);
  }

  /** One folder replaces the view and several get tabs; files go to their Windows default app. */
  async function openEntries(pane: Pane, entries: Entry[], inNewTab: boolean) {
    const folders = entries.filter((entry) => entry.directory);
    const files = entries.flatMap((entry) => (!entry.directory && entry.id ? [entry.id] : []));
    if (folders.length === 1 && files.length === 0 && !inNewTab) void pane.open(folders[0].path);
    else for (const folder of folders) await newTab(folder.path);
    if (files.length === 0) return;
    try {
      const { navigate } = await api.openItems(files);
      if (navigate && inNewTab) void newTab(navigate);
      else if (navigate) void pane.open(navigate);
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  function confirmOpen() {
    const ask = openAsk;
    openAsk = undefined;
    lists[activeIndex]?.focus();
    if (ask && active) void openEntries(active, ask.entries, ask.inNewTab);
  }

  function cancelOpen() {
    openAsk = undefined;
    lists[activeIndex]?.focus();
  }

  async function clip(cut: boolean) {
    menu = undefined;
    const items = active?.selectedRefs() ?? [];
    if (items.length === 0) return;
    try {
      session = await api.setClipboard(items, cut);
      notify(`${cut ? "Cut" : "Copied"} ${items.length} item${items.length === 1 ? "" : "s"}`);
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  function paste() {
    const into = active?.folder?.item;
    if (into && session.clipboard) void start(api.paste(into));
  }

  function toOtherPane(moveItems: boolean) {
    const into = other?.folder?.item;
    const items = active?.selectedRefs() ?? [];
    if (into && items.length > 0) void start(api.transfer(items, into, moveItems));
  }

  // In-window drag and drop, as in the Win32 build: pointer events, not HTML5 or OLE, so
  // virtual rows can leave the DOM mid-drag. Drop on a folder row or tile, another pane,
  // a folder tab or a pin; Shift moves, Esc cancels.
  /** The core's limit for one transfer or recycle (`MAX_TRANSFER_ITEMS`). */
  const MAX_ITEMS = 32;
  type DropTarget = { item: ItemRef; name: string; path: string; element: HTMLElement };
  let drag = $state<{ entries: Entry[]; from: string; x: number; y: number; move: boolean; target?: DropTarget; refusal: string }>();
  let dragFrame = 0;
  const samePath = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
  const within = (path: string, folder: string) => samePath(path, folder) || path.toLowerCase().startsWith(`${folder.toLowerCase()}\\`);

  function dropAt(x: number, y: number): DropTarget | undefined {
    const element = document.elementFromPoint(x, y);
    if (!(element instanceof Element)) return undefined;
    const pane = (id: string | undefined) => allPanes().find((candidate) => candidate.id === id);
    const row = element.closest<HTMLElement>("[data-row]");
    const list = element.closest<HTMLElement>("[data-pane]");
    if (row && list) {
      const entry = pane(list.dataset.pane)?.entry(Number(row.dataset.row));
      if (entry?.directory && entry.id) return { item: entry.id, name: entry.style?.alias || entry.name, path: entry.path, element: row };
    }
    if (list) {
      const target = pane(list.dataset.pane);
      if (target?.folder?.item) return { item: target.folder.item, name: paneLabel(target), path: target.folder.path, element: list };
    }
    const tab = element.closest<HTMLElement>("[data-tab]");
    const tabPane = pane(tab?.dataset.tab);
    if (tab && tabPane?.identity) return { item: tabPane.identity, name: paneLabel(tabPane), path: tabPane.path, element: tab };
    const pin = element.closest<HTMLElement>("[data-pin]");
    const pinned = [...session.pins, ...drives].find((candidate) => candidate.path === pin?.dataset.pin);
    if (pin && pinned?.item) return { item: pinned.item, name: styleOf(pinned.item)?.alias || pinned.name, path: pinned.path, element: pin };
    return undefined;
  }

  function hover(x: number, y: number, move: boolean) {
    if (!drag) return;
    const target = dropAt(x, y);
    const refusal = !target ? "Drop on a folder, a pane, a tab or a pin"
      : samePath(target.path, drag.from) ? "Already in this folder"
      : drag.entries.some((entry) => within(target.path, entry.path)) ? "A folder cannot go into itself"
      : drag.entries.length > MAX_ITEMS ? `Drag at most ${MAX_ITEMS} items`
      : "";
    if (drag.target?.element !== target?.element || !!drag.refusal !== !!refusal) {
      drag.target?.element.classList.remove("drop-target");
      if (!refusal) target?.element.classList.add("drop-target");
    }
    drag.x = x;
    drag.y = y;
    drag.move = move;
    drag.target = target;
    drag.refusal = refusal;
  }

  /** Scrolls a list while the pointer rests near its top or bottom edge. */
  function autoScroll() {
    if (!drag) return;
    const list = document.elementFromPoint(drag.x, drag.y)?.closest<HTMLElement>('[role="grid"]');
    if (list && edgeScroll(list, drag.y)) hover(drag.x, drag.y, drag.move);
    dragFrame = requestAnimationFrame(autoScroll);
  }

  function beginDrag(event: PointerEvent, source: Pane) {
    const entries = source.selectedEntries().filter((entry) => entry.id);
    if (entries.length === 0 || !source.folder) return;
    menu = undefined;
    try { (event.currentTarget as Element | null)?.setPointerCapture(event.pointerId); } catch { /* Synthetic pointers have no capture. */ }
    drag = { entries, from: source.folder.path, x: event.clientX, y: event.clientY, move: event.shiftKey, refusal: "" };
    hover(event.clientX, event.clientY, event.shiftKey);
    window.addEventListener("pointermove", dragMove, true);
    window.addEventListener("pointerup", dragUp, true);
    window.addEventListener("pointercancel", endDrag, true);
    window.addEventListener("keydown", dragKey, true);
    window.addEventListener("keyup", dragKey, true);
    dragFrame = requestAnimationFrame(autoScroll);
  }

  function dragMove(event: PointerEvent) { hover(event.clientX, event.clientY, event.shiftKey); }

  function dragKey(event: KeyboardEvent) {
    if (!drag) return;
    if (event.key === "Escape") endDrag();
    else if (event.key === "Shift") hover(drag.x, drag.y, event.type === "keydown");
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  function dragUp(event: PointerEvent) {
    const current = drag;
    if (current) hover(event.clientX, event.clientY, event.shiftKey);
    const target = drag?.target;
    const refusal = drag?.refusal;
    endDrag();
    if (!current || !target || refusal) return;
    const items = current.entries.flatMap((entry) => (entry.id ? [entry.id] : []));
    void start(api.transfer(items, target.item, current.move));
  }

  function endDrag() {
    drag?.target?.element.classList.remove("drop-target");
    drag = undefined;
    cancelAnimationFrame(dragFrame);
    window.removeEventListener("pointermove", dragMove, true);
    window.removeEventListener("pointerup", dragUp, true);
    window.removeEventListener("pointercancel", endDrag, true);
    window.removeEventListener("keydown", dragKey, true);
    window.removeEventListener("keyup", dragKey, true);
  }

  function rename(entry: Entry, name: string) {
    if (entry.id) void start(api.rename(entry.id, name));
  }

  /** Asks in the window first; the core recycles the whole selection as one undoable batch. */
  function recycle() {
    menu = undefined;
    const entries = (active?.selectedEntries() ?? []).filter((entry) => entry.id);
    if (entries.length === 0) {
      notify("Select the items to move to the Recycle Bin", { error: true });
      return;
    }
    if (entries.length > MAX_ITEMS) {
      notify(`Move at most ${MAX_ITEMS} items to the Recycle Bin at a time`, { error: true });
      return;
    }
    recycleAsk = entries;
  }

  function confirmRecycle() {
    const items = (recycleAsk ?? []).flatMap((entry) => (entry.id ? [entry.id] : []));
    recycleAsk = undefined;
    lists[activeIndex]?.focus();
    if (items.length > 0) void start(api.recycle(items));
  }

  function cancelRecycle() {
    recycleAsk = undefined;
    lists[activeIndex]?.focus();
  }

  /** Focuses a sheet's primary button, so Enter confirms and Esc cancels. */
  function initialFocus(node: HTMLElement) {
    void tick().then(() => node.focus());
  }

  async function selectAll() {
    const pane = active;
    if (!pane || pane.rows === 0) return;
    const all = await pane.selectAll();
    if (!all) notify(`Selected the first ${formatCount(SELECT_ALL_LIMIT)} of ${formatCount(pane.rows)} items`);
  }

  function undo() {
    toast = undefined;
    void start(api.undo());
  }

  const isShortcut = (entry?: Entry) => !!entry && !entry.directory && /\.lnk$/i.test(entry.name);

  function menuContext(onItem: boolean): MenuContext {
    return {
      item: onItem ? focused : undefined,
      selectedCount,
      searching: !!active?.searching,
      current: !!active?.folder?.item,
      clipboard: !!session.clipboard,
      idle: !transferRunning,
      split: !!other?.folder?.item,
      elevation: settings?.elevation ?? { mode: "standard", executables: [] },
    };
  }

  /** Runs a saved menu entry; programs start in the core, which re-checks identity and policy. */
  async function runEntry(row: MenuRow, onItem: boolean) {
    menu = undefined;
    const action = row.entry.action;
    // The empty-area menu acts on the open folder, as its enable rules assume.
    const item = onItem ? focused : undefined;
    try {
      if (isCustom(action)) {
        const target = item?.id ?? active?.folder?.item;
        if (target) await api.runMenuAction(row.index, action.custom.executable, target);
        return;
      }
      switch (action) {
        case "open": if (item) open(item, false, true); break;
        case "run_as_administrator": if (item?.id) await api.runAsAdministrator(item.id); break;
        case "containing": if (item) await active?.open(parentPath(item.path)); break;
        case "copy": await clip(false); break;
        case "cut": await clip(true); break;
        case "paste": paste(); break;
        case "copy_other": toOtherPane(false); break;
        case "move_other": toOtherPane(true); break;
        case "rename": if (active) active.renaming = active.selected; break;
        case "recycle": recycle(); break;
        case "undo": undo(); break;
        case "refresh": await active?.refresh(); break;
        case "folder_style": customizeFolder(item); break;
      }
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  /** Opens the folder sheet for a folder item, or for the open folder from the empty area. */
  function customizeFolder(item: Entry | undefined) {
    const folder = active?.folder;
    const target = item ? (item.directory ? item.id : null) : folder?.item;
    if (!target || (!item && !folder)) {
      notify("Select a folder to customize it", { error: true });
      return;
    }
    folderSheet = item
      ? { item: target, name: item.name, path: item.path }
      : { item: target, name: folder!.segments.at(-1)?.name ?? folder!.path, path: folder!.path };
  }

  async function saveFolderStyle(style: FolderStyleView | null) {
    const sheet = folderSheet;
    if (!sheet) return;
    settings = withPending(await api.setFolderStyle(sheet.item, style));
    folderSheet = undefined;
  }

  async function saveOrThrow(patch: SettingsPatch) {
    settings = withPending(await api.updateSettings(patch));
  }

  /** Windows' Change Icon dialog for one folder or shortcut; the result reaches every window. */
  async function changeIcon(item: ItemRef | null | undefined) {
    menu = undefined;
    if (!item) return;
    try { await api.pickCustomIcon(item); }
    catch (reason) { notify(String(reason), { error: true }); }
  }

  /** A changed icon can add or remove a folder's key, which the listing reads from disk. */
  function windowsIconChanged(key: string) {
    iconChanged(key);
    for (const pane of panes) void pane.refresh();
    void api.session().then((state) => (session = state));
  }

  /** Device notifications arrive in bursts, so a change reads the drives once they settle. */
  function refreshDrives(delay = 0) {
    clearTimeout(drivesTimer);
    drivesTimer = setTimeout(() => void api.drives().then((list) => (drives = list), () => {}), delay);
  }

  // Reordering pins: pointer events on the sidebar, like file drags and the tab strip.
  const PIN_DRAG_THRESHOLD = 6;
  let pinDrag = $state<{ path: string; before: string | null }>();
  let pinPress: { path: string; x: number; y: number; pointerY: number; frame: number } | undefined;
  const pinRows = () => [...document.querySelectorAll<HTMLElement>(".sidebar .pin[data-pinned]")];

  function pinDown(event: PointerEvent, path: string) {
    if (event.button !== 0) return;
    pinPress = { path, x: event.clientX, y: event.clientY, pointerY: event.clientY, frame: 0 };
    window.addEventListener("pointermove", pinMove, true);
    window.addEventListener("pointerup", pinUp, true);
    window.addEventListener("pointercancel", endPinDrag, true);
    window.addEventListener("keydown", pinKey, true);
  }

  /** The pin the dragged row would land in front of; `null` is the end of the list. */
  function pinTarget(y: number) {
    const row = pinRows().find((item) => {
      const box = item.getBoundingClientRect();
      return y < box.top + box.height / 2;
    });
    return row?.dataset.pin ?? null;
  }

  function pinMove(event: PointerEvent) {
    if (!pinPress) return;
    pinPress.pointerY = event.clientY;
    if (!pinDrag) {
      if (Math.hypot(event.clientX - pinPress.x, event.clientY - pinPress.y) < PIN_DRAG_THRESHOLD) return;
      menu = undefined;
      pinPress.frame = requestAnimationFrame(pinScroll);
    }
    pinDrag = { path: pinPress.path, before: pinTarget(event.clientY) };
  }

  /** Long pin lists scroll while the pointer rests near the sidebar's edges. */
  function pinScroll() {
    if (!pinPress) return;
    const side = document.querySelector<HTMLElement>(".sidebar");
    if (side && pinDrag && edgeScroll(side, pinPress.pointerY)) pinDrag = { ...pinDrag, before: pinTarget(pinPress.pointerY) };
    pinPress.frame = requestAnimationFrame(pinScroll);
  }

  /** The drop would leave the row where it is, so no insertion line is shown for it. */
  function pinStays(path: string, before: string | null) {
    const pins = session.pins.map((pin) => pin.path);
    const at = pins.indexOf(path);
    return before === path || (before ?? null) === (pins[at + 1] ?? null);
  }

  function pinUp() {
    const moved = pinDrag;
    endPinDrag();
    if (!moved) return;
    // The button that was pressed would otherwise open its folder when the press ends.
    const swallow = (event: Event) => { event.preventDefault(); event.stopPropagation(); };
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, true));
    if (pinStays(moved.path, moved.before)) return;
    api.movePin(moved.path, moved.before).then((state) => (session = state), (reason) => notify(String(reason), { error: true }));
  }

  function pinKey(event: KeyboardEvent) {
    if (event.key !== "Escape" || !pinDrag) return;
    event.preventDefault();
    event.stopPropagation();
    endPinDrag();
  }

  function endPinDrag() {
    if (pinPress) cancelAnimationFrame(pinPress.frame);
    pinPress = undefined;
    pinDrag = undefined;
    window.removeEventListener("pointermove", pinMove, true);
    window.removeEventListener("pointerup", pinUp, true);
    window.removeEventListener("pointercancel", endPinDrag, true);
    window.removeEventListener("keydown", pinKey, true);
  }

  function togglePcSection() {
    pcOpen = !pcOpen;
    try { localStorage.setItem("firewol.pc-open", String(pcOpen)); } catch { /* Only a convenience. */ }
  }

  async function togglePin(path: string | undefined) {
    menu = undefined;
    if (!path) return;
    try {
      session = await api.togglePin(path);
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  async function toggleSplit() {
    menu = undefined;
    if (slots.length > 1) {
      const second = slots[1].active;
      if (second?.kind === "folder") await second.pane.suspend();
      slots[0].tabs = [...slots[0].tabs, ...slots[1].tabs];
      slots = [slots[0]];
      lists = [lists[0]];
      activeIndex = 0;
      return;
    }
    const pane = newPane();
    const path = active?.path || startup?.path;
    slots = [...slots, new PaneSlot(folderTab(pane))];
    if (path) await pane.open(path);
  }

  async function selectTab(slot: PaneSlot, tab: Tab) {
    const previous = slot.active;
    if (previous?.id === tab.id && (tab.kind === "settings" || tab.pane.folder)) return;
    const activation = ++slot.activation;
    activeIndex = slots.indexOf(slot);
    slot.activeTab = tab.id;
    try {
      if (previous?.kind === "folder" && previous.id !== tab.id) await previous.pane.suspend();
      if (activation !== slot.activation) return;
      if (tab.kind === "folder") await tab.pane.resume();
      await tick();
      if (activation === slot.activation && tab.kind === "folder") lists[activeIndex]?.focus();
    } catch (reason) { notify(String(reason), { error: true }); }
  }

  async function newTab(path = active?.path || startup?.path || "", slot = slots[activeIndex]) {
    if (!slot) return;
    const pane = newPane();
    pane.path = path;
    const tab = folderTab(pane);
    slot.tabs = [...slot.tabs, tab];
    await selectTab(slot, tab);
    return tab;
  }

  async function closeTab(slot: PaneSlot, tab: Tab, detached = false) {
    if (tab.kind === "folder") { if (detached) tab.pane.release(); else tab.pane.close(); }
    const index = slot.tabs.indexOf(tab);
    const wasActive = slot.activeTab === tab.id;
    slot.tabs = slot.tabs.filter(current => current.id !== tab.id);
    if (!slot.tabs.length) {
      if (slots.length > 1) { slots = slots.filter(current => current !== slot); activeIndex = 0; }
      else await newTab(startup?.path, slot);
    } else if (wasActive) {
      const next = slot.tabs[Math.min(index, slot.tabs.length - 1)];
      slot.activeTab = "";
      await selectTab(slot, next);
    }
  }

  function cycleTab(backwards = false) {
    const slot = slots[activeIndex];
    if (!slot) return;
    const position = slot.tabs.findIndex(tab => tab.id === slot.activeTab);
    void selectTab(slot, slot.tabs[(position + (backwards ? -1 : 1) + slot.tabs.length) % slot.tabs.length]);
  }

  async function detachTab(slot: PaneSlot, tab: Tab) {
    try { await api.adoptPane(snapshot(tab)); await closeTab(slot, tab, true); }
    catch (reason) { notify(String(reason), { error: true }); if (tab.kind === "folder") await tab.pane.resume(); }
  }

  function openSettings(section?: string) {
    menu = undefined;
    if (section) settingsSection = section;
    const slot = slots[activeIndex];
    if (!slot) return;
    let tab = slot.tabs.find(tab => tab.kind === "settings");
    if (!tab) { tab = { kind: "settings", id: `${startup?.label}:settings-${++paneCounter}` }; slot.tabs = [...slot.tabs, tab]; }
    void selectTab(slot, tab);
  }

  function newWindow() {
    menu = undefined;
    if (active?.folder) void api.openWindow(active.folder.path, false).catch(reason => notify(String(reason), { error: true }));
  }

  function editAddress() {
    if (!active?.folder) return;
    address = active.folder.path;
    editing = true;
    void tick().then(() => addressField?.select());
  }

  function submitAddress(event: SubmitEvent) {
    event.preventDefault();
    editing = false;
    if (address.trim()) void active?.open(address.trim());
  }

  /** Opens a menu that fits the window, with focus on its first enabled item. */
  async function showMenu(kind: MenuKind, x: number, y: number, target?: Place) {
    if (!menu) menuReturn = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    menu = { kind, x, y, target };
    loadShellMenu(kind, target);
    await tick();
    if (!menuElement || !menu) return;
    const box = menuElement.getBoundingClientRect();
    menu.x = Math.max(8, Math.min(x, window.innerWidth - box.width - 8));
    menu.y = Math.max(8, Math.min(y, window.innerHeight - box.height - 8));
    menuItems()[0]?.focus();
  }

  /** Keeps Firewol's rows instant: Windows' commands join the menu when they are ready. */
  function loadShellMenu(kind: MenuKind, target?: Place) {
    const request = ++shellRequest;
    shellMenu = null;
    submenus = [];
    if (kind === "more" || settings?.shell_items === false) return;
    const folder = active?.folder?.item;
    const items = kind === "item" ? active?.selectedRefs() ?? []
      : kind === "area" ? (folder ? [folder] : [])
      : target?.item ? [target.item] : [];
    if (items.length === 0 || items.length > MAX_ITEMS) return;
    shellMenu = "loading";
    shellLive = true;
    const late = new Promise<never>((_, reject) => setTimeout(() => reject(new Error("Timed out")), SHELL_WAIT_MS));
    void Promise.race([api.shellMenuOpen(items, kind === "area"), late]).then(
      (list) => { if (request === shellRequest) { shellMenu = list; void fitMenu(); } },
      () => { if (request === shellRequest) shellMenu = null; },
    );
  }

  /** The Windows rows make the menu taller, so it may need to move up. */
  async function fitMenu() {
    await tick();
    if (!menuElement || !menu) return;
    menu.y = Math.max(8, Math.min(menu.y, window.innerHeight - menuElement.getBoundingClientRect().height - 8));
  }

  function closeShellMenu() {
    shellRequest++;
    clearTimeout(hoverTimer);
    shellMenu = null;
    submenus = [];
    if (shellLive) {
      shellLive = false;
      void api.shellMenuClose();
    }
  }

  $effect(() => {
    if (!menu) untrack(closeShellMenu);
  });

  /** Both end the Windows menu in the core, so the interface must not close it again. */
  function leaveMenu() {
    shellLive = false;
    menu = undefined;
  }

  function runShell(item: ShellMenuItem, level: number, event: MouseEvent) {
    if (item.children.length > 0) {
      void openSubmenu(event.currentTarget as HTMLElement, item, level, true);
      return;
    }
    const scale = window.devicePixelRatio;
    const [x, y] = [Math.round(event.screenX * scale), Math.round(event.screenY * scale)];
    leaveMenu();
    api.shellMenuInvoke(item.id, x, y).catch((reason) => notify(String(reason), { error: true }));
  }

  function showNativeMenu() {
    leaveMenu();
    api.shellMenuNative().catch((reason) => notify(String(reason), { error: true }));
  }

  async function openSubmenu(row: HTMLElement, item: ShellMenuItem, level: number, focus: boolean) {
    clearTimeout(hoverTimer);
    const panelAt = () => document.querySelectorAll<HTMLElement>(".shell-sub")[level];
    const focusFirst = () => panelAt()?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    if (submenus[level]?.row === row) {
      if (focus) focusFirst();
      return;
    }
    const box = row.getBoundingClientRect();
    submenus = [...submenus.slice(0, level), { items: item.children, x: box.right - 4, y: box.top - 7, row }];
    await tick();
    const panel = panelAt();
    if (!panel || submenus[level]?.row !== row) return;
    const size = panel.getBoundingClientRect();
    const x = box.right - 4 + size.width > window.innerWidth - 8 ? Math.max(8, box.left - size.width + 4) : box.right - 4;
    const y = Math.max(8, Math.min(box.top - 7, window.innerHeight - size.height - 8));
    submenus = submenus.map((entry, index) => (index === level ? { ...entry, x, y } : entry));
    if (focus) {
      await tick();
      focusFirst();
    }
  }

  function shellHover(row: HTMLElement, item: ShellMenuItem, level: number) {
    clearTimeout(hoverTimer);
    if (item.children.length > 0) hoverTimer = setTimeout(() => void openSubmenu(row, item, level, false), 120);
    else if (submenus.length > level) submenus = submenus.slice(0, level);
  }

  function submenuKey(event: KeyboardEvent, index: number) {
    const panel = event.currentTarget as HTMLElement;
    const items = [...panel.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const to = (position: number) => items[(position + items.length) % items.length]?.focus();
    if (event.key === "ArrowDown") to(at + 1);
    else if (event.key === "ArrowUp") to(at - 1);
    else if (event.key === "Home") to(0);
    else if (event.key === "End") to(items.length - 1);
    else if (event.key === "ArrowRight" && items[at]?.getAttribute("aria-haspopup")) items[at].click();
    else if (event.key === "ArrowLeft" || event.key === "Escape") {
      const row = submenus[index].row;
      submenus = submenus.slice(0, index);
      row.focus();
    } else if (event.key === "Tab") closeMenu(true);
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  /** Right-click, Shift+F10 or the Menu key on a sidebar row. */
  function placeMenu(event: MouseEvent | KeyboardEvent, kind: "pin" | "drive", place: Place) {
    if (event instanceof KeyboardEvent && event.key !== "ContextMenu" && !(event.shiftKey && event.key === "F10")) return;
    event.preventDefault();
    event.stopPropagation();
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    if (event instanceof MouseEvent) void showMenu(kind, event.clientX, event.clientY, place);
    else void showMenu(kind, box.left + 24, box.bottom, place);
  }

  function openPlace(place: Place, inNewTab: boolean) {
    menu = undefined;
    if (inNewTab) void newTab(place.path);
    else void active?.open(place.path);
  }

  function customizePlace(place: Place) {
    menu = undefined;
    if (place.item) folderSheet = { item: place.item, name: place.name, path: place.path };
  }

  function copyPath(path: string | undefined) {
    menu = undefined;
    if (path) void navigator.clipboard.writeText(path);
  }

  /** Windows' browser menu (Back, Print, …) must never appear; text fields keep theirs. */
  function blockBrowserMenu(event: MouseEvent) {
    if (!(event.target as Element | null)?.closest("input, textarea, [contenteditable]")) event.preventDefault();
  }

  const menuItems = () => [...(menuElement?.querySelectorAll<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)') ?? [])];

  function closeMenu(restoreFocus: boolean) {
    menu = undefined;
    if (restoreFocus && menuReturn?.isConnected) menuReturn.focus();
    menuReturn = null;
  }

  /** Arrow keys move through the menu; Left and Right move along the icon row. */
  function menuKey(event: KeyboardEvent) {
    const items = menuItems();
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const row = (item?: Element) => !!item?.closest(".quick");
    const to = (index: number) => items[(index + items.length) % items.length]?.focus();
    if (event.key === "ArrowDown") {
      // From the icon row, Down leaves the row instead of walking along it.
      if (row(items[at])) {
        const below = items.findIndex((item, index) => index > at && !row(item));
        if (below >= 0) to(below);
      } else to(at + 1);
    } else if (event.key === "ArrowUp") {
      if (row(items[at])) to(-1);
      else if (at > 0 && row(items[at - 1])) to(items.findIndex((item) => row(item)));
      else to(at - 1);
    } else if ((event.key === "ArrowRight" || event.key === "ArrowLeft") && row(items[at])) {
      const quick = items.filter((item) => row(item));
      const next = quick[(quick.indexOf(items[at]) + (event.key === "ArrowRight" ? 1 : -1) + quick.length) % quick.length];
      next?.focus();
    } else if (event.key === "ArrowRight" && items[at]?.getAttribute("aria-haspopup")) {
      items[at].click();
    } else if (event.key === "Home") to(0);
    else if (event.key === "End") to(items.length - 1);
    else if (event.key === "Escape") closeMenu(true);
    else if (event.key === "Tab") { closeMenu(true); }
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  function shortcut(event: KeyboardEvent) {
    // An open sheet is modal: window shortcuts must not act behind it.
    if (document.querySelector("dialog[open]")) return;
    const key = event.key.toLowerCase();
    const typing = event.target instanceof HTMLInputElement;
    if (event.key === "Escape" && menu) menu = undefined;
    else if (event.key === "Escape" && editing) editing = false;
    else if (event.ctrlKey && key === "l") editAddress();
    else if (event.ctrlKey && key === "f") searchBox?.focus();
    else if (event.ctrlKey && event.shiftKey && key === "n" && activeTab) void detachTab(slots[activeIndex], activeTab);
    else if (event.ctrlKey && key === "n") newWindow();
    else if (event.ctrlKey && key === "t") void newTab();
    else if (event.ctrlKey && key === "w" && activeTab) void closeTab(slots[activeIndex], activeTab);
    else if (event.ctrlKey && event.key === "Tab") cycleTab(event.shiftKey);
    else if (event.ctrlKey && event.key === ",") openSettings();
    else if (event.ctrlKey && event.key === "\\") void toggleSplit();
    else if (event.ctrlKey && key === "b") toggleSidebar();
    else if (event.ctrlKey && event.shiftKey && event.code === "Digit6") setView(false);
    else if (event.ctrlKey && event.shiftKey && ["Digit2", "Digit3", "Digit4"].includes(event.code)) setView(true, TILES[Number(event.code.at(-1)) - 2][0]);
    else if (event.altKey && event.key === "ArrowLeft") void active?.back();
    else if (event.altKey && event.key === "ArrowRight") void active?.forward();
    else if (event.altKey && event.key === "ArrowUp") void active?.up();
    else if (event.key === "F5") void active?.refresh();
    else if (typing) return;
    else if (event.ctrlKey && key === "c") void clip(false);
    else if (event.ctrlKey && key === "x") void clip(true);
    else if (event.ctrlKey && key === "v") paste();
    else if (event.ctrlKey && key === "z") undo();
    else if (event.ctrlKey && key === "a" && active) void selectAll();
    else return;
    event.preventDefault();
  }

  /** Mouse buttons 3 and 4 drive the pane under the pointer; WebView2 must never see them. */
  function sideButton(event: MouseEvent) {
    if (event.button !== 3 && event.button !== 4) return;
    event.preventDefault();
    if (event.type !== "mouseup" || document.querySelector("dialog[open]")) return;
    const id = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>("[data-pane]")?.dataset.pane;
    const pane = allPanes().find((candidate) => candidate.id === id) ?? active;
    void (event.button === 3 ? pane?.back() : pane?.forward());
  }

  const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));
  const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

  async function painted() {
    await tick();
    await frame();
    await frame();
  }

  /** Drives the Settings tab through its real controls; leaves the appearance as it found it. */
  async function settingsSelfTest(check: Check) {
    const root = document.querySelector<HTMLElement>(".settings")!;
    const shell = document.querySelector<HTMLElement>(".shell")!.parentElement!;
    const original = settings!.appearance;
    const button = (text: string) => [...root.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.textContent?.trim() === text);
    const type = (input: HTMLInputElement, value: string, event: "input" | "change") => {
      input.value = value;
      input.dispatchEvent(new Event(event, { bubbles: true }));
    };
    button("Glassmorphism")?.click();
    await painted();
    check("a preset card applies its theme at once", shell.classList.contains("p-glass"));
    await flushAppearance();
    check("the chosen preset persists through the core", (await api.getSettings()).appearance.preset === "glassmorphism");
    type(root.querySelector<HTMLInputElement>('input[aria-label="Accent colour"]')!, "#123456", "change");
    await painted();
    check("a typed hex overrides the accent token", getComputedStyle(shell).getPropertyValue("--accent").trim().toLowerCase() === "#123456");
    const wheel = root.querySelector<HTMLCanvasElement>("canvas")!.getBoundingClientRect();
    root.querySelector("canvas")!.dispatchEvent(new PointerEvent("pointerdown", { clientX: wheel.right - 4, clientY: wheel.top + wheel.height / 2, pointerId: 1, bubbles: true }));
    root.querySelector("canvas")!.dispatchEvent(new PointerEvent("pointerup", { pointerId: 1, bubbles: true }));
    const picked = settings!.appearance.accent;
    check("the colour wheel picks a hue at the kept brightness", !!picked && picked[0] === 0x56 && picked[0] > picked[1] + 40 && picked[0] > picked[2] + 40, picked?.join());
    button("Reset to preset")?.click();
    await flushAppearance();
    const reset = await api.getSettings();
    check("Reset to preset clears colour overrides", reset.appearance.accent === null && reset.appearance.preset === "glassmorphism");
    button("Circle")?.click();
    button("Left")?.click();
    await painted();
    const caption = document.querySelector(".caption");
    check("window controls follow shape and side", !!caption?.classList.contains("circle") && caption.classList.contains("left"));
    const find = root.querySelector<HTMLInputElement>('input[type="search"]')!;
    type(find, "uac", "input");
    await painted();
    const links = [...root.querySelectorAll(".s-link")].map((link) => link.textContent?.trim()).join();
    check("Find a setting filters sections", links === "Administrator", links);
    type(find, "", "input");
    previewAppearance(original);
    await flushAppearance();
    check("appearance restores through the core", (await api.getSettings()).appearance.preset === original.preset);

    const initialMenu = JSON.stringify(settings!.menu);
    button("Context menu")?.click();
    await painted();
    root.querySelector<HTMLInputElement>('input[aria-label="Show Copy"]')!.click();
    await waitFor(() => settings!.menu.some((entry) => entry.action === "copy" && !entry.visible), 5000, "hiding Copy");
    check("the menu editor hides an action", true);
    const form = root.querySelector<HTMLFormElement>("form.add")!;
    const executable = form.querySelector<HTMLInputElement>('input[aria-label="Executable path"]')!;
    type(form.querySelector<HTMLInputElement>('input[aria-label="Custom action label"]')!, "Self-test action", "input");
    type(executable, "notepad", "input");
    form.requestSubmit();
    await waitFor(() => !!root.querySelector('[role="alert"]'), 5000, "a refused custom action");
    const refusal = root.querySelector('[role="alert"]')?.textContent ?? "";
    check("an invalid executable is refused inline", /absolute path/i.test(refusal), refusal);
    // Saved only: the self-test never runs this program.
    type(executable, "C:\\Tools\\firewol-self-test.exe", "input");
    form.requestSubmit();
    await waitFor(() => settings!.menu.some((entry) => isCustom(entry.action) && entry.label === "Self-test action"), 5000, "a custom action");
    check("a custom action is added", !root.querySelector('[role="alert"]'));

    const radio = (name: string) => [...root.querySelectorAll<HTMLButtonElement>('button[role="radio"]')].find((item) => item.querySelector("strong")?.textContent === name)!;
    button("Administrator")?.click();
    await painted();
    radio("Any executable").click();
    await painted();
    check("Any executable asks for confirmation first", !!root.querySelector('[aria-label="Allow any executable"]') && settings!.elevation.mode === "standard");
    button("Cancel")?.click();
    radio("Allowlisted executables").click();
    await waitFor(() => settings!.elevation.mode === "allowlist", 5000, "allowlist mode");
    const allow = root.querySelector<HTMLFormElement>("form.add")!;
    type(allow.querySelector<HTMLInputElement>("input")!, "C:\\Windows\\System32\\whoami.exe", "input");
    allow.requestSubmit();
    await waitFor(() => settings!.elevation.executables.length === 1, 5000, "an allowlisted executable");
    check("an allowlisted executable is saved", /whoami\.exe$/i.test(settings!.elevation.executables[0]), settings!.elevation.executables[0]);
    radio("Standard").click();
    await waitFor(() => settings!.elevation.mode === "standard", 5000, "standard mode");
    root.querySelector<HTMLButtonElement>('button[aria-label^="Remove "]')!.click();
    await waitFor(() => settings!.elevation.executables.length === 0, 5000, "an empty allowlist");
    check("administrator policy returns to Standard", true);
    return initialMenu;
  }

  /**
   * Styles the fixture's `z-folder` through the context menu and the real sheet, checks the list,
   * Tags column, tab, title and pin, then clears the style from the empty-area menu.
   */
  async function folderStyleSelfTest(check: Check, pane: Pane, folderPath: string) {
    const ALIAS = "Self-test alias";
    const sheet = () => document.querySelector<HTMLDialogElement>('dialog[aria-label="Customize folder"]');
    const field = (label: string) => sheet()!.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
    const sheetButton = (name: string) =>
      [...sheet()!.querySelectorAll<HTMLButtonElement>("button")].find((item) => item.getAttribute("aria-label") === name || item.textContent?.trim() === name)!;
    const typeText = (input: HTMLInputElement, value: string) => {
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    };
    const openSheet = async (kind: "item" | "area") => {
      showMenu(kind, 40, 40);
      await painted();
      const item = [...document.querySelectorAll<HTMLButtonElement>(".menu .mi")].find((entry) => entry.querySelector(".label")?.textContent?.trim() === "Customize folder…");
      if (!item?.disabled) item?.click();
      await waitFor(() => !!sheet(), 5000, "the folder sheet");
      return item;
    };
    const tabLabel = () => document.querySelector(".tab.on .label > span:last-child")?.textContent;
    const pinLabels = () => [...document.querySelectorAll(".sidebar .pin > span:last-child")].map((label) => label.textContent);

    await pane.open(folderPath);
    const row = (pane.pages[0] ?? []).findIndex((entry) => entry.name === "z-folder");
    await pane.select(row);
    const folder = pane.entry(row);
    check("find z-folder", !!folder?.id, names(pane).join());
    const key = styleKey(folder!.id!);
    const item = await openSheet("item");
    check("Customize folder… is enabled on a folder", !!item && !item.disabled);
    check("the sheet opens for the right-clicked folder", !!sheet()?.textContent?.includes(folder!.path));
    typeText(field("Display name"), ALIAS);
    sheetButton("Teal").click();
    typeText(field("Add tag"), "alpha, beta");
    await painted();
    sheetButton("Save").click();
    await waitFor(() => !sheet() && pane.entry(row)?.style?.alias === ALIAS, 5000, "the saved style");
    const saved = (await api.getSettings()).folders.find((styled) => styleKey(styled) === key)?.style;
    check("the sheet saves alias, colour and tags through the core", saved?.alias === ALIAS && saved.color?.join() === "20,160,160" && saved.tags.join() === "alpha,beta" && saved.icon === "", JSON.stringify(saved));
    await painted();
    const cells = document.getElementById(`pane-0-row-${row}`);
    check("the alias replaces the name in the list", cells?.querySelector(".nm")?.textContent === ALIAS, cells?.textContent ?? "no row");
    const shape = cells?.querySelector(".folder-icon");
    check("the folder colour tints the folder icon", !!shape && getComputedStyle(shape).color === "rgb(20, 160, 160)", shape ? getComputedStyle(shape).color : "no icon");
    const tags = [...(cells?.querySelectorAll(".tag") ?? [])].map((tag) => tag.textContent).join();
    check("a Tags column lists the folder's tags", !!document.querySelector(".tags-head") && tags === "alpha,beta", tags);
    await pane.refresh();
    check("a reload lists the style from the core", pane.entry(row)?.style?.alias === ALIAS && pane.tagged);

    await pane.open(folder!.path);
    await painted();
    check("the alias names the tab and the window", tabLabel() === ALIAS && document.title.startsWith(ALIAS), `${tabLabel()} / ${document.title}`);
    await togglePin(folder!.path);
    await painted();
    check("the pinned folder shows the alias", pinLabels().includes(ALIAS), pinLabels().join());
    await openSheet("area");
    check("the empty-area menu edits the open folder's saved style", field("Display name").value === ALIAS && sheetButton("Clear style") !== undefined);
    sheetButton("Icon ★").click();
    sheetButton("Save").click();
    await waitFor(() => !sheet() && styleOf(pane.identity)?.icon === "★", 5000, "the icon glyph");
    await painted();
    const glyph = document.querySelector(".sidebar .pin .folder-glyph");
    check("the icon glyph replaces the folder shape in the folder colour", glyph?.textContent === "★" && getComputedStyle(glyph).color === "rgb(20, 160, 160)", glyph?.outerHTML ?? "no glyph");
    await openSheet("area");
    sheetButton("Clear style").click();
    await waitFor(() => !sheet() && !styleOf(pane.identity), 5000, "the cleared style");
    await painted();
    const remaining = (await api.getSettings()).folders.length;
    check("Clear style removes the alias everywhere", tabLabel() === "z-folder" && pinLabels().includes("z-folder") && remaining === 0, `${tabLabel()} / ${pinLabels().join()} / ${remaining}`);
    await togglePin(folder!.path);
    await pane.back();
    await waitFor(() => pane.folder?.path === folderPath && !pane.tagged, 5000, "the unstyled listing");
    await painted();
    check("the Tags column hides when no listed folder has tags", !document.querySelector(".tags-head") && !pane.entry(row)?.style);
  }

  const names = (pane: Pane) => (pane.pages[0] ?? []).map((entry) => entry.name);

  async function waitFor(ready: () => boolean, timeout: number, what: string) {
    const start = performance.now();
    while (!ready()) {
      if (performance.now() - start > timeout) throw new Error(`Timed out waiting for ${what}`);
      await sleep(25);
    }
  }

  // Chromium-only, rounded heap figures that let the gate separate JavaScript from the rest.
  function heap() {
    const memory = (performance as Performance & { memory?: { usedJSHeapSize: number; totalJSHeapSize: number } }).memory;
    return memory ? { used: memory.usedJSHeapSize, total: memory.totalJSHeapSize } : null;
  }

  async function benchmark(info: Startup, firstListMs: number) {
    const main = panes[0];
    const marks: Record<string, number> = { start: Date.now() };
    const heaps: Record<string, unknown> = { start: heap() };
    const scrollFps = await lists[0].benchmarkScroll(info.label === "main" ? 3000 : 1000);
    marks.scrolled = Date.now();
    heaps.scrolled = heap();
    const lastRowVerified = await lists[0].verifyLast();
    marks.lastRow = Date.now();
    for (const list of lists.slice(1)) await list.benchmarkScroll(1000);
    const report: Record<string, unknown> = {
      label: info.label,
      panes: panes.length,
      sortMs: main.sortMs,
      rows: main.count,
      firstListMs,
      scrollFps,
      lastRowVerified,
      jsReadyMs: info.sinceStartMs,
      marks,
      heaps,
    };
    if (info.label === "main") {
      // In-app navigation into the large folder, separate from process and WebView2 startup.
      const navigations: number[] = [];
      for (let sample = 0; sample < 5; sample++) {
        await main.up();
        await painted();
        const start = performance.now();
        await main.open(info.path);
        await painted();
        navigations.push(performance.now() - start);
      }
      navigations.sort((a, b) => a - b);
      report.navigateMedianMs = navigations[2];
      report.navigateMaxMs = navigations[4];
      const indexStart = performance.now();
      await main.startIndex();
      await waitFor(() => main.index === "ready", 180_000, "the folder index");
      marks.indexed = Date.now();
      report.folderIndexMs = performance.now() - indexStart;
      report.indexedItems = main.indexCount;
      const times: number[] = [];
      for (let sample = 0; sample < 30; sample++) {
        const start = performance.now();
        await main.search(QUERIES[sample % QUERIES.length]);
        if (!main.entry(0)) throw new Error("Expected search results in the fixture");
        await painted();
        times.push(performance.now() - start);
      }
      await main.search("");
      await painted();
      marks.searched = Date.now();
      heaps.searched = heap();
      times.sort((a, b) => a - b);
      report.searchP95Ms = times[Math.ceil(times.length * 0.95) - 1];
      report.searchSamples = times.length;
      // Keep several suspended tabs per slot during the four-window gate.
      const slot = slots[0];
      const original = slot.active;
      await newTab(info.path, slot);
      await newTab(info.path, slot);
      await selectTab(slot, original);
      report.tabs = slot.tabs.length;
    }
    await api.saveReport(report);
    if (info.extraWindows > 0) {
      await sleep(info.idleMs);
      if (slots.length === 1) await toggleSplit();
      await painted();
      for (let window = 0; window < info.extraWindows; window++) await api.openWindow(info.path, true);
    }
    if (info.combined) await combinedLoad(info, info.combined);
  }

  /**
   * M4.4 worst case: on the gate's signal the main window copies a 256 MiB file through the
   * session (verified, with progress events) in a new tab while every other window builds the
   * folder index of its 100k listing.
   */
  async function combinedLoad(info: Startup, folder: string) {
    await api.awaitSignal("combined");
    const start = performance.now();
    const report: Record<string, unknown> = { label: info.label };
    try {
      if (info.label === "main") {
        const tab = await newTab(folder, slots[0]);
        if (tab?.kind !== "folder") throw new Error("No folder tab for the transfer");
        await waitFor(() => tab.pane.complete && !!tab.pane.pages[0], 10_000, "the transfer folder");
        const source = tab.pane.pages[0].find((entry) => entry.name === "source.bin");
        const dest = tab.pane.pages[0].find((entry) => entry.name === "dest");
        if (!source?.id || !dest?.id) throw new Error("The transfer folder needs source.bin and dest");
        const done = nextDone();
        await api.transfer([source.id], dest.id, false);
        const result = await done;
        report.transferOk = result.succeeded;
        report.transferMessage = result.message;
        report.tabs = slots.reduce((count, slot) => count + slot.tabs.length, 0);
      } else {
        const pane = panes[0];
        const folder = pane.folder?.id;
        await pane.startIndex();
        await waitFor(() => pane.index === "ready" || !!pane.error, 120_000, "the folder index");
        if (pane.error) throw new Error(pane.error);
        // The main window's finished copy reloads every pane; the index must survive it.
        report.sameFolder = pane.folder?.id === folder;
        report.indexedItems = pane.indexCount;
      }
    } catch (reason) {
      report.error = String(reason);
    }
    report.ms = performance.now() - start;
    await api.saveReport(report, "combined");
  }

  /**
   * Drives the real interface functions against a fixture with `source\note.txt` and an
   * empty `dest` folder next to it, then saves each step's result.
   */
  /** M4.3-f: debounce, paged results, regex, filters and the NTFS scope chip (declined). */
  async function searchSteps(source: Pane, check: Check) {
    activeIndex = slots.findIndex(slot => slot.active?.kind === "folder" && slot.active.pane === source);
    await source.startIndex();
    await waitFor(() => source.index === "ready", 10000, "the folder index");
    const searchCall = api.search;
    let searches = 0;
    api.search = (request) => { searches++; return searchCall(request); };
    try {
      for (const text of ["i", "it", "ite", "item", "item-"]) source.input(text);
      check("typing waits before searching", searches === 0 && source.query === "item-");
      await waitFor(() => source.resultCount !== null, 5000, "debounced results");
      check("a burst of keys runs one search", searches === 1, `${searches} searches`);
    } finally {
      api.search = searchCall;
    }
    check("results count every match past the first page", source.resultCount?.count === 300 && !source.resultCount.more && source.rows === 300, JSON.stringify(source.resultCount));
    await source.loadPage(256);
    check("later result pages load on demand", source.entry(299)?.name.startsWith("item-") === true && Object.keys(source.resultPages).length === 2);
    await source.search("report");
    check("search covers subfolders", source.rows === 1 && source.entry(0)?.path.endsWith("z-folder\\report-2026.log") === true, source.entry(0)?.path);
    const found = () => Array.from({ length: source.rows }, (_, row) => source.entry(row)?.name).sort().join();
    source.setRegex(true);
    await source.search("^file\\d+\\.txt$");
    check("regular expression search", found() === "file10.txt,file2.txt", found());
    await source.search("(");
    check("an invalid expression explains itself", source.rows === 0 && /regular expression/i.test(source.searchError), source.searchError);
    source.setFilters({ ...NO_FILTERS, minBytes: 1024 });
    await source.search(".");
    check("size filter", found() === "report-2026.log", found());
    source.setFilters({ ...NO_FILTERS, modifiedAfter: Math.floor(Date.now() / 1000) + 86_400 });
    await source.search(".");
    check("date filter", source.rows === 0 && !source.searchError, source.searchError);
    source.setFilters(NO_FILTERS);
    await source.search(".");
    await tick();
    const box = document.querySelector<HTMLElement>('.toolbar [role="search"]');
    check("the toolbar search box shows the scope, regex and filter controls", !!box?.querySelector('button[title="Search scope"]') && box.querySelector('button[aria-label="Regular expression"]')?.getAttribute("aria-pressed") === "true");
    const enter = (input: HTMLInputElement, value: string) => { input.value = value; input.dispatchEvent(new Event("input", { bubbles: true })); };
    box!.querySelector<HTMLButtonElement>('button[title="Filters"]')!.click();
    await tick();
    const form = () => box!.querySelector<HTMLFormElement>('form[aria-label="Search filters"]');
    enter(form()!.querySelector("input")!, ".LOG");
    await waitFor(() => found() === "report-2026.log", 5000, "results filtered from the popover");
    check("the Filters popover applies the extension while typing", source.filters.extension === "LOG");
    enter(form()!.querySelector<HTMLInputElement>('input[aria-label="Minimum size"]')!, "5");
    enter(form()!.querySelector<HTMLInputElement>('input[aria-label="Maximum size"]')!, "1");
    await tick();
    check("a reversed size range waits with a message", source.filters.minBytes === 5 * 1024 ** 2 && source.filters.maxBytes === null && /larger than/.test(form()?.querySelector('[role="alert"]')?.textContent ?? ""));
    box!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    check("Escape closes the popover first", !form() && source.searching);
    box!.querySelector<HTMLButtonElement>('button[title="Search scope"]')!.click();
    const ntfs = () => [...box!.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')].find((item) => item.textContent?.includes("NTFS"));
    await waitFor(() => !!ntfs(), 5000, "the NTFS scope of the fixture drive");
    ntfs()!.click();
    await tick();
    const ask = document.querySelector<HTMLDialogElement>('dialog[aria-label^="Search all of"]');
    check("NTFS search asks in the window before Windows does", !!ask?.open && !source.volume);
    [...ask!.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "Cancel")!.click();
    await tick();
    check("declining keeps folder search", !document.querySelector('dialog[aria-label^="Search all of"]') && !source.volume && Object.keys(volumes.states).length === 0);
    source.setRegex(false);
    source.setFilters(NO_FILTERS);
    await source.search("");
    check("clearing the search returns to the listing", source.rows === source.count && !!source.entry(0));
  }

  /** M4.3-g: core progress events, the pill's pause, the speed popover and cancel. */
  async function transferSteps(source: Pane, target: Pane, check: Check) {
    check("the last transfer or undo keeps its result for the popover", transfer?.phase === "finished" && transfer.succeeded === true && !!transfer.message, JSON.stringify(transfer));
    const big = target.pages[0]?.find((entry) => entry.name === "big.bin");
    const into = source.pages[0]?.find((entry) => entry.name === "z-folder");
    check("the 512 MiB fixture is listed", !!big?.id && !!into?.id);
    const done = nextDone();
    await start(api.transfer([big!.id!], into!.id!, false));
    await api.pauseTransfer();
    await waitFor(() => transfer?.phase === "paused" && transfer.samples.length > 0, 5000, "a paused transfer event");
    check("progress events report the planned size", transfer!.totalBytes === 512 * 1024 ** 2 && transfer!.totalItems === 1 && transfer!.bytes < transfer!.totalBytes, JSON.stringify({ ...transfer, samples: transfer?.samples.length }));
    await tick();
    const pill = () => document.querySelector<HTMLButtonElement>('.status button[title="Transfer details"]');
    check("the status bar shows a Transfers button with Resume", !!pill() && pill()!.textContent!.includes("Paused") && !!document.querySelector('.status button[aria-label="Resume transfer"]'));
    pill()!.click();
    await tick();
    const popover = document.querySelector('.status [role="dialog"][aria-label="Transfer details"]');
    check("the popover shows the speed graph and totals", !!popover?.querySelector("svg") && /of 512 MB/.test(popover.textContent ?? "") && /Resume/.test(popover.textContent ?? ""), popover?.textContent ?? "");
    [...popover!.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "Cancel")!.click();
    const result = await done;
    check("Cancel stops a paused transfer", !result.succeeded && /cancel/i.test(result.message), result.message);
    await waitFor(() => transfer?.phase === "finished", 5000, "the cancelled result");
    check("the popover keeps the cancelled result", transfer!.succeeded === false && !!document.querySelector('.status [role="dialog"] .result.failed'));
    pill()!.click();
    document.querySelector<HTMLButtonElement>('.status button[aria-label="Dismiss transfer result"]')!.click();
    await tick();
    check("the result can be dismissed", !pill() && transfer === null);
    // M4.4: pause, then Resume from the popover until the verified copy lands in `scratch`.
    await target.up();
    const scratch = target.pages[0]?.find((entry) => entry.name === "scratch");
    await target.back();
    const resumed = nextDone();
    await start(api.transfer([big!.id!], scratch!.id!, false));
    await api.pauseTransfer();
    await waitFor(() => transfer?.phase === "paused", 5000, "the second pause");
    const pausedAt = transfer!.bytes;
    await tick();
    pill()!.click();
    await tick();
    const again = document.querySelector('.status [role="dialog"][aria-label="Transfer details"]')!;
    [...again.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "Resume")!.click();
    const finished = await resumed;
    check("Resume finishes a paused copy and verifies it", finished.succeeded && /verified/i.test(finished.message) && pausedAt < 512 * 1024 ** 2, `${finished.message} (paused at ${pausedAt} bytes)`);
    await waitFor(() => transfer?.phase === "finished", 5000, "the finished copy");
    document.querySelector<HTMLButtonElement>('.status button[aria-label="Dismiss transfer result"]')?.click();
  }

  /** M4.4: an inaccessible folder, then rename, recycle and copy with undo in a long Unicode path. */
  async function acceptanceSteps(pane: Pane, root: string, check: Check) {
    const from = pane.folder!.path;
    await pane.open(`${root}\\locked`);
    await painted();
    check("an inaccessible folder explains itself and keeps the open folder", pane.folder?.path === from && /denied/i.test(pane.error) && document.querySelector(".status .error")?.textContent === pane.error, pane.error);
    let deep = `${root}\\Ünïcode 火 \u{1F600}`;
    for (let n = 1; n <= 8; n++) deep += `\\uzun-klasör-adı-${String(n).padStart(2, "0")}-${"x".repeat(20)}`;
    const file = "ünïcöde-火.txt";
    const renamed = "yeniden-adlandırıldı-火.txt";
    await pane.open(deep);
    check("a path over 260 characters with Unicode names opens", deep.length > 260 && samePath(pane.folder?.path ?? "", deep) && names(pane).join() === file && !pane.error, `${pane.folder?.path.length} · ${names(pane).join()} · ${pane.error}`);
    const step = async (action: () => void) => {
      const done = nextDone();
      action();
      const result = await done;
      await pane.refresh();
      return result;
    };
    await pane.select(0);
    let result = await step(() => rename(pane.focusedEntry!, renamed));
    check("rename inside the long Unicode path", result.succeeded && names(pane).join() === renamed, result.message);
    result = await step(undo);
    check("undo the rename there", result.succeeded && names(pane).join() === file, result.message);
    await pane.select(0);
    recycle();
    await painted();
    result = await step(() => recycleButton("Move to Recycle Bin").click());
    check("recycle inside the long Unicode path", result.succeeded && pane.rows === 0, result.message);
    result = await step(undo);
    check("undo restores the exact item into the long path", result.succeeded && names(pane).join() === file, result.message);
    const item = pane.entry(0)!.id!;
    await pane.up();
    result = await step(() => void start(api.transfer([item], pane.folder!.item!, false)));
    check("copy out of the long Unicode path", result.succeeded && names(pane).includes(file), result.message);
    result = await step(undo);
    check("undo removes that copy", result.succeeded && !names(pane).includes(file), result.message);
    await pane.open(from);
  }

  /** M4.3-g: grid view through its shortcuts, 2D keys, large Shell icons and Ctrl+wheel. */
  async function gridSteps(source: Pane, check: Check) {
    const key = (code: string) => window.dispatchEvent(new KeyboardEvent("keydown", { key: code.at(-1)!, code, ctrlKey: true, shiftKey: true, cancelable: true }));
    const list = () => document.querySelector<HTMLElement>(`#pane-${activeIndex}-row-0`)?.closest<HTMLElement>(".list");
    key("Digit3");
    await waitFor(() => gridView && tile === 128 && !!list()?.classList.contains("tiles"), 5000, "Ctrl+Shift+3");
    const table = () => list()!.querySelector<HTMLElement>('[role="grid"]')!;
    const columns = Number(table().getAttribute("aria-colcount"));
    const tiles = () => [...list()!.querySelectorAll<HTMLElement>(".tile")];
    check("Ctrl+Shift+3 shows medium tiles in columns from the width", tiles().length === source.rows && columns >= 2 && tiles()[0].offsetWidth === 128, `${tiles().length} tiles, ${columns} columns`);
    await waitFor(() => tiles().some((item) => { const image = item.querySelector("img"); return !!image?.complete && image.naturalWidth >= 64; }), 5000, "a 64 px Shell icon");
    check("tiles request large Shell icons", true);
    await source.select(0);
    table().focus();
    const arrow = (key: string) => table().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    arrow("ArrowRight");
    await waitFor(() => source.selected === 1, 2000, "ArrowRight");
    arrow("ArrowDown");
    await waitFor(() => source.selected === source.rows - 1, 2000, "ArrowDown");
    const current = document.getElementById(`pane-${activeIndex}-row-${source.selected}`);
    check("arrow keys move across and down the tiles", current?.getAttribute("aria-selected") === "true" && current.getAttribute("aria-colindex") === String((source.selected % columns) + 1) && table().getAttribute("aria-activedescendant") === current.id);
    key("Digit2");
    await waitFor(() => tile === 160 && tiles()[0]?.offsetWidth === 160, 5000, "Ctrl+Shift+2");
    const wheel = new WheelEvent("wheel", { deltaY: 120, ctrlKey: true, bubbles: true, cancelable: true });
    tiles()[0].dispatchEvent(wheel);
    await waitFor(() => tile === 128, 5000, "Ctrl+wheel");
    check("Ctrl+wheel resizes tiles instead of zooming the page", wheel.defaultPrevented);
    check("the view persists through the core", (await api.getSettings()).grid && (await api.getSettings()).tile === 128);
    key("Digit6");
    await waitFor(() => !gridView && !list()?.classList.contains("tiles"), 5000, "Ctrl+Shift+6");
    check("Ctrl+Shift+6 returns to details", !(await api.getSettings()).grid);
  }

  /** M4.3-g: in-window drag and drop with synthetic pointer events over the real layout. */
  async function dragSteps(source: Pane, target: Pane, check: Check) {
    const center = (element: Element) => { const box = element.getBoundingClientRect(); return { x: box.left + box.width / 2, y: box.top + box.height / 2 }; };
    const rowOf = (pane: Pane, name: string) => document.querySelector<HTMLElement>(`[data-pane="${pane.id}"] [data-row="${names(pane).indexOf(name)}"]`)!;
    const listOf = (pane: Pane) => document.querySelector<HTMLElement>(`[data-pane="${pane.id}"] [role="grid"]`)!;
    const pointer = (element: Element, type: string, at: { x: number; y: number }, shiftKey = false) =>
      element.dispatchEvent(new PointerEvent(type, { clientX: at.x, clientY: at.y, buttons: type === "pointerup" ? 0 : 1, pointerId: 1, isPrimary: true, shiftKey, bubbles: true, cancelable: true }));
    const ghost = () => document.querySelector(".drag-ghost")?.textContent ?? "";
    const grab = async (pane: Pane, name: string, over: Element, shiftKey = false) => {
      const row = rowOf(pane, name);
      const from = center(row.querySelector(".name .nm") ?? row);
      (row.querySelector(".name .nm") ?? row).dispatchEvent(new MouseEvent("mousedown", { clientX: from.x, clientY: from.y, button: 0, bubbles: true }));
      await waitFor(() => pane.focusedEntry?.name === name, 2000, `select ${name}`);
      pointer(row, "pointermove", { x: from.x + 20, y: from.y });
      pointer(row, "pointermove", center(over), shiftKey);
      await tick();
    };
    const escape = async () => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })); await tick(); };
    const drop = async (over: Element, shiftKey = false) => {
      const done = nextDone();
      pointer(over, "pointerup", center(over), shiftKey);
      const result = await done;
      await Promise.all([source.refresh(), target.refresh()]);
      return result;
    };
    await grab(source, "file2.txt", listOf(target));
    check("dragging over the other pane offers a copy", /Copy to dest/.test(ghost()) && !!listOf(target).closest(".list")?.classList.contains("drop-target"), ghost());
    await escape();
    check("Escape cancels a drag", !document.querySelector(".drag-ghost") && !document.querySelector(".drop-target"));
    await grab(source, "file2.txt", listOf(target), true);
    check("Shift turns the drag into a move", /Move to dest/.test(ghost()), ghost());
    let result = await drop(listOf(target), true);
    check("a Shift drop moves into the other pane", result.succeeded && names(target).includes("file2.txt") && !names(source).includes("file2.txt"), result.message);
    const undone = nextDone();
    undo();
    result = await undone;
    await Promise.all([source.refresh(), target.refresh()]);
    check("undo returns the dragged item", result.succeeded && names(source).includes("file2.txt") && !names(target).includes("file2.txt"), result.message);
    const tabOf = document.querySelector<HTMLElement>(`[data-tab="${target.id}"]`)!;
    await grab(source, "file2.txt", tabOf);
    check("a folder tab is a drop target", /Copy to dest/.test(ghost()) && tabOf.classList.contains("drop-target"), ghost());
    await escape();
    await grab(source, "file10.txt", rowOf(source, "note.txt"));
    check("the item's own folder refuses the drop", /Already in this folder/.test(ghost()) && !document.querySelector(".drop-target"), ghost());
    await escape();
    await grab(source, "z-folder", rowOf(source, "z-folder"));
    check("a folder refuses a drop into itself", /cannot go into itself/.test(ghost()), ghost());
    await escape();
    await grab(source, "file10.txt", rowOf(source, "z-folder"));
    check("a folder row is a drop target", /Copy to z-folder/.test(ghost()) && rowOf(source, "z-folder").classList.contains("drop-target"), ghost());
    result = await drop(rowOf(source, "z-folder"));
    check("dropping on a folder row copies into it", result.succeeded && names(source).includes("file10.txt"), result.message);
    const removed = nextDone();
    undo();
    check("undo removes the dropped copy", (await removed).succeeded);
    await source.refresh();
  }

  const recycleSheet = () => document.querySelector<HTMLDialogElement>('dialog[aria-label*="Recycle Bin"]');
  const recycleButton = (text: string) => [...(recycleSheet()?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((item) => item.textContent?.trim() === text)!;

  /** M4.3-h: Ctrl+A, a multi-item recycle through Delete and the sheet, and one undo for all. */
  async function recycleBatchSteps(source: Pane, check: Check) {
    activeIndex = slots.findIndex((slot) => slot.active?.kind === "folder" && slot.active.pane === source);
    lists[activeIndex]?.focus();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "a", ctrlKey: true, cancelable: true }));
    await waitFor(() => source.rows > 0 && source.selection.size === source.rows, 5000, "Ctrl+A");
    check("Ctrl+A selects every item", source.selection.size === 4, names(source).join());
    await source.select(names(source).indexOf("z-folder"), "toggle");
    const files = source.selectedEntries().map((entry) => entry.name).sort().join();
    check("Ctrl+click leaves three files selected", files === "file10.txt,file2.txt,note.txt", files);
    const grid = document.querySelector<HTMLElement>(`[data-pane="${source.id}"] [role="grid"]`)!;
    grid.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
    await waitFor(() => !!recycleSheet(), 5000, "the Recycle Bin sheet");
    const listed = [...recycleSheet()!.querySelectorAll(".recycle-list li")].map((item) => item.textContent).sort().join();
    check("Delete asks once for the whole selection", recycleSheet()!.getAttribute("aria-label") === "Move 3 items to the Recycle Bin?" && listed === files, listed);
    let done = nextDone();
    recycleButton("Move to Recycle Bin").click();
    let result = await done;
    await source.refresh();
    check("the core recycles every selected item", result.succeeded && result.message === "Moved 3 items to Recycle Bin" && names(source).join() === "z-folder", `${result.message} / ${names(source).join()}`);
    done = nextDone();
    undo();
    result = await done;
    await source.refresh();
    check("one undo restores the whole batch", result.succeeded && result.message === "Restored 3 items from Recycle Bin" && names(source).length === 4, `${result.message} / ${names(source).join()}`);
  }

  /** M4.3-h: context-menu icon row, keys and row height (the menu is open on a file). */
  async function menuKeySteps(check: Check) {
    const quick = () => [...document.querySelectorAll<HTMLButtonElement>(".menu .quick .qi")];
    const focusedQuick = () => quick().includes(document.activeElement as HTMLButtonElement);
    check("the menu opens with focus on its first enabled item", focusedQuick() && document.activeElement?.getAttribute("aria-label") === "Cut", document.activeElement?.outerHTML.slice(0, 80));
    const press = (key: string) => menuElement!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    press("ArrowRight");
    await tick();
    check("Right moves along the icon row", focusedQuick() && document.activeElement?.getAttribute("aria-label") !== "Cut");
    press("ArrowDown");
    await tick();
    const row = document.activeElement as HTMLElement;
    check("Down leaves the icon row for the 28 px rows", row.classList.contains("mi") && row.offsetHeight === 28, `${row.className} ${row.offsetHeight}`);
    press("ArrowDown");
    await tick();
    check("Down moves to the next row", document.activeElement !== row && !!(document.activeElement as HTMLElement).classList.contains("mi"));
    press("ArrowUp");
    press("ArrowUp");
    await tick();
    check("Up returns to the icon row", focusedQuick());
    const before = menuReturn;
    press("Escape");
    await tick();
    check("Escape closes the menu and restores focus", !menu && !document.querySelector(".menu") && (!before || document.activeElement === before));
  }

  /** M4.3-h: breadcrumb collapsing and chevrons, status bar and sidebar sizes. */
  async function layoutSteps(pane: Pane, check: Check) {
    const nav = document.querySelector<HTMLElement>('.toolbar nav[aria-label="Folder path"]')!;
    const crumbs = nav.querySelector<HTMLElement>(".crumbs")!;
    const visible = () => nav.querySelectorAll(".crumb:not(.more)").length;
    const wide = visible();
    nav.style.flex = "0 0 240px";
    await waitFor(() => !!nav.querySelector('button[aria-label="Hidden folders"]'), 5000, "collapsed segments");
    await painted();
    const segments = pane.folder!.segments;
    const current = nav.querySelector<HTMLElement>(".crumb.cur")!;
    check("leading segments collapse into … when the path is narrow", crumbs.scrollWidth <= crumbs.clientWidth + 1 && current.textContent === segments.at(-1)!.name && (visible() === 1 || current.scrollWidth <= current.clientWidth + 1) && visible() < segments.length, `${visible()} of ${segments.length}`);
    nav.querySelector<HTMLButtonElement>('button[aria-label="Hidden folders"]')!.click();
    await waitFor(() => !!document.querySelector('.crumb-menu[aria-label="Hidden folders"]'), 5000, "the hidden folders list");
    const hiddenNames = [...document.querySelectorAll('.crumb-menu .mi .label')].map((label) => label.textContent);
    check("… lists the collapsed folders from the drive down", hiddenNames.length === segments.length - visible() && hiddenNames[0] === segments[0].name, hiddenNames.join(" › "));
    document.querySelector(".crumb-menu")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    nav.style.flex = "";
    await waitFor(() => visible() === wide, 5000, "the restored breadcrumb");
    check("a wider address shows the segments again", !document.querySelector(".crumb-menu"));
    const chevrons = nav.querySelectorAll<HTMLButtonElement>(".chev");
    chevrons[chevrons.length - 1].click();
    await waitFor(() => !!document.querySelector(".crumb-menu .mi"), 5000, "sibling folders");
    const siblings = [...document.querySelectorAll<HTMLButtonElement>(".crumb-menu .mi")];
    const marked = siblings.find((item) => item.classList.contains("on"));
    check("a chevron lists the sibling folders with the current one marked", siblings.some((item) => item.textContent === "dest") && marked?.textContent === segments.at(-1)!.name && document.activeElement === marked, siblings.map((item) => item.textContent).join());
    const from = pane.folder!.path;
    siblings.find((item) => item.textContent === "dest")!.click();
    await waitFor(() => pane.folder?.path.endsWith("\\dest") === true, 5000, "the chosen sibling");
    check("choosing a sibling opens it", !document.querySelector(".crumb-menu"));
    await pane.back();
    await waitFor(() => pane.folder?.path === from, 5000, "back to the source");

    const status = document.querySelector<HTMLElement>(".status")!;
    await waitFor(() => /free$/.test(status.querySelector(".space")?.textContent ?? ""), 5000, "the free space");
    check("the 32 px status bar shows the drive's free space", status.offsetHeight === 32, `${status.offsetHeight} px · ${status.querySelector(".space")?.textContent}`);
    const side = document.querySelector<HTMLElement>(".sidebar")!;
    check("the sidebar starts 232 px wide", sidebar && side.offsetWidth === 232, `${side.offsetWidth}`);
    const handle = document.querySelector<HTMLElement>('[role="separator"][aria-label="Sidebar width"]')!;
    handle.focus();
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, bubbles: true, cancelable: true }));
    await waitFor(() => settings?.sidebar_width === 264 && side.offsetWidth === 264, 5000, "a wider sidebar");
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true, cancelable: true }));
    await waitFor(() => side.offsetWidth === 180, 5000, "the narrowest sidebar");
    check("the sidebar resizes within 180–360 px and persists", (await api.getSettings()).sidebar_width === 180);
    handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await waitFor(() => side.offsetWidth === 232 && settings?.sidebar_width === 232, 5000, "the default width");
    check("double-clicking the edge restores 232 px", true);
  }

  /** The design-review corrections: mouse buttons, zoom, drag selection, sidebar and Windows menus. */
  async function designSteps(pane: Pane, root: string, check: Check) {
    const from = pane.folder!.path;
    const manyPath = `${from}\\z-folder\\many`;
    const listEl = () => document.querySelector<HTMLElement>(`[data-pane="${pane.id}"] [role="grid"]`)!;
    const rowEl = (row: number) => document.querySelector<HTMLElement>(`[data-pane="${pane.id}"] [data-row="${row}"]`)!;
    const middle = (element: Element) => { const box = element.getBoundingClientRect(); return { x: box.left + box.width / 2, y: box.top + box.height / 2 }; };
    const side = (button: number) => {
      const at = middle(listEl());
      const init = { button, clientX: at.x, clientY: at.y, bubbles: true, cancelable: true };
      listEl().dispatchEvent(new MouseEvent("mousedown", init));
      const up = new MouseEvent("mouseup", init);
      listEl().dispatchEvent(up);
      return up;
    };
    await pane.open(`${from}\\z-folder`);
    let event = side(3);
    await waitFor(() => pane.folder?.path === from, 5000, "mouse button 3");
    check("mouse button 3 goes back without WebView2 history", event.defaultPrevented);
    event = side(4);
    await waitFor(() => pane.folder?.path === `${from}\\z-folder`, 5000, "mouse button 4");
    check("mouse button 4 goes forward", event.defaultPrevented);
    await pane.back();
    await waitFor(() => pane.folder?.path === from, 5000, "the source again");

    const wheel = (target: Element, deltaY: number) => {
      const scroll = new WheelEvent("wheel", { deltaY, ctrlKey: true, bubbles: true, cancelable: true });
      target.dispatchEvent(scroll);
      return scroll;
    };
    await painted();
    let scrolled = wheel(rowEl(0), -120);
    await waitFor(() => settings?.row === 40 && rowEl(0)?.offsetHeight === 40, 5000, "Ctrl+wheel in details");
    check("Ctrl+wheel in details makes rows taller and never zooms", scrolled.defaultPrevented && !!rowEl(0).querySelector('[width="24"], .file-icon[style*="24px"]'), rowEl(0).outerHTML.slice(0, 200));
    scrolled = wheel(rowEl(0), 120);
    await waitFor(() => settings?.row === 32 && rowEl(0)?.offsetHeight === 32, 5000, "Ctrl+wheel back");
    check("Ctrl+wheel returns to 32 px rows", (await api.getSettings()).row === 32);
    const before = settings!.row;
    scrolled = wheel(document.querySelector(".sidebar")!, -120);
    await sleep(50);
    check("Ctrl+wheel outside the list only blocks page zoom", scrolled.defaultPrevented && settings!.row === before);

    const down = (target: Element, at: { x: number; y: number }, ctrlKey = false) =>
      target.dispatchEvent(new MouseEvent("mousedown", { clientX: at.x, clientY: at.y, button: 0, ctrlKey, bubbles: true, cancelable: true }));
    const pointer = (type: string, at: { x: number; y: number }) =>
      window.dispatchEvent(new PointerEvent(type, { clientX: at.x, clientY: at.y, buttons: type === "pointerup" ? 0 : 1, pointerId: 1, isPrimary: true, bubbles: true, cancelable: true }));
    const shown = (row: number, what: string) => waitFor(() => pane.complete && !!rowEl(row)?.querySelector(".nm"), 5000, what);
    await pane.open(manyPath);
    await shown(7, "the rows of many");
    await painted();
    const cell = (row: number) => middle(rowEl(row).querySelector(".dim")!);
    down(rowEl(2).querySelector(".dim")!, cell(2));
    pointer("pointermove", cell(5));
    await waitFor(() => pane.selection.size === 4, 5000, "the selection box");
    check("the box draws while dragging", !!document.querySelector(".marquee"));
    pointer("pointerup", cell(5));
    await tick();
    check("dragging from a row's date selects the rows it crosses", !document.querySelector(".marquee") && [2, 3, 4, 5].every((row) => pane.isSelected(row)), `${pane.selection.size}`);
    // Row 6 is not selected, so the press starts a box and not an item drag.
    down(rowEl(6).querySelector(".dim")!, cell(6), true);
    pointer("pointermove", cell(4));
    await waitFor(() => pane.selection.size === 3 && pane.isSelected(6) && !pane.isSelected(4), 5000, "Ctrl and the box");
    pointer("pointerup", cell(4));
    check("Ctrl flips the rows the box crosses", pane.isSelected(2) && pane.isSelected(3) && !pane.isSelected(5));
    down(rowEl(1).querySelector(".dim")!, cell(1));
    pointer("pointerup", cell(1));
    await waitFor(() => pane.selection.size === 1 && pane.isSelected(1), 5000, "a click on a row's date");
    check("a click outside the name still selects that row", true);
    await pane.open(from);
    await shown(0, "the rows of the source");
    await painted();
    const bounds = listEl().getBoundingClientRect();
    const empty = { x: bounds.left + bounds.width / 2, y: bounds.bottom - 24 };
    down(listEl(), empty);
    pointer("pointermove", middle(rowEl(0)));
    await waitFor(() => pane.selection.size === pane.rows, 5000, "the box from empty space");
    pointer("pointerup", middle(rowEl(0)));
    check("the box from empty space selects upward", pane.rows === 4);
    down(listEl(), empty);
    pointer("pointerup", empty);
    await tick();
    check("a click on empty space clears the selection", pane.selection.size === 0);
    await pane.select(0);
    setView(true, 128);
    await waitFor(() => !!rowEl(1)?.querySelector(".nm") && !!document.querySelector(`[data-pane="${pane.id}"] .tile`), 5000, "tiles");
    await painted();
    const tileBox = rowEl(0).getBoundingClientRect();
    const gap = { x: tileBox.left - 4, y: tileBox.top + 4 };
    down(rowEl(0).parentElement!.parentElement!, gap);
    pointer("pointermove", middle(rowEl(1)));
    await waitFor(() => pane.isSelected(0) && pane.isSelected(1) && pane.selection.size === 2, 5000, "the box over tiles");
    pointer("pointerup", middle(rowEl(1)));
    check("the box selects the tiles it crosses", true);
    setView(false);
    await waitFor(() => !document.querySelector(`[data-pane="${pane.id}"] .tile`), 5000, "details again");
    await painted();
    await pane.select(0);

    const seeded = (await api.session()).pins.length;
    const rows = () => [...document.querySelectorAll<HTMLElement>(".sidebar .pin")];
    await waitFor(() => rows().some((item) => /\(C:\)$/.test(item.textContent?.trim() ?? "")), 5000, "the drive list");
    check("the Windows folders are pinned and This PC lists the drives", seeded >= 6 && !!settings && document.querySelector(".side-toggle")?.textContent?.includes("This PC") === true, `${seeded} pins`);
    const labels = () => [...document.querySelectorAll(".menu .mi .label")].map((label) => label.textContent?.trim()).join("|");
    const pinRow = rows()[0];
    const open = new MouseEvent("contextmenu", { clientX: 40, clientY: 40, bubbles: true, cancelable: true });
    pinRow.dispatchEvent(open);
    await painted();
    check("right-clicking a pin opens Firewol's menu, not the browser's", open.defaultPrevented && menu?.kind === "pin" && labels().includes("Open in new tab") && labels().includes("Customize folder…") && labels().includes("Change icon…") && labels().includes("Unpin from sidebar") && labels().includes("Copy path"), labels());
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    pinRow.focus();
    pinRow.dispatchEvent(new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true, cancelable: true }));
    await painted();
    check("Shift+F10 opens the pin menu", menu?.kind === "pin");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    const driveRow = rows().find((item) => item.dataset.pin === "C:\\")!;
    driveRow.dispatchEvent(new MouseEvent("contextmenu", { clientX: 40, clientY: 40, bubbles: true, cancelable: true }));
    await painted();
    check("a drive has its own menu", menu?.kind === "drive" && labels().includes("Pin to sidebar") && !labels().includes("Change icon…"), labels());
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    const pinPaths = () => session.pins.map((pin) => pin.path);
    const pinAt = (path: string) => document.querySelector<HTMLElement>(`.sidebar .pin[data-pinned][data-pin="${CSS.escape(path)}"]`)!;
    const edge = (path: string) => { const box = pinAt(path).getBoundingClientRect(); return { x: box.left + 20, y: box.top + 2 }; };
    const pressPin = (path: string) => {
      const box = pinAt(path).getBoundingClientRect();
      pinAt(path).dispatchEvent(new PointerEvent("pointerdown", { clientX: box.left + 20, clientY: box.top + box.height / 2, button: 0, pointerId: 1, isPrimary: true, bubbles: true, cancelable: true }));
    };
    const [first, second, third, fourth] = pinPaths();
    const order = pinPaths().join();
    pressPin(first);
    pointer("pointermove", edge(fourth));
    await tick();
    check("dragging a pin shows where it will land", pinAt(first).classList.contains("moving") && pinAt(fourth).classList.contains("line-before"));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    pointer("pointerup", edge(fourth));
    await tick();
    check("Escape cancels a pin drag", pinPaths().join() === order && !document.querySelector(".pin.moving, .pin.line-before"));
    pressPin(first);
    pointer("pointermove", edge(fourth));
    pointer("pointerup", edge(fourth));
    await waitFor(() => pinPaths()[2] === first, 5000, "the moved pin");
    check("dropping a pin reorders the sidebar", pinPaths().slice(0, 4).join() === [second, third, first, fourth].join());
    check("the new order persists through the core", (await api.session()).pins[2].path === first);
    check("a pin drag does not open the folder", pane.folder?.path === from);
    pressPin(first);
    pointer("pointermove", edge(second));
    pointer("pointerup", edge(second));
    await waitFor(() => pinPaths().join() === order, 5000, "the original pin order");
    await tick();
    const stray = new MouseEvent("contextmenu", { clientX: 4, clientY: 4, bubbles: true, cancelable: true });
    document.querySelector(".status")!.dispatchEvent(stray);
    check("no part of the window shows the browser's menu", stray.defaultPrevented);

    await pane.select((pane.pages[0] ?? []).findIndex((entry) => entry.name === "note.txt"));
    showMenu("item", 40, 40);
    await waitFor(() => Array.isArray(shellMenu), SHELL_WAIT_MS + 500, "the Windows commands");
    await painted();
    check("Windows commands join the item menu with Show more options", (shellMenu as ShellMenuItem[]).some((item) => !item.separator) && labels().endsWith("Show more options"), labels());
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    await saveSettings({ shell_items: false });
    showMenu("area", 40, 40);
    await painted();
    check("the setting hides the Windows commands", shellMenu === null && !labels().includes("Show more options"), labels());
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await tick();
    await saveSettings({ shell_items: true });
    void root;
  }

  /** Files run their Windows default action; a shortcut to a folder stays inside Firewol. */
  async function openSteps(pane: Pane, root: string, check: Check) {
    const from = pane.folder!.path;
    await pane.open(`${root}\\launch`);
    const entry = (name: string) => (pane.pages[0] ?? []).find((item) => item.name === name);
    await pane.select((pane.pages[0] ?? []).indexOf(entry("open-me.cmd")!));
    const grid = document.querySelector<HTMLElement>(`[data-pane="${pane.id}"] [role="grid"]`)!;
    grid.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    for (let attempt = 0; attempt < 100 && !entry("opened.txt"); attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      await pane.refresh();
    }
    check("Enter runs a file's Windows default action", !!entry("opened.txt"), names(pane).join());
    open(entry("to-dest.lnk")!, false);
    await waitFor(() => pane.folder?.path === `${root}\\dest`, 5000, "the shortcut's folder");
    check("a shortcut to a folder opens inside Firewol", true);
    await pane.open(from);
  }

  async function selfTest(info: Startup) {
    const steps: { step: string; ok: boolean; detail?: string }[] = [];
    const check = (step: string, ok: boolean, detail?: string) => {
      steps.push({ step, ok, detail });
      if (!ok) throw new Error(`${step}: ${detail ?? "failed"}`);
    };
    const pick = (pane: Pane, name: string) => {
      const row = names(pane).indexOf(name);
      check(`find ${name}`, row >= 0, names(pane).join(", "));
      pane.select(row);
    };
    const settle = async (pane: Pane, done: Promise<OperationDone>) => {
      const result = await done;
      await pane.refresh();
      session = await api.session();
      return result;
    };
    const source = panes[0];
    try {
      const sourcePath = info.path;
      const destPath = `${source.folder?.segments.at(-2)?.path}\\dest`;
      pick(source, "note.txt");
      await waitFor(() => {
        const icon = document.querySelector<HTMLImageElement>('img[src^="http://icon.localhost/"]');
        return !!icon?.complete && icon.naturalWidth >= 16;
      }, 5000, "Shell icon");
      check("Shell file icon loaded through CSP", true);
      await clip(true);
      check("cut", session.clipboard?.cut === true && session.clipboard.count === 1);
      await source.open(destPath);
      let done = nextDone();
      paste();
      let result = await settle(source, done);
      check("paste moves the file", result.succeeded && names(source).includes("note.txt"), result.message);
      check("a finished move empties the clipboard", session.clipboard === null);
      pick(source, "note.txt");
      done = nextDone();
      rename(source.entry(source.selected)!, "renamed.txt");
      result = await settle(source, done);
      check("rename", result.succeeded && names(source).includes("renamed.txt"), result.message);
      done = nextDone();
      undo();
      result = await settle(source, done);
      check("undo rename", result.succeeded && names(source).includes("note.txt"), result.message);
      done = nextDone();
      undo();
      result = await settle(source, done);
      check("undo move", result.succeeded && !names(source).includes("note.txt"), result.message);
      check("back is available", source.canBack);
      await source.back();
      check("back returns to the source", source.folder?.path === sourcePath && names(source).includes("note.txt"));
      check("forward is available", source.canForward);
      await toggleSplit();
      const target = panes[1];
      await target.open(destPath);
      activeIndex = 0;
      pick(source, "note.txt");
      done = nextDone();
      toOtherPane(false);
      result = await settle(target, done);
      check("copy to the other pane", result.succeeded && names(target).includes("note.txt"), result.message);
      activeIndex = 1;
      pick(target, "note.txt");
      recycle();
      await painted();
      check("recycling asks in the window first", !!recycleSheet()?.open && recycleSheet()!.getAttribute("aria-label") === "Move “note.txt” to the Recycle Bin?" && document.activeElement === recycleButton("Move to Recycle Bin"), recycleSheet()?.getAttribute("aria-label") ?? "no sheet");
      recycleButton("Cancel").click();
      await painted();
      await target.refresh();
      check("Cancel keeps the item", !recycleSheet() && names(target).includes("note.txt"));
      pick(target, "note.txt");
      recycle();
      await painted();
      done = nextDone();
      recycleButton("Move to Recycle Bin").click();
      result = await settle(target, done);
      check("move to the Recycle Bin", result.succeeded && !names(target).includes("note.txt"), result.message);
      done = nextDone();
      undo();
      result = await settle(target, done);
      check("undo recycle", result.succeeded && names(target).includes("note.txt"), result.message);
      await recycleBatchSteps(source, check);
      await transferSteps(source, target, check);
      await dragSteps(source, target, check);
      await togglePin(destPath);
      check("pin", session.pins.some((pin) => pin.path === destPath));
      await togglePin(destPath);
      check("unpin", !session.pins.some((pin) => pin.path === destPath));
      const original = await api.getSettings();
      await saveSettings({ sidebar: false, appearance: { ...original.appearance, accent: [18, 52, 86] } });
      await painted();
      check("settings apply through CSS tokens", getComputedStyle(document.querySelector(".fw")!).getPropertyValue("--accent").trim() === "#123456" && !sidebar);
      const persisted = await api.getSettings();
      check("settings persist through the core", !persisted.sidebar && persisted.appearance.accent?.join() === "18,52,86" && persisted.version === 2);
      await saveSettings({ sidebar: original.sidebar, appearance: original.appearance });
      await source.refresh();
      pick(source, "file2.txt");
      check("folders precede naturally sorted file names", names(source).join() === "z-folder,file2.txt,file10.txt,note.txt");
      const identity = source.selectedRefs()[0];
      const oldRow = source.selected;
      await source.sortBy("name");
      await source.refresh();
      check("sort and refresh preserve selected identity", source.selected !== oldRow && source.selectedRefs()[0]?.index === identity.index && source.selectedRefs()[0]?.path === identity.path);
      await searchSteps(source, check);
      await gridSteps(source, check);
      const slot = slots[0];
      const originalTab = slot.active;
      source.query = "file";
      const extra = await newTab(destPath, slot);
      check("inactive tabs release listing pages", !source.folder && Object.keys(source.pages).length === 0);
      check("a new tab owns its folder", extra?.kind === "folder" && extra.pane.folder?.path === destPath);
      await selectTab(slot, originalTab);
      check("switching back restores history and search draft", source.query === "file" && source.canForward && !!source.folder);
      await source.search("");
      if (extra) { slot.reorder(extra.id, originalTab.id); check("tabs reorder", slot.tabs[0].id === extra.id); await closeTab(slot, extra); }
      check("closing a tab preserves the active tab", slot.activeTab === originalTab.id && slot.tabs.length === 1);
      await toggleSplit();
      check("closing split view merges tabs", slots.length === 1 && slots[0].tabs.length === 2);
      // Shortcuts go through the same window handler a real key press reaches.
      const press = (key: string, init: KeyboardEventInit = {}) =>
        window.dispatchEvent(new KeyboardEvent("keydown", { key, ctrlKey: true, cancelable: true, ...init }));
      const tabs = () => slots[0].tabs;
      const shown = () => slots[0].active?.kind === "folder" && !!slots[0].active.pane.folder;
      const count = tabs().length;
      press("t");
      await waitFor(() => tabs().length === count + 1 && shown(), 5000, "Ctrl+T");
      const opened = slots[0].activeTab;
      check("Ctrl+T opens and lists a new tab", opened === tabs().at(-1)?.id);
      press("Tab");
      await waitFor(() => slots[0].activeTab === tabs()[0].id && shown(), 5000, "Ctrl+Tab");
      check("Ctrl+Tab wraps to the first tab", true);
      press("Tab", { shiftKey: true });
      await waitFor(() => slots[0].activeTab === opened && shown(), 5000, "Ctrl+Shift+Tab");
      check("Ctrl+Shift+Tab cycles back", true);
      press("w");
      await waitFor(() => tabs().length === count && !tabs().some((tab) => tab.id === opened) && shown(), 5000, "Ctrl+W");
      check("Ctrl+W closes the active tab", true);
      press(",");
      await waitFor(() => slots[0].active?.kind === "settings", 5000, "Ctrl+,");
      await tick();
      check("Ctrl+, opens Settings as a tab", !!document.querySelector(".settings") && !document.querySelector(".grid") && !document.querySelector(".toolbar"));
      const initialMenu = await settingsSelfTest(check);
      press("w");
      await waitFor(() => tabs().length === count && shown(), 5000, "closing Settings");
      check("closing Settings returns to a folder tab", true);
      const listing = active!;
      listing.select((listing.pages[0] ?? []).findIndex((entry) => !entry.directory));
      showMenu("item", 40, 40);
      await painted();
      const items = () => [...document.querySelectorAll<HTMLButtonElement>(".menu .mi")];
      const labels = items().map((item) => item.querySelector(".label")?.textContent?.trim()).join("|");
      const quick = [...document.querySelectorAll(".menu .quick .qi")].map((item) => item.getAttribute("aria-label")).join("|");
      check("the context menu follows the saved menu", labels.includes("Self-test action") && !quick.split("|").includes("Copy") && quick.includes("Cut") && !labels.split("|").includes("Cut") && labels.includes("Edit menu…") && !labels.includes("Customize folder"), `${quick} / ${labels}`);
      check("the icon row holds Cut, Paste, Rename and Recycle", quick === "Cut|Paste in current folder|Rename|Move to Recycle Bin" && !labels.includes("Open in new tab"), `${quick} / ${labels}`);
      await menuKeySteps(check);
      showMenu("item", 40, 40);
      await painted();
      items().find((item) => item.textContent?.includes("Edit menu…"))!.click();
      await waitFor(() => slots[0].active?.kind === "settings" && !!document.querySelector(".settings h2"), 5000, "Edit menu…");
      check("Edit menu… opens the Context menu section", document.querySelector(".settings h2")?.textContent === "Context menu");
      const settingsButton = (text: string) => [...document.querySelectorAll<HTMLButtonElement>(".settings button")].find((item) => item.textContent?.trim() === text);
      settingsButton("Reset to default…")?.click();
      await painted();
      settingsButton("Reset menu")?.click();
      await waitFor(() => JSON.stringify(settings!.menu) === initialMenu, 5000, "the menu reset");
      check("Reset restores the core default menu", true);
      press("w");
      await waitFor(() => tabs().length === count && shown(), 5000, "closing Settings again");
      await folderStyleSelfTest(check, active!, sourcePath);
      await layoutSteps(active!, check);
      await designSteps(active!, parentPath(sourcePath), check);
      await acceptanceSteps(active!, parentPath(sourcePath), check);
      await openSteps(active!, parentPath(sourcePath), check);
      const sidebarBefore = sidebar;
      press("b");
      await waitFor(() => sidebar !== sidebarBefore, 5000, "Ctrl+B");
      press("b");
      await waitFor(() => sidebar === sidebarBefore, 5000, "Ctrl+B again");
      check("Ctrl+B still toggles the sidebar", true);
      press("\\");
      await waitFor(() => slots.length === 2 && slots[1].active?.kind === "folder" && !!slots[1].active.pane.folder, 5000, "Ctrl+\\");
      press("\\");
      await waitFor(() => slots.length === 1 && tabs().length === count + 1, 5000, "Ctrl+\\ again");
      check("Ctrl+\\ still splits and merges", true);
      const detached = slots[0].active;
      check("a folder tab is active before detaching", detached?.kind === "folder");
      if (detached?.kind === "folder") {
        press("N", { shiftKey: true });
        await waitFor(() => !tabs().some((tab) => tab.id === detached.id) && shown(), 10000, "Ctrl+Shift+N");
        const refused = await api.navigate(detached.id, null, "reload", detached.pane.sort).then(() => false, () => true);
        check("Ctrl+Shift+N hands the tab to a new window", refused, "the old window can no longer drive the detached tab");
      }
    } catch (reason) {
      if (steps.every((step) => step.ok)) steps.push({ step: "error", ok: false, detail: String(reason) });
    }
    await api.saveReport({ steps, passed: steps.every((step) => step.ok) });
  }

  onMount(() => {
    const unlisten = [
      listen<FolderProgress>(FOLDER_PROGRESS, (event) => panes.forEach((pane) => pane.progress(event.payload))),
      listen<IndexState>(INDEX_STATE, (event) => panes.forEach((pane) => pane.indexed(event.payload))),
      listen<VolumeState>(VOLUME_STATE, (event) => volumeChanged(event.payload)),
      listen<TransferStatus>(TRANSFER_PROGRESS, (event) => (transfer = event.payload)),
      listen<OperationDone>(OPERATION_DONE, (event) => finished(event.payload)),
      listen<SessionState>(SESSION_CHANGED, (event) => (session = event.payload)),
      listen<SettingsView>(SETTINGS_CHANGED, (event) => (settings = withPending(event.payload))),
      listen<string>(ICON_CHANGED, (event) => windowsIconChanged(event.payload)),
      listen(DRIVES_CHANGED, () => refreshDrives(300)),
      appWindow.onFocusChanged((event) => { if (event.payload) refreshDrives(); }),
      appWindow.onCloseRequested(async (event) => {
        // Not prevented: the API then destroys the window, which needs `core:window:allow-destroy`.
        if (allowClose) return;
        event.preventDefault();
        // A failed save or query must never leave the window impossible to close.
        await flushAppearance().catch(() => {});
        if (await api.operationBusy().catch(() => false)) closeWarning = true;
        else { allowClose = true; await appWindow.close(); }
      }),
    ];
    for (const type of ["mousedown", "mouseup", "auxclick"] as const) window.addEventListener(type, sideButton, true);
    window.addEventListener("wheel", blockPageZoom, { capture: true, passive: false });
    window.addEventListener("contextmenu", blockBrowserMenu, true);
    void (async () => {
      const info = await api.startup();
      await Promise.all([loadSettings(), volumes.load(), api.transferStatus().then((status) => (transfer = status))]);
      const start = performance.now();
      useLocale(info.locale);
      startup = info;
      session = info.session;
      refreshDrives();
      if (info.adopt) {
        const tab = restore(info.adopt);
        slots = [new PaneSlot(tab)];
        if (tab.kind === "folder") await tab.pane.resume();
      } else {
        slots = (info.split ? [newPane(), newPane()] : [newPane()]).map(pane => new PaneSlot(folderTab(pane)));
        await Promise.all(panes.map((pane) => pane.open(info.path)));
      }
      await painted();
      const firstListMs = info.sinceStartMs + performance.now() - start;
      lists[0]?.focus();
      if (info.selfTest && info.label === "main") await selfTest(info);
      else if (info.selfTest) await api.saveReport({ label: info.label, ready: true, adopted: info.adopt?.kind ?? null, path: active?.folder?.path ?? null });
      // The self-test ends with a real click on each window's close button (see Test-Interface.ps1).
      if (info.selfTest) { await api.awaitSignal("close"); document.querySelector<HTMLButtonElement>(".cap.close")!.click(); }
      else if (info.report && !info.adopt) {
        await benchmark(info, firstListMs).catch((reason) =>
          api.saveReport({ label: info.label, error: String(reason) }),
        );
      }
    })().catch((reason) => (fatal = String(reason)));
    return () => {
      unlisten.forEach((promise) => void promise.then((stop) => stop()));
      for (const type of ["mousedown", "mouseup", "auxclick"] as const) window.removeEventListener(type, sideButton, true);
      window.removeEventListener("wheel", blockPageZoom, true);
      window.removeEventListener("contextmenu", blockBrowserMenu, true);
    };
  });
</script>

<svelte:window onkeydown={shortcut} onmousedown={() => (menu = undefined)} />

{#snippet shellRows(items: ShellMenuItem[], level: number)}
  {#each items as item, index (index)}
    {#if item.separator}
      <div class="sep"></div>
    {:else}
      <button class="mi shell" role="menuitem" aria-haspopup={item.children.length > 0 ? "menu" : undefined} disabled={item.disabled} onclick={(event) => runShell(item, level, event)} onmouseenter={(event) => shellHover(event.currentTarget, item, level)}>
        {#if item.icon}<FileIcon icon={item.icon} />{:else}<span class="shell-blank"></span>{/if}
        <span class="label">{item.label}</span>
        {#if item.children.length > 0}<Icon name="chevron" size={12} weight={2} />{/if}
      </button>
    {/if}
  {/each}
{/snippet}

{#snippet shellBlock()}
  {#if shellMenu === "loading"}
    <div class="sep"></div>
    <div class="mi shell-wait" aria-hidden="true"><span class="shell-blank"></span><span class="label muted">Loading Windows commands…</span></div>
  {:else if shellMenu}
    {#if shellMenu.some((item) => !item.separator)}
      <div class="sep"></div>
      {@render shellRows(shellMenu, 0)}
    {/if}
    <div class="sep"></div>
    <button class="mi" role="menuitem" onclick={showNativeMenu}><Icon name="more" /><span class="label">Show more options</span></button>
  {/if}
{/snippet}

<div class="fw p-{preset}" class:dark use:colors={settings?.appearance}>
  <div class="shell">
    <header class="titlebar" data-tauri-drag-region>
      {#if slots.length === 1}<TabBar slot={slots[0]} styleOf={tabStyle} onselect={tab => selectTab(slots[0], tab)} onclose={tab => closeTab(slots[0], tab)} onnew={() => newTab()} ondetach={tab => detachTab(slots[0], tab)} />{:else}<span class="app-title">Firewol</span>{/if}
      <div class="drag" data-tauri-drag-region></div>
      <div class="caption {settings?.appearance.caption_shape ?? 'rounded'}" class:left={settings?.appearance.caption_left}>
        <button class="cap" aria-label="Minimize" onclick={() => appWindow.minimize()}><Icon name="minimize" size={14} weight={1.5} /></button>
        <button class="cap" aria-label="Maximize" onclick={() => appWindow.toggleMaximize()}><Icon name="maximize" size={14} weight={1.5} /></button>
        <button class="cap close" aria-label="Close" onclick={() => appWindow.close()}><Icon name="close" size={14} weight={1.5} /></button>
      </div>
    </header>

    {#if !settingsOnly}
    <div class="toolbar">
      <button class="ib" class:on={sidebar} aria-pressed={sidebar} aria-label="Sidebar (Ctrl+B)" title="Sidebar (Ctrl+B)" onclick={toggleSidebar}><Icon name="sidebar" /></button>
      <button class="ib" aria-label="Back (Alt+Left)" title="Back (Alt+Left)" disabled={!active?.canBack} onclick={() => active?.back()}><Icon name="back" /></button>
      <button class="ib" aria-label="Forward (Alt+Right)" title="Forward (Alt+Right)" disabled={!active?.canForward} onclick={() => active?.forward()}><Icon name="forward" /></button>
      <button class="ib" aria-label="Up one level (Alt+Up)" title="Up one level (Alt+Up)" disabled={(active?.folder?.segments.length ?? 0) < 2} onclick={() => active?.up()}><Icon name="up" /></button>
      {#if editing}
        <form class="field address" onsubmit={submitAddress}>
          <label class="sr" for="address">Folder path</label>
          <input id="address" bind:this={addressField} bind:value={address} spellcheck="false" onblur={() => (editing = false)} />
        </form>
      {:else}
        <Breadcrumb
          segments={active?.folder?.segments ?? []}
          icon={active?.icon}
          onopen={(path) => void active?.open(path)}
          onedit={editAddress}
          onrefresh={() => void active?.refresh()}
          onerror={(message) => notify(message, { error: true })}
        />
      {/if}
      {#if active}
        {#key active}
          <SearchBox bind:this={searchBox} pane={active} label={title} onvolume={drive => (volumeAsk = { drive, pane: active! })} />
        {/key}
      {:else}
        <div class="search-gap"></div>
      {/if}
      <div class="seg" role="group" aria-label="View">
        <button class="ib" class:on={!gridView} aria-pressed={!gridView} aria-label="Details view (Ctrl+Shift+6)" title="Details view (Ctrl+Shift+6)" onclick={() => setView(false)}><Icon name="list" /></button>
        <button class="ib" class:on={gridView} aria-pressed={gridView} aria-label="Grid view (Ctrl+Shift+3)" title="Grid view (Ctrl+Shift+3, Ctrl+wheel resizes)" onclick={() => setView(true)}><Icon name="grid" /></button>
      </div>
      <button class="ib" class:on={slots.length > 1} aria-pressed={slots.length > 1} aria-label="Split view (Ctrl+\)" title="Split view (Ctrl+\)" onclick={toggleSplit}><Icon name="split" /></button>
      <button class="ib" aria-label="More" title="More" aria-haspopup="menu" onmousedown={(event) => event.stopPropagation()} onclick={(event) => showMenu("more", event.clientX - 220, event.clientY + 16)}><Icon name="more" /></button>
    </div>
    {/if}

    <div class="body" class:settings-only={settingsOnly}>
      {#if sidebar && !settingsOnly}
        <nav class="sidebar" aria-label="Pinned folders" style:flex-basis={`${sidebarWidth}px`}>
          <div class="side-head">
            <span class="side-title">Pinned</span>
            <button
              class="ib sm"
              class:on={pinned}
              aria-pressed={pinned}
              aria-label={pinned ? "Unpin current folder" : "Pin current folder"}
              title={pinned ? "Unpin current folder" : "Pin current folder"}
              onclick={() => togglePin(active?.folder?.path)}
            ><Icon name="pin" size={14} /></button>
          </div>
          {#each session.pins as pin (pin.path)}
            {@const style = styleOf(pin.item)}
            {@const place = { path: pin.path, item: pin.item, name: style?.alias || pin.name }}
            {@const showLine = !!pinDrag && !pinStays(pinDrag.path, pinDrag.before)}
            <button class="pin" data-pin={pin.path} data-pinned class:on={pin.path === active?.folder?.path} class:moving={pinDrag?.path === pin.path} class:line-before={showLine && pinDrag?.before === pin.path} class:line-after={showLine && pinDrag?.before === null && pin === session.pins.at(-1)} title={pin.path} onpointerdown={(event) => pinDown(event, pin.path)} onclick={() => active?.open(pin.path)} oncontextmenu={(event) => placeMenu(event, "pin", place)} onkeydown={(event) => placeMenu(event, "pin", place)}>
              <FolderIcon color={tint(style)} glyph={style?.icon} icon={pin.icon} /><span>{style?.alias || pin.name}</span>
            </button>
          {:else}
            <p class="muted side-empty">Pin folders you use often. They stay here until you unpin them.</p>
          {/each}
          <div class="side-head pc-head">
            <button class="side-toggle" aria-expanded={pcOpen} onclick={togglePcSection}><span class="turn" class:open={pcOpen}><Icon name="chevron" size={12} weight={2} /></span><span class="side-title">This PC</span></button>
          </div>
          {#if pcOpen}
            {#each drives as drive (drive.path)}
              {@const place = { path: drive.path, item: drive.item, name: drive.name }}
              <button class="pin" data-pin={drive.path} class:on={drive.path === active?.folder?.path} title={drive.path} onclick={() => active?.open(drive.path)} oncontextmenu={(event) => placeMenu(event, "drive", place)} onkeydown={(event) => placeMenu(event, "drive", place)}>
                <FolderIcon icon={drive.icon} /><span>{drive.name}</span>
              </button>
            {/each}
          {/if}
        </nav>
        <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
        <div
          class="side-resize"
          class:dragging={sidebarDrag !== undefined}
          role="separator"
          aria-orientation="vertical"
          aria-label="Sidebar width"
          aria-valuemin={SIDEBAR_MIN}
          aria-valuemax={SIDEBAR_MAX}
          aria-valuenow={sidebarWidth}
          tabindex="0"
          title="Drag to resize · double-click resets"
          onpointerdown={resizeSidebar}
          onkeydown={sidebarKey}
          ondblclick={() => void saveSidebarWidth(232)}
        ></div>
      {/if}
      <div class="panes" use:nonPassiveWheel={zoom}>
        {#each slots as slot, index (slot)}
          <div class="pane-slot">
          {#if slots.length > 1}
            <TabBar {slot} styleOf={tabStyle} onselect={tab => selectTab(slot, tab)} onclose={tab => closeTab(slot, tab)} onnew={() => newTab(undefined, slot)} ondetach={tab => detachTab(slot, tab)} />
            {#if slot.active?.kind === "folder"}
              {@const pane = slot.active.pane}
              <div class="pane-nav">
                <button class="ib" title="Back" aria-label="Pane back" disabled={!pane.canBack} onclick={() => pane.back()}><Icon name="back" /></button>
                <button class="ib" title="Forward" aria-label="Pane forward" disabled={!pane.canForward} onclick={() => pane.forward()}><Icon name="forward" /></button>
                <input class="field pane-address" aria-label="Pane folder path" value={pane.path} onfocus={() => (activeIndex = index)} onkeydown={(event) => { if (event.key === "Enter") void pane.open(event.currentTarget.value); }} />
                <div class="pane-search" onfocusin={() => (activeIndex = index)}>
                  <SearchBox {pane} label={paneLabel(pane)} compact onvolume={drive => (volumeAsk = { drive, pane })} />
                </div>
              </div>
            {/if}
          {/if}
          {#if slot.active?.kind === "folder"}
          {#key slot.active.id}
          <FileList
            bind:this={lists[index]}
            pane={slot.active.pane}
            id={`pane-${index}`}
            marked={slots.length > 1 && index === activeIndex}
            grid={gridView}
            {tile}
            row={rowHeight}
            onactivate={() => (activeIndex = index)}
            onopen={open}
            onmenu={(x, y, onRow) => showMenu(onRow ? "item" : "area", x, y)}
            onrename={rename}
            onrecycle={recycle}
            ondrag={(event) => { if (slot.active?.kind === "folder") beginDrag(event, slot.active.pane); }}
          />
          {/key}
          {:else if slot.active?.kind === "settings" && settings}
            <Settings {settings} dark={dark ?? false} bind:section={settingsSection} onappearance={previewAppearance} onsave={saveOrThrow} />
          {/if}
          </div>
        {/each}
      </div>
    </div>

    <footer class="status" role="status" class:collapsed={settingsOnly && !transferRunning && !fatal}>
      {#if fatal || active?.error}
        <span class="error">{fatal || active?.error}</span>
      {:else if active}
        <span>{active.searching ? resultText(active) : `${formatCount(active.rows)} ${active.rows === 1 ? "item" : "items"}${active.complete ? "" : " · reading…"}`}{selectedCount > 0 ? ` · ${selectedCount} selected` : ""}</span>
      {/if}
      {#if session.clipboard}
        <span>· {session.clipboard.count} {session.clipboard.cut ? "cut" : "copied"}</span>
      {/if}
      <span class="grow"></span>
      {#if transfer}
        <TransferPill status={transfer} ondismiss={() => (transfer = null)} />
      {/if}
      {#if active?.volume}
        {@const volume = volumes.states[active.volume]}
        <span>{volume?.ready ? `${active.volume}: index · ${formatCount(volume.count)} items` : `Indexing ${active.volume}:…`}</span>
      {:else if active?.index === "building"}
        <span>Indexing folder…</span>
      {:else if active?.index === "ready"}
        <span>Folder index ready · {formatCount(active.indexCount)} items</span>
      {/if}
      {#if active && space && space.path === active.folder?.path}
        <span class="space" title={`${formatSize(space.free)} free of ${formatSize(space.total)}`}>{formatSize(space.free)} free</span>
      {/if}
    </footer>
  </div>

  {#if drag}
    <div class="drag-ghost" class:refused={!!drag.refusal} style:left={`${drag.x + 14}px`} style:top={`${drag.y + 12}px`} role="status" aria-live="polite">
      <Icon name={drag.refusal ? "close" : drag.move ? "transfer" : "copy"} size={14} />
      <span>{drag.entries.length === 1 ? (drag.entries[0].style?.alias || drag.entries[0].name) : `${drag.entries.length} items`}</span>
      <span class="muted">{drag.refusal || `${drag.move ? "Move" : "Copy"} to ${drag.target?.name}${drag.move ? "" : " · Shift moves"}`}</span>
    </div>
  {/if}

  {#if toast}
    <div class="toast" class:error={toast.error} role="alert">
      <span>{toast.message}</span>
      {#if toast.undo}<button class="toast-action" onclick={undo}>Undo <kbd class="kbd">Ctrl Z</kbd></button>{/if}
      <button class="ib xs" aria-label="Dismiss" onclick={() => (toast = undefined)}><Icon name="close" size={12} /></button>
    </div>
  {/if}

  {#if menu}
    <!-- svelte-ignore a11y_mouse_events_have_key_events -->
    <div class="menu" class:scrolls={menu.kind !== "more"} role="menu" tabindex="-1" bind:this={menuElement} style:left={`${menu.x}px`} style:top={`${menu.y}px`} onmousedown={(event) => event.stopPropagation()} onkeydown={menuKey} onmouseover={(event) => { if (!(event.target as Element).closest(".shell")) { clearTimeout(hoverTimer); if (submenus.length) submenus = []; } }}>
      {#if menu.kind === "item" || menu.kind === "area"}
        {@const onItem = menu.kind === "item"}
        {@const split = splitRows(menuRows(settings?.menu ?? [], menuContext(onItem)))}
        {#if split.quick.length > 0}
          <div class="quick" role="group" aria-label="Clipboard and file actions">
            {#each split.quick as row (row.index)}
              {@const label = labelOf(row.entry).replace(/…$/, "")}
              {@const keys = details(row.entry.action).shortcut}
              <button class="qi" role="menuitem" aria-label={label} title={keys ? `${label} (${keys})` : label} disabled={!row.enabled} style:color={row.entry.color ? hex(row.entry.color) : undefined} onclick={() => runEntry(row, onItem)}>
                {#if row.entry.icon}<span class="glyph">{row.entry.icon}</span>{:else}<Icon name={details(row.entry.action).icon} />{/if}
              </button>
            {/each}
          </div>
          {#if split.rows.length > 0}<div class="sep"></div>{/if}
        {/if}
        {#each split.rows as row (row.index)}
          {#if row.divider}<div class="sep"></div>{/if}
          <button class="mi" role="menuitem" disabled={!row.enabled} onclick={() => runEntry(row, onItem)}>
            {#if row.entry.icon || row.entry.color}
              <span class="glyph" style:color={row.entry.color ? hex(row.entry.color) : undefined}>{#if row.entry.icon}{row.entry.icon}{:else}<Icon name={details(row.entry.action).icon} />{/if}</span>
            {:else}<Icon name={details(row.entry.action).icon} />{/if}
            <span class="label">{labelOf(row.entry)}</span><span class="sc">{details(row.entry.action).shortcut}</span>
          </button>
          {#if onItem && row.entry.action === "open" && focused?.directory}
            <button class="mi" role="menuitem" disabled={!row.enabled} onclick={() => focused && open(focused, true)}><Icon name="window" /><span class="label">Open in new tab</span><span class="sc">Ctrl+Enter</span></button>
          {/if}
        {/each}
        <div class="sep"></div>
        {#if onItem}
          {#if selectedCount === 1 && (focused?.directory || isShortcut(focused))}
            <button class="mi" role="menuitem" onclick={() => changeIcon(focused?.id)}><Icon name="image" /><span class="label">Change icon…</span></button>
          {/if}
          {#if focused?.directory && selectedCount === 1}
            <button class="mi" role="menuitem" onclick={() => togglePin(focused?.path)}><Icon name="pin" /><span class="label">{session.pins.some((pin) => pin.path === focused?.path) ? "Unpin from sidebar" : "Pin to sidebar"}</span></button>
          {/if}
          <button class="mi" role="menuitem" onclick={() => copyPath(focused?.path)}><Icon name="copy" /><span class="label">Copy path</span></button>
        {:else}
          <button class="mi" role="menuitem" onclick={() => togglePin(active?.folder?.path)}><Icon name="pin" /><span class="label">{pinned ? "Unpin this folder" : "Pin this folder"}</span></button>
        {/if}
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={() => openSettings("menu")}><Icon name="settings" /><span class="label">Edit menu…</span></button>
        {@render shellBlock()}
      {:else if menu.target}
        {@const place = menu.target}
        {@const isPinned = session.pins.some((pin) => pin.path === place.path)}
        <button class="mi" role="menuitem" onclick={() => openPlace(place, false)}><Icon name="open" /><span class="label">Open</span></button>
        <button class="mi" role="menuitem" onclick={() => openPlace(place, true)}><Icon name="window" /><span class="label">Open in new tab</span></button>
        <div class="sep"></div>
        {#if place.item}
          <button class="mi" role="menuitem" onclick={() => customizePlace(place)}><Icon name="palette" /><span class="label">Customize folder…</span></button>
          {#if menu.kind === "pin"}
            <button class="mi" role="menuitem" onclick={() => changeIcon(place.item)}><Icon name="image" /><span class="label">Change icon…</span></button>
          {/if}
        {/if}
        <button class="mi" role="menuitem" onclick={() => togglePin(place.path)}><Icon name="pin" /><span class="label">{isPinned ? "Unpin from sidebar" : "Pin to sidebar"}</span></button>
        <button class="mi" role="menuitem" onclick={() => copyPath(place.path)}><Icon name="copy" /><span class="label">Copy path</span></button>
        {@render shellBlock()}
      {:else}
        <button class="mi" role="menuitem" onclick={() => openSettings()}><Icon name="settings" /><span class="label">Settings</span><span class="sc">Ctrl+,</span></button>
        <div class="sep"></div>
        <div class="menu-title">View</div>
        <button class="mi" class:on={!gridView} role="menuitemradio" aria-checked={!gridView} onclick={() => setView(false)}><Icon name="list" /><span class="label">Details</span><span class="sc">Ctrl+Shift+6</span></button>
        {#each TILES as [size, name, keys] (size)}
          <button class="mi" class:on={gridView && tile === size} role="menuitemradio" aria-checked={gridView && tile === size} onclick={() => setView(true, size)}><Icon name="grid" /><span class="label">{name}</span><span class="sc">{keys}</span></button>
        {/each}
        <div class="sep"></div>
        <div class="menu-title">Theme</div>
        {#each PRESETS as [value, css, name] (value)}
          <button class="mi" class:on={preset === css} role="menuitemradio" aria-checked={preset === css} onclick={() => choosePreset(value)}><Icon name="palette" /><span class="label">{name}</span></button>
        {/each}
        {#if preset === "light" || preset === "neu"}<button class="mi" role="menuitemcheckbox" aria-checked={dark ?? false} onclick={toggleDark}><Icon name="check" /><span class="label">Dark variant</span><span class="sc">{dark ? "On" : "Off"}</span></button>{/if}
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={newWindow}><Icon name="window" /><span class="label">New window</span><span class="sc">Ctrl+N</span></button>
        <button class="mi" role="menuitem" onclick={toggleSplit}><Icon name="split" /><span class="label">{slots.length > 1 ? "Single pane" : "Split view"}</span><span class="sc">Ctrl+\</span></button>
      {/if}
    </div>
  {/if}
  {#each submenus as panel, index (index)}
    <div class="menu scrolls shell-sub" role="menu" tabindex="-1" style:left={`${panel.x}px`} style:top={`${panel.y}px`} onmousedown={(event) => event.stopPropagation()} onkeydown={(event) => submenuKey(event, index)}>
      {@render shellRows(panel.items, index + 1)}
    </div>
  {/each}
  {#if folderSheet}
    {@const item = folderSheet.item}
    <FolderStyleSheet name={folderSheet.name} path={folderSheet.path} style={styleOf(item)} onsave={saveFolderStyle} onclose={() => (folderSheet = undefined)} onpickicon={() => api.pickCustomIcon(item)} onreseticon={() => api.resetCustomIcon(item)} />
  {/if}
  {#if recycleAsk}
    {@const items = recycleAsk}
    {@const name = (entry: Entry) => entry.style?.alias || entry.name}
    <Sheet title={items.length === 1 ? `Move “${name(items[0])}” to the Recycle Bin?` : `Move ${items.length} items to the Recycle Bin?`} onclose={cancelRecycle}>
      {#if items.length > 1}
        <ul class="recycle-list">
          {#each items.slice(0, 5) as entry (entry.path)}<li>{name(entry)}</li>{/each}
          {#if items.length > 5}<li class="muted">and {items.length - 5} more</li>{/if}
        </ul>
      {/if}
      <p class="muted">You can restore {items.length === 1 ? "it" : "them"} with Undo (Ctrl+Z) while Firewol is open, or from the Recycle Bin.</p>
      <div class="actions">
        <button class="action" onclick={cancelRecycle}>Cancel</button>
        <button class="action primary" use:initialFocus onclick={confirmRecycle}>Move to Recycle Bin</button>
      </div>
    </Sheet>
  {/if}
  {#if openAsk}
    <Sheet title={`Open ${openAsk.entries.length} items?`} onclose={cancelOpen}>
      <p class="muted">Each item opens in its Windows default app or a new tab.</p>
      <div class="actions">
        <button class="action" onclick={cancelOpen}>Cancel</button>
        <button class="action primary" use:initialFocus onclick={confirmOpen}>Open all</button>
      </div>
    </Sheet>
  {/if}
  {#if volumeAsk}
    {@const drive = volumeAsk.drive}
    <Sheet title={`Search all of ${drive}:?`} onclose={() => (volumeAsk = undefined)}>
      <p>Firewol indexes the files you can open on {drive}: for this session and keeps the index up to date while it runs.</p>
      <p class="muted">Reading the NTFS change journal needs a small read-only helper with administrator rights, so Windows asks for approval next. The helper cannot change files. If you decline, folder search keeps working.</p>
      <div class="actions">
        <button class="action" onclick={() => (volumeAsk = undefined)}>Cancel</button>
        <button class="action primary" onclick={startVolume}>Continue</button>
      </div>
    </Sheet>
  {/if}
  {#if closeWarning}
    <Sheet title="A file operation is running" onclose={() => (closeWarning = false)}>
      <p>Keep Firewol open until this operation finishes. You can continue browsing while it runs.</p>
      <div class="actions">
        <button class="action" onclick={() => { closeWhenFinished = true; closeWarning = false; if (!transferRunning) void appWindow.close(); }}>Close when finished</button>
        <button class="action primary" onclick={() => (closeWarning = false)}>Keep open</button>
      </div>
    </Sheet>
  {/if}
</div>

<style>
  .shell { display: flex; flex-direction: column; height: 100%; }
  .titlebar { display: flex; align-items: center; gap: 4px; flex: none; height: 44px; padding-left: 10px; }
  .app-title { padding: 0 12px; color: var(--muted); }
  .drag { flex-grow: 1; align-self: stretch; }
  .caption { display: flex; align-self: flex-start; }
  .caption.left { order: -1; margin-right: 8px; }
  .caption.rounded .cap { border-radius: 6px; }
  .caption.circle .cap { border-radius: 50%; width: 28px; height: 28px; margin: 6px; }
  .caption.minimal .cap { width: 32px; }
  .cap { display: flex; align-items: center; justify-content: center; width: 46px; height: 40px; padding: 0; border: 0; background: var(--caption-fill, transparent); }
  .cap:hover { background: var(--hover); }
  .cap.close:hover { background: var(--caption-hover, #c42b1c); color: #ffffff; }
  .toolbar { display: flex; align-items: center; gap: 4px; flex: none; min-height: 52px; padding: 6px 12px 8px 10px; }
  .address { flex: 1 1 360px; min-width: 0; margin-left: 6px; overflow: hidden; }
  .accent { color: var(--accent); }
  .search-gap { flex: 0 1 280px; }
  .body { display: flex; flex: 1; min-height: 0; gap: 8px; padding: 0 12px 0 8px; }
  .sidebar { display: flex; flex-direction: column; gap: 2px; flex: 0 0 232px; min-width: 0; padding: 2px 0; overflow: auto; }
  /* Sits in the 8 px gap beside the sidebar; a thin accent line shows while hovered or dragged. */
  .side-resize { position: relative; flex: none; width: 8px; margin: 0 -8px; cursor: col-resize; touch-action: none; }
  .side-resize::after { content: ""; position: absolute; top: 8px; bottom: 8px; left: 3px; width: 2px; border-radius: 1px; background: transparent; transition: background 120ms ease-out; }
  .side-resize:hover::after, .side-resize.dragging::after, .side-resize:focus-visible::after { background: var(--accent); }
  .side-resize:focus-visible { outline: none; }
  .side-head { display: flex; align-items: center; justify-content: space-between; padding: 0 4px 4px 12px; }
  .side-title { font-size: 12px; font-weight: 600; color: var(--muted); }
  .pc-head { margin-top: 8px; padding-left: 4px; }
  .side-toggle { display: flex; align-items: center; gap: 4px; height: 26px; padding: 0 8px 0 4px; border: 0; border-radius: var(--radius-s); background: transparent; }
  .side-toggle:hover { background: var(--hover); }
  .turn { display: inline-flex; color: var(--muted); transition: transform 120ms ease-out; }
  .turn.open { transform: rotate(90deg); }
  .side-empty { margin: 0; padding: 4px 12px; font-size: 12px; line-height: 18px; }
  .pin { display: flex; align-items: center; gap: 10px; width: 100%; height: 32px; padding: 0 12px; border: 0; border-radius: var(--radius-s); background: transparent; font-size: 13px; text-align: left; }
  .pin { position: relative; }
  .pin.moving { opacity: .5; }
  .pin.line-before::before, .pin.line-after::after { content: ""; position: absolute; left: 8px; right: 8px; height: 2px; border-radius: 1px; background: var(--accent); pointer-events: none; }
  .pin.line-before::before { top: -2px; }
  .pin.line-after::after { bottom: -2px; }
  .pin span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .pin:hover { background: var(--hover); }
  .pin.on { background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); font-weight: 600; }
  .panes { display: flex; flex: 1; min-width: 0; gap: 12px; }
  .pane-slot { display: flex; flex-direction: column; gap: 8px; flex: 1; min-width: 0; }
  .pane-nav { display: flex; gap: 4px; min-width: 0; }
  .pane-address { min-width: 0; width: 45%; flex: 1; }
  .pane-search { display: flex; flex: 0 1 35%; min-width: 120px; }
  .pane-search:focus-within { flex-basis: 60%; }
  .pane-search :global(.box) { flex: 1; }
  .body.settings-only { padding: 8px 12px 0 8px; }
  .glyph { display: inline-flex; justify-content: center; flex: none; width: 16px; color: var(--accent); }
  .status.collapsed { height: 12px; overflow: hidden; visibility: hidden; }
  .status { display: flex; align-items: center; gap: 12px; flex: none; height: 32px; padding: 0 16px 0 20px; font-size: 12px; color: var(--muted); }
  .grow { flex-grow: 1; }
  .space { flex: none; white-space: nowrap; }
  .scrolls { max-width: 420px; max-height: calc(100vh - 16px); overflow: auto; }
  .shell-sub { z-index: 21; }
  .shell .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .shell-blank { flex: none; width: 16px; }
  .shell-wait { color: var(--muted); }
  .quick { display: flex; gap: 2px; padding: 2px; }
  .qi { display: flex; align-items: center; justify-content: center; flex: 1; height: 32px; min-width: 36px; padding: 0; border: 0; border-radius: var(--radius-s); background: transparent; color: var(--text); }
  .qi:hover:not(:disabled), .qi:focus-visible { background: var(--hover); outline: none; }
  .qi:disabled { opacity: .4; }
  .recycle-list { margin: 16px 0 0; padding-left: 20px; line-height: 22px; }
  .recycle-list li { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .error { color: var(--text); }
  :global(.ib.xs) { width: 22px; height: 22px; border-radius: 999px; }
  .drag-ghost { position: fixed; z-index: 40; display: flex; align-items: center; gap: 8px; max-width: 420px; padding: 6px 12px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); box-shadow: var(--shadow-pop); backdrop-filter: var(--blur); font-size: 12px; pointer-events: none; white-space: nowrap; }
  .drag-ghost span { overflow: hidden; text-overflow: ellipsis; }
  .drag-ghost.refused { color: var(--muted); }
  :global(.tab.drop-target), :global(.pin.drop-target) { outline: 2px dashed var(--accent); outline-offset: -2px; }
  .toast { position: fixed; left: 50%; bottom: 52px; z-index: 30; display: flex; align-items: center; gap: 12px; max-width: min(640px, calc(100% - 32px)); padding: 8px 8px 8px 16px; transform: translateX(-50%); border: var(--border-w) solid var(--border); border-radius: var(--radius); background: var(--surface-2); box-shadow: var(--shadow-pop); backdrop-filter: var(--blur); font-size: 13px; }
  .toast.error { border-color: #c42b1c; }
  .toast span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .toast-action { display: flex; align-items: center; gap: 8px; flex: none; height: 28px; padding: 0 10px; border: 0; border-radius: var(--radius-s); background: var(--sel); color: var(--sel-text); font-size: 13px; font-weight: 600; }
  .toast-action:hover { background: var(--hover); }
</style>
