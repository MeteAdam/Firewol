<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, tick, untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { api } from "./lib/api";
  import {
    FOLDER_PROGRESS,
    INDEX_STATE,
    VOLUME_STATE,
    OPERATION_DONE,
    SESSION_CHANGED,
    SETTINGS_CHANGED,
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
  } from "./lib/contract";
  import FileList from "./lib/FileList.svelte";
  import FolderIcon from "./lib/FolderIcon.svelte";
  import FolderStyleSheet from "./lib/FolderStyleSheet.svelte";
  import { formatCount, formatSize, useLocale } from "./lib/format";
  import Icon from "./lib/Icon.svelte";
  import { NO_FILTERS, Pane } from "./lib/pane.svelte";
  import { PRESETS, colors, hex, presetClass } from "./lib/appearance";
  import { details, isCustom, labelOf, menuRows, parentPath, type MenuContext, type MenuRow } from "./lib/menu";
  import SearchBox from "./lib/SearchBox.svelte";
  import Sheet from "./lib/Sheet.svelte";
  import Settings from "./lib/Settings.svelte";
  import { styleKey, styleMap, tint } from "./lib/styles";
  import TabBar from "./lib/TabBar.svelte";
  import { volumes } from "./lib/volumes.svelte";
  import { PaneSlot, folderTab, restore, snapshot, type Tab } from "./lib/tabs.svelte";

  type Check = (step: string, ok: boolean, detail?: string) => void;

  const QUERIES = ["ré", "cord", "port", "résumé", "record-0000"];

  const appWindow = getCurrentWindow();
  let startup = $state<Startup>();
  let session = $state.raw<SessionState>({ pins: [], clipboard: null });
  let slots = $state<PaneSlot[]>([]);
  const panes = $derived(slots.flatMap(slot => slot.active?.kind === "folder" ? [slot.active.pane] : []));
  let lists = $state<FileList[]>([]);
  let paneCounter = 0;
  let activeIndex = $state(0);
  let settings = $state.raw<SettingsView>();
  const windowsDark = new MediaQuery("(prefers-color-scheme: dark)");
  const sidebar = $derived(settings?.sidebar ?? true);
  const preset = $derived(presetClass(settings?.appearance.preset ?? "light_morphism"));
  const dark = $derived((preset === "light" || preset === "neu") && (settings?.appearance.follow_windows ? windowsDark.current : settings?.appearance.dark));
  let closeWarning = $state(false);
  let closeWhenFinished = false;
  let allowClose = false;
  let editing = $state(false);
  let address = $state("");
  let menu = $state<{ kind: "item" | "area" | "more"; x: number; y: number }>();
  let toast = $state<{ message: string; undo: boolean; error: boolean }>();
  let transfer = $state.raw<TransferStatus | null>(null);
  let fatal = $state("");
  let searchBox: SearchBox | undefined = $state();
  /** The NTFS drive waiting for in-window confirmation before Windows asks for approval. */
  let volumeAsk = $state<{ drive: string; pane: Pane }>();
  let settingsSection = $state("appearance");
  let folderSheet = $state<{ item: ItemRef; name: string; path: string }>();
  let addressField: HTMLInputElement | undefined = $state();
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let transferTimer: ReturnType<typeof setInterval> | undefined;

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
  async function start(action: Promise<void>, transfers = false) {
    menu = undefined;
    try {
      await action;
      if (transfers) watchTransfer();
    } catch (reason) {
      notify(String(reason), { error: true });
    }
  }

  function watchTransfer() {
    clearInterval(transferTimer);
    transferTimer = setInterval(async () => {
      transfer = await api.transferStatus().catch(() => null);
    }, 300);
  }

  function stopTransferWatch() {
    clearInterval(transferTimer);
    transferTimer = undefined;
    transfer = null;
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
    stopTransferWatch();
    if (done.refresh) for (const pane of panes) void pane.refresh();
    notify(done.message, { undo: done.succeeded, error: !done.succeeded });
    void api.session().then((state) => (session = state));
  }

  function open(entry: Entry, inNewTab: boolean) {
    menu = undefined;
    if (!entry.directory) return;
    if (inNewTab) void newTab(entry.path);
    else void active?.open(entry.path);
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
    if (into && session.clipboard) void start(api.paste(into), true);
  }

  function toOtherPane(moveItems: boolean) {
    const into = other?.folder?.item;
    const items = active?.selectedRefs() ?? [];
    if (into && items.length > 0) void start(api.transfer(items, into, moveItems), true);
  }

  function rename(entry: Entry, name: string) {
    if (entry.id) void start(api.rename(entry.id, name));
  }

  function recycle() {
    const entries = active?.selectedEntries() ?? [];
    if (entries.length !== 1 || !entries[0].id) {
      notify("Select one item to move to the Recycle Bin", { error: true });
      return;
    }
    void start(api.recycle(entries[0].id));
  }

  function undo() {
    toast = undefined;
    void start(api.undo(), true);
  }

  function menuContext(onItem: boolean): MenuContext {
    return {
      item: onItem ? focused : undefined,
      selectedCount,
      searching: !!active?.searching,
      current: !!active?.folder?.item,
      clipboard: !!session.clipboard,
      idle: !transfer,
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
        case "open": if (item) open(item, false); break;
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

  function showMenu(kind: "item" | "area" | "more", x: number, y: number) {
    menu = { kind, x: Math.min(x, window.innerWidth - 268), y: Math.min(y, window.innerHeight - 380) };
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
    else if (event.altKey && event.key === "ArrowLeft") void active?.back();
    else if (event.altKey && event.key === "ArrowRight") void active?.forward();
    else if (event.altKey && event.key === "ArrowUp") void active?.up();
    else if (event.key === "F5") void active?.refresh();
    else if (typing) return;
    else if (event.ctrlKey && key === "c") void clip(false);
    else if (event.ctrlKey && key === "x") void clip(true);
    else if (event.ctrlKey && key === "v") paste();
    else if (event.ctrlKey && key === "z") undo();
    else return;
    event.preventDefault();
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
      done = nextDone();
      recycle();
      result = await settle(target, done);
      check("move to the Recycle Bin", result.succeeded && !names(target).includes("note.txt"), result.message);
      done = nextDone();
      undo();
      result = await settle(target, done);
      check("undo recycle", result.succeeded && names(target).includes("note.txt"), result.message);
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
      check("the context menu follows the saved menu", labels.includes("Self-test action") && !labels.split("|").includes("Copy") && labels.includes("Cut") && labels.includes("Edit menu…") && !labels.includes("Customize folder"), labels);
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
      listen<OperationDone>(OPERATION_DONE, (event) => finished(event.payload)),
      listen<SessionState>(SESSION_CHANGED, (event) => (session = event.payload)),
      listen<SettingsView>(SETTINGS_CHANGED, (event) => (settings = withPending(event.payload))),
      appWindow.onCloseRequested(async (event) => {
        if (allowClose) return;
        event.preventDefault();
        await flushAppearance();
        if (await api.operationBusy()) closeWarning = true;
        else { allowClose = true; await appWindow.close(); }
      }),
    ];
    void (async () => {
      const info = await api.startup();
      await Promise.all([loadSettings(), volumes.load()]);
      const start = performance.now();
      useLocale(info.locale);
      startup = info;
      session = info.session;
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
      else if (info.report && !info.adopt) {
        await benchmark(info, firstListMs).catch((reason) =>
          api.saveReport({ label: info.label, error: String(reason) }),
        );
      }
    })().catch((reason) => (fatal = String(reason)));
    return () => {
      clearInterval(transferTimer);
      unlisten.forEach((promise) => void promise.then((stop) => stop()));
    };
  });
</script>

<svelte:window onkeydown={shortcut} onmousedown={() => (menu = undefined)} />

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
        <nav class="field address" aria-label="Folder path">
          <span class="muted icon"><Icon name="drive" /></span>
          {#each active?.folder?.segments ?? [] as segment, index (segment.path)}
            {#if index > 0}<span class="muted icon"><Icon name="chevron" size={12} weight={2} /></span>{/if}
            <button class="crumb" class:cur={index === (active?.folder?.segments.length ?? 0) - 1} onclick={() => active?.open(segment.path)}>{segment.name}</button>
          {/each}
          <button class="crumb-space" aria-label="Edit path (Ctrl+L)" onclick={editAddress}></button>
          <button class="ib sm" aria-label="Refresh (F5)" title="Refresh (F5)" onclick={() => active?.refresh()}><Icon name="refresh" size={14} /></button>
        </nav>
      {/if}
      {#if active}
        {#key active}
          <SearchBox bind:this={searchBox} pane={active} label={title} onvolume={drive => (volumeAsk = { drive, pane: active! })} />
        {/key}
      {:else}
        <div class="search-gap"></div>
      {/if}
      <div class="seg" role="group" aria-label="View">
        <button class="ib on" aria-pressed="true" aria-label="Details view" title="Details view"><Icon name="list" /></button>
        <button class="ib" aria-pressed="false" aria-label="Grid view (not in this build)" title="Grid view (not in this build)" disabled><Icon name="grid" /></button>
      </div>
      <button class="ib" class:on={slots.length > 1} aria-pressed={slots.length > 1} aria-label="Split view (Ctrl+\)" title="Split view (Ctrl+\)" onclick={toggleSplit}><Icon name="split" /></button>
      <button class="ib" aria-label="More" title="More" aria-haspopup="menu" onmousedown={(event) => event.stopPropagation()} onclick={(event) => showMenu("more", event.clientX - 220, event.clientY + 16)}><Icon name="more" /></button>
    </div>
    {/if}

    <div class="body" class:settings-only={settingsOnly}>
      {#if sidebar && !settingsOnly}
        <nav class="sidebar" aria-label="Pinned folders">
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
            <button class="pin" class:on={pin.path === active?.folder?.path} title={pin.path} onclick={() => active?.open(pin.path)}>
              <FolderIcon color={tint(style)} glyph={style?.icon} /><span>{style?.alias || pin.name}</span>
            </button>
          {:else}
            <p class="muted side-empty">Pin folders you use often. They stay here until you unpin them.</p>
          {/each}
        </nav>
      {/if}
      <div class="panes">
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
            onactivate={() => (activeIndex = index)}
            onopen={open}
            onmenu={(x, y, onRow) => showMenu(onRow ? "item" : "area", x, y)}
            onrename={rename}
            onrecycle={recycle}
          />
          {/key}
          {:else if slot.active?.kind === "settings" && settings}
            <Settings {settings} dark={dark ?? false} bind:section={settingsSection} onappearance={previewAppearance} onsave={saveOrThrow} />
          {/if}
          </div>
        {/each}
      </div>
    </div>

    <footer class="status" role="status" class:collapsed={settingsOnly && !transfer && !fatal}>
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
        <span class="pill">
          <span class="accent icon"><Icon name="transfer" size={14} /></span>
          <span>{transfer.phase === "paused" ? "Paused" : transfer.phase === "cancelled" ? "Cancelling" : "Transferring"} · {formatSize(transfer.bytes)} · {transfer.finished} done</span>
          <button class="ib xs" aria-label={transfer.phase === "paused" ? "Resume transfer" : "Pause transfer"} onclick={() => api.pauseTransfer()}><Icon name={transfer.phase === "paused" ? "play" : "pause"} size={12} /></button>
          <button class="ib xs" aria-label="Cancel transfer" onclick={() => api.cancelTransfer()}><Icon name="close" size={12} /></button>
        </span>
      {/if}
      {#if active?.volume}
        {@const volume = volumes.states[active.volume]}
        <span>{volume?.ready ? `${active.volume}: index · ${formatCount(volume.count)} items` : `Indexing ${active.volume}:…`}</span>
      {:else if active?.index === "building"}
        <span>Indexing folder…</span>
      {:else if active?.index === "ready"}
        <span>Folder index ready · {formatCount(active.indexCount)} items</span>
      {/if}
    </footer>
  </div>

  {#if toast}
    <div class="toast" class:error={toast.error} role="alert">
      <span>{toast.message}</span>
      {#if toast.undo}<button class="toast-action" onclick={undo}>Undo <kbd class="kbd">Ctrl Z</kbd></button>{/if}
      <button class="ib xs" aria-label="Dismiss" onclick={() => (toast = undefined)}><Icon name="close" size={12} /></button>
    </div>
  {/if}

  {#if menu}
    <div class="menu" role="menu" tabindex="-1" style:left={`${menu.x}px`} style:top={`${menu.y}px`} onmousedown={(event) => event.stopPropagation()}>
      {#if menu.kind === "item" || menu.kind === "area"}
        {@const onItem = menu.kind === "item"}
        {#each menuRows(settings?.menu ?? [], menuContext(onItem)) as row (row.index)}
          {#if row.divider}<div class="sep"></div>{/if}
          <button class="mi" role="menuitem" disabled={!row.enabled} onclick={() => runEntry(row, onItem)}>
            {#if row.entry.icon || row.entry.color}
              <span class="glyph" style:color={row.entry.color ? hex(row.entry.color) : undefined}>{#if row.entry.icon}{row.entry.icon}{:else}<Icon name={details(row.entry.action).icon} />{/if}</span>
            {:else}<Icon name={details(row.entry.action).icon} />{/if}
            <span class="label">{labelOf(row.entry)}</span><span class="sc">{details(row.entry.action).shortcut}</span>
          </button>
          {#if onItem && row.entry.action === "open"}
            <button class="mi" role="menuitem" disabled={!row.enabled} onclick={() => focused && open(focused, true)}><Icon name="window" /><span class="label">Open in new tab</span><span class="sc">Ctrl+Enter</span></button>
          {/if}
        {/each}
        <div class="sep"></div>
        {#if onItem}
          {#if focused?.directory && selectedCount === 1}
            <button class="mi" role="menuitem" onclick={() => togglePin(focused?.path)}><Icon name="pin" /><span class="label">{session.pins.some((pin) => pin.path === focused?.path) ? "Unpin from sidebar" : "Pin to sidebar"}</span></button>
          {/if}
          <button class="mi" role="menuitem" onclick={() => { if (focused) void navigator.clipboard.writeText(focused.path); menu = undefined; }}><Icon name="copy" /><span class="label">Copy path</span></button>
        {:else}
          <button class="mi" role="menuitem" onclick={() => togglePin(active?.folder?.path)}><Icon name="pin" /><span class="label">{pinned ? "Unpin this folder" : "Pin this folder"}</span></button>
        {/if}
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={() => openSettings("menu")}><Icon name="settings" /><span class="label">Edit menu…</span></button>
      {:else}
        <button class="mi" role="menuitem" onclick={() => openSettings()}><Icon name="settings" /><span class="label">Settings</span><span class="sc">Ctrl+,</span></button>
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
  {#if folderSheet}
    <FolderStyleSheet name={folderSheet.name} path={folderSheet.path} style={styleOf(folderSheet.item)} onsave={saveFolderStyle} onclose={() => (folderSheet = undefined)} />
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
        <button class="action" onclick={() => { closeWhenFinished = true; closeWarning = false; if (!transfer) void appWindow.close(); }}>Close when finished</button>
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
  .icon { display: flex; flex: none; }
  .accent { color: var(--accent); }
  .crumb { flex: none; padding: 3px 6px; border: 0; border-radius: 6px; background: transparent; font-size: 13px; }
  .crumb:hover { background: var(--hover); }
  .crumb.cur { font-weight: 600; }
  .crumb-space { flex-grow: 1; align-self: stretch; min-width: 24px; border: 0; background: transparent; }
  .search-gap { flex: 0 1 280px; }
  .body { display: flex; flex: 1; min-height: 0; gap: 8px; padding: 0 12px 0 8px; }
  .sidebar { display: flex; flex-direction: column; gap: 2px; flex: 0 0 216px; padding: 2px 0; overflow: auto; }
  .side-head { display: flex; align-items: center; justify-content: space-between; padding: 0 4px 4px 12px; }
  .side-title { font-size: 12px; font-weight: 600; color: var(--muted); }
  .side-empty { margin: 0; padding: 4px 12px; font-size: 12px; line-height: 18px; }
  .pin { display: flex; align-items: center; gap: 10px; width: 100%; height: 32px; padding: 0 12px; border: 0; border-radius: var(--radius-s); background: transparent; font-size: 13px; text-align: left; }
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
  .status { display: flex; align-items: center; gap: 12px; flex: none; height: 36px; padding: 0 16px 0 20px; font-size: 12px; color: var(--muted); }
  .grow { flex-grow: 1; }
  .error { color: var(--text); }
  .pill { display: flex; align-items: center; gap: 6px; height: 26px; padding: 0 4px 0 10px; border: var(--border-w) solid var(--border); border-radius: 999px; background: var(--surface); box-shadow: var(--shadow-s); backdrop-filter: var(--blur); color: var(--text); }
  :global(.ib.xs) { width: 22px; height: 22px; border-radius: 999px; }
  .toast { position: fixed; left: 50%; bottom: 52px; z-index: 30; display: flex; align-items: center; gap: 12px; max-width: min(640px, calc(100% - 32px)); padding: 8px 8px 8px 16px; transform: translateX(-50%); border: var(--border-w) solid var(--border); border-radius: var(--radius); background: var(--surface-2); box-shadow: var(--shadow-pop); backdrop-filter: var(--blur); font-size: 13px; }
  .toast.error { border-color: #c42b1c; }
  .toast span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .toast-action { display: flex; align-items: center; gap: 8px; flex: none; height: 28px; padding: 0 10px; border: 0; border-radius: var(--radius-s); background: var(--sel); color: var(--sel-text); font-size: 13px; font-weight: 600; }
  .toast-action:hover { background: var(--hover); }
</style>
