<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount, tick } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { api } from "./lib/api";
  import {
    FOLDER_PROGRESS,
    INDEX_STATE,
    OPERATION_DONE,
    SESSION_CHANGED,
    SETTINGS_CHANGED,
    type SettingsView,
    type SettingsPatch,
    type AppearanceView,
    type Entry,
    type FolderProgress,
    type IndexState,
    type OperationDone,
    type SessionState,
    type Startup,
    type TransferStatus,
  } from "./lib/contract";
  import FileList from "./lib/FileList.svelte";
  import FolderIcon from "./lib/FolderIcon.svelte";
  import { formatCount, formatSize, useLocale } from "./lib/format";
  import Icon from "./lib/Icon.svelte";
  import { Pane } from "./lib/pane.svelte";
  import { PRESETS, colors, presetClass } from "./lib/appearance";
  import Sheet from "./lib/Sheet.svelte";

  const QUERIES = ["ré", "cord", "port", "résumé", "record-0000"];

  const appWindow = getCurrentWindow();
  let startup = $state<Startup>();
  let session = $state.raw<SessionState>({ pins: [], clipboard: null });
  let panes = $state<Pane[]>([]);
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
  let searchField: HTMLInputElement;
  let addressField: HTMLInputElement | undefined = $state();
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let transferTimer: ReturnType<typeof setInterval> | undefined;

  const active = $derived(panes[activeIndex] ?? panes[0]);
  const other = $derived(panes.length > 1 ? panes[activeIndex === 0 ? 1 : 0] : undefined);
  const title = $derived(active?.folder?.segments.at(-1)?.name ?? "Firewol");
  const focused = $derived(active?.focusedEntry);
  const selectedCount = $derived(active?.selection.size ?? 0);
  const pinned = $derived(session.pins.some((pin) => pin.path === active?.folder?.path));

  $effect(() => {
    document.title = `${title} – Firewol`;
  });

  function stored(key: string, fallback: string) {
    try {
      return localStorage.getItem(`firewol.${key}`) ?? fallback;
    } catch {
      return fallback;
    }
  }

  async function saveSettings(patch: SettingsPatch) {
    try { settings = await api.updateSettings(patch); }
    catch (reason) { notify(String(reason), { error: true }); }
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

  function open(entry: Entry, newWindow: boolean) {
    menu = undefined;
    if (!entry.directory) return;
    if (newWindow) void api.openWindow(entry.path, false);
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
    if (panes.length > 1) {
      panes[1].close();
      panes = [panes[0]];
      lists = [lists[0]];
      activeIndex = 0;
      return;
    }
    const pane = newPane();
    panes = [...panes, pane];
    if (active?.folder) await pane.open(active.folder.path);
  }

  function newWindow() {
    menu = undefined;
    if (active?.folder) void api.openWindow(active.folder.path, false);
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
    const key = event.key.toLowerCase();
    const typing = event.target instanceof HTMLInputElement;
    if (event.key === "Escape" && menu) menu = undefined;
    else if (event.key === "Escape" && editing) editing = false;
    else if (event.ctrlKey && key === "l") editAddress();
    else if (event.ctrlKey && key === "f") searchField.focus();
    else if (event.ctrlKey && key === "n") newWindow();
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
        if (!main.results?.items.length) throw new Error("Expected search results in the fixture");
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
    }
    await api.saveReport(report);
    if (info.extraWindows > 0) {
      await sleep(info.idleMs);
      if (panes.length === 1) await toggleSplit();
      await painted();
      for (let window = 0; window < info.extraWindows; window++) await api.openWindow(info.path, true);
    }
  }

  /**
   * Drives the real interface functions against a fixture with `source\note.txt` and an
   * empty `dest` folder next to it, then saves each step's result.
   */
  async function selfTest(info: Startup) {
    const steps: { step: string; ok: boolean; detail?: string }[] = [];
    const check = (step: string, ok: boolean, detail?: string) => {
      steps.push({ step, ok, detail });
      if (!ok) throw new Error(`${step}: ${detail ?? "failed"}`);
    };
    const names = (pane: Pane) => (pane.pages[0] ?? []).map((entry) => entry.name);
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
    } catch (reason) {
      if (steps.every((step) => step.ok)) steps.push({ step: "error", ok: false, detail: String(reason) });
    }
    await api.saveReport({ steps, passed: steps.every((step) => step.ok) });
  }

  onMount(() => {
    const unlisten = [
      listen<FolderProgress>(FOLDER_PROGRESS, (event) => panes.forEach((pane) => pane.progress(event.payload))),
      listen<IndexState>(INDEX_STATE, (event) => panes.forEach((pane) => pane.indexed(event.payload))),
      listen<OperationDone>(OPERATION_DONE, (event) => finished(event.payload)),
      listen<SessionState>(SESSION_CHANGED, (event) => (session = event.payload)),
      listen<SettingsView>(SETTINGS_CHANGED, (event) => (settings = event.payload)),
      appWindow.onCloseRequested(async (event) => {
        if (allowClose) return;
        event.preventDefault();
        if (await api.operationBusy()) closeWarning = true;
        else { allowClose = true; await appWindow.close(); }
      }),
    ];
    void (async () => {
      const info = await api.startup();
      await loadSettings();
      const start = performance.now();
      useLocale(info.locale);
      startup = info;
      session = info.session;
      panes = info.split ? [newPane(), newPane()] : [newPane()];
      await Promise.all(panes.map((pane) => pane.open(info.path)));
      await painted();
      const firstListMs = info.sinceStartMs + performance.now() - start;
      lists[0]?.focus();
      if (info.selfTest) await selfTest(info);
      else if (info.report) {
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
      <div class="tabs" role="tablist" aria-label="Folder tabs">
        <div class="tab on">
          <button class="tl" role="tab" aria-selected="true">
            <FolderIcon />
            <span>{title}</span>
          </button>
        </div>
      </div>
      <button class="ib" aria-label="New window (Ctrl+N)" title="New window (Ctrl+N)" onclick={newWindow}><Icon name="plus" /></button>
      <div class="drag" data-tauri-drag-region></div>
      <div class="caption {settings?.appearance.caption_shape ?? 'rounded'}" class:left={settings?.appearance.caption_left}>
        <button class="cap" aria-label="Minimize" onclick={() => appWindow.minimize()}><Icon name="minimize" size={14} weight={1.5} /></button>
        <button class="cap" aria-label="Maximize" onclick={() => appWindow.toggleMaximize()}><Icon name="maximize" size={14} weight={1.5} /></button>
        <button class="cap close" aria-label="Close" onclick={() => appWindow.close()}><Icon name="close" size={14} weight={1.5} /></button>
      </div>
    </header>

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
      <label class="field search">
        <span class="sr">Search {title}</span>
        <span class="muted icon"><Icon name="search" /></span>
        <input
          bind:this={searchField}
          type="search"
          placeholder={`Search ${title}`}
          value={active?.query ?? ""}
          oninput={(event) => active?.search(event.currentTarget.value)}
        />
        <kbd class="kbd">Ctrl F</kbd>
      </label>
      <div class="seg" role="group" aria-label="View">
        <button class="ib on" aria-pressed="true" aria-label="Details view" title="Details view"><Icon name="list" /></button>
        <button class="ib" aria-pressed="false" aria-label="Grid view (not in this build)" title="Grid view (not in this build)" disabled><Icon name="grid" /></button>
      </div>
      <button class="ib" class:on={panes.length > 1} aria-pressed={panes.length > 1} aria-label="Split view (Ctrl+\)" title="Split view (Ctrl+\)" onclick={toggleSplit}><Icon name="split" /></button>
      <button class="ib" aria-label="More" title="More" aria-haspopup="menu" onmousedown={(event) => event.stopPropagation()} onclick={(event) => showMenu("more", event.clientX - 220, event.clientY + 16)}><Icon name="more" /></button>
    </div>

    <div class="body">
      {#if sidebar}
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
            <button class="pin" class:on={pin.path === active?.folder?.path} onclick={() => active?.open(pin.path)}>
              <FolderIcon /><span>{pin.name}</span>
            </button>
          {:else}
            <p class="muted side-empty">Pin folders you use often. They stay here until you unpin them.</p>
          {/each}
        </nav>
      {/if}
      <div class="panes">
        {#each panes as pane, index (pane)}
          <FileList
            bind:this={lists[index]}
            {pane}
            id={`pane-${index}`}
            marked={panes.length > 1 && index === activeIndex}
            onactivate={() => (activeIndex = index)}
            onopen={open}
            onmenu={(x, y, onRow) => showMenu(onRow ? "item" : "area", x, y)}
            onrename={rename}
            onrecycle={recycle}
          />
        {/each}
      </div>
    </div>

    <footer class="status" role="status">
      {#if fatal || active?.error}
        <span class="error">{fatal || active?.error}</span>
      {:else if active}
        <span>{formatCount(active.rows)} {active.searching ? (active.rows === 1 ? "result" : "results") : active.rows === 1 ? "item" : "items"}{active.complete ? "" : " · reading…"}{selectedCount > 0 ? ` · ${selectedCount} selected` : ""}</span>
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
      {#if active?.index === "building"}
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
      {#if menu.kind === "item"}
        <button class="mi" role="menuitem" disabled={!focused?.directory} onclick={() => focused && open(focused, false)}><Icon name="open" /><span class="label">Open</span><span class="sc">Enter</span></button>
        <button class="mi" role="menuitem" disabled={!focused?.directory} onclick={() => focused && open(focused, true)}><Icon name="window" /><span class="label">Open in new window</span><span class="sc">Ctrl+Enter</span></button>
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={() => clip(true)}><Icon name="cut" /><span class="label">Cut</span><span class="sc">Ctrl+X</span></button>
        <button class="mi" role="menuitem" onclick={() => clip(false)}><Icon name="copy" /><span class="label">Copy</span><span class="sc">Ctrl+C</span></button>
        {#if other?.folder?.item}
          <button class="mi" role="menuitem" onclick={() => toOtherPane(false)}><Icon name="toPane" /><span class="label">Copy to other pane</span></button>
          <button class="mi" role="menuitem" onclick={() => toOtherPane(true)}><Icon name="transfer" /><span class="label">Move to other pane</span></button>
        {/if}
        <div class="sep"></div>
        <button class="mi" role="menuitem" disabled={selectedCount !== 1} onclick={() => { menu = undefined; if (active) active.renaming = active.selected; }}><Icon name="rename" /><span class="label">Rename</span><span class="sc">F2</span></button>
        <button class="mi" role="menuitem" disabled={selectedCount !== 1} onclick={recycle}><Icon name="trash" /><span class="label">Move to Recycle Bin</span><span class="sc">Del</span></button>
        <div class="sep"></div>
        {#if focused?.directory && selectedCount === 1}
          <button class="mi" role="menuitem" onclick={() => togglePin(focused?.path)}><Icon name="pin" /><span class="label">{session.pins.some((pin) => pin.path === focused?.path) ? "Unpin from sidebar" : "Pin to sidebar"}</span></button>
        {/if}
        <button class="mi" role="menuitem" onclick={() => { if (focused) void navigator.clipboard.writeText(focused.path); menu = undefined; }}><Icon name="copy" /><span class="label">Copy path</span></button>
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={undo}><Icon name="undo" /><span class="label">Undo</span><span class="sc">Ctrl+Z</span></button>
        <button class="mi" role="menuitem" onclick={() => { menu = undefined; void active?.refresh(); }}><Icon name="refresh" /><span class="label">Refresh</span><span class="sc">F5</span></button>
      {:else if menu.kind === "area"}
        <button class="mi" role="menuitem" disabled={!session.clipboard} onclick={paste}><Icon name="paste" /><span class="label">Paste{session.clipboard ? ` ${session.clipboard.count} item${session.clipboard.count === 1 ? "" : "s"}` : ""}</span><span class="sc">Ctrl+V</span></button>
        <button class="mi" role="menuitem" onclick={() => togglePin(active?.folder?.path)}><Icon name="pin" /><span class="label">{pinned ? "Unpin this folder" : "Pin this folder"}</span></button>
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={undo}><Icon name="undo" /><span class="label">Undo</span><span class="sc">Ctrl+Z</span></button>
        <button class="mi" role="menuitem" onclick={() => { menu = undefined; void active?.refresh(); }}><Icon name="refresh" /><span class="label">Refresh</span><span class="sc">F5</span></button>
      {:else}
        <div class="menu-title">Theme</div>
        {#each PRESETS as [value, css, name] (value)}
          <button class="mi" class:on={preset === css} role="menuitemradio" aria-checked={preset === css} onclick={() => choosePreset(value)}><Icon name="palette" /><span class="label">{name}</span></button>
        {/each}
        {#if preset === "light" || preset === "neu"}<button class="mi" role="menuitemcheckbox" aria-checked={dark ?? false} onclick={toggleDark}><Icon name="check" /><span class="label">Dark variant</span><span class="sc">{dark ? "On" : "Off"}</span></button>{/if}
        <div class="sep"></div>
        <button class="mi" role="menuitem" onclick={newWindow}><Icon name="window" /><span class="label">New window</span><span class="sc">Ctrl+N</span></button>
        <button class="mi" role="menuitem" onclick={toggleSplit}><Icon name="split" /><span class="label">{panes.length > 1 ? "Single pane" : "Split view"}</span><span class="sc">Ctrl+\</span></button>
      {/if}
    </div>
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
  .tabs { display: flex; align-items: center; gap: 4px; min-width: 0; }
  .tab { display: flex; align-items: center; height: 32px; max-width: 240px; padding: 0 4px 0 2px; border: var(--border-w) solid transparent; border-radius: var(--radius-s); color: var(--muted); }
  .tab.on { background: var(--tab-active); color: var(--text); border-color: var(--border); box-shadow: var(--shadow-s); backdrop-filter: var(--blur); }
  .tl { display: flex; align-items: center; gap: 8px; min-width: 0; height: 100%; padding: 0 8px; border: 0; background: transparent; font-size: 12px; }
  .tl span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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
  .search { flex: 0 1 280px; min-width: 180px; margin: 0 6px; }
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
