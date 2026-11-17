<script lang="ts">
  // The Recycle Bin of every fixed drive, shown in Firewol instead of handed to Explorer.
  // Restore puts items back through Windows' own Restore; Delete permanently and Empty ask first.
  import { onDestroy, onMount } from "svelte";
  import { api } from "./api";
  import type { RecycledView } from "./contract";
  import { formatDate, formatSize, visibleName } from "./format";
  import Icon from "./Icon.svelte";
  import Sheet from "./Sheet.svelte";

  let { onactivate, onerror }: { onactivate: () => void; onerror: (message: string) => void } = $props();

  type Column = "name" | "from" | "deleted" | "size";
  let items = $state.raw<RecycledView[]>([]);
  let loaded = $state(false);
  let selected = $state.raw<Set<string>>(new Set());
  let anchor = $state<string | null>(null);
  let sort = $state<{ column: Column; descending: boolean }>({ column: "deleted", descending: true });
  let busy = $state(false);
  let ask = $state<{ kind: "delete" | "empty"; paths: string[] } | null>(null);
  let menu = $state<{ x: number; y: number } | null>(null);
  let list: HTMLElement | undefined = $state();

  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  const rows = $derived.by(() => {
    const order = sort.descending ? -1 : 1;
    const value = (item: RecycledView) => sort.column === "name" ? item.name : sort.column === "from" ? item.from : "";
    return [...items].sort((left, right) => {
      if (sort.column === "deleted") return order * (left.deleted - right.deleted);
      if (sort.column === "size") return order * (left.size - right.size);
      return order * collator.compare(value(left), value(right));
    });
  });
  const chosen = $derived(rows.filter((item) => selected.has(item.path)));

  export async function refresh() {
    try {
      const next = await api.recycleBinList();
      items = next;
      const present = new Set(next.map((item) => item.path));
      selected = new Set([...selected].filter((path) => present.has(path)));
    } catch (reason) {
      onerror(String(reason));
    } finally {
      loaded = true;
    }
  }

  export function focus() {
    list?.focus();
  }

  // The bin changes when anything on the PC deletes; a light re-read keeps the view honest.
  let timer: ReturnType<typeof setInterval> | undefined;
  onMount(() => {
    void refresh();
    timer = setInterval(() => { if (!busy && document.visibilityState === "visible") void refresh(); }, 5000);
  });
  onDestroy(() => clearInterval(timer));

  function choose(item: RecycledView, event: MouseEvent | KeyboardEvent) {
    onactivate();
    menu = null;
    if (event.shiftKey && anchor) {
      const from = rows.findIndex((row) => row.path === anchor);
      const to = rows.findIndex((row) => row.path === item.path);
      const [start, end] = from < to ? [from, to] : [to, from];
      selected = new Set(rows.slice(start, end + 1).map((row) => row.path));
      return;
    }
    if (event.ctrlKey) {
      const next = new Set(selected);
      if (next.has(item.path)) next.delete(item.path); else next.add(item.path);
      selected = next;
    } else {
      selected = new Set([item.path]);
    }
    anchor = item.path;
  }

  function openMenu(item: RecycledView | null, event: MouseEvent) {
    event.preventDefault();
    if (item && !selected.has(item.path)) choose(item, new MouseEvent("click"));
    menu = { x: event.clientX, y: event.clientY };
  }

  async function run(action: () => Promise<void>) {
    busy = true;
    menu = null;
    try {
      await action();
    } catch (reason) {
      onerror(String(reason));
    } finally {
      busy = false;
      await refresh();
    }
  }

  const restore = (paths: string[]) => run(() => api.recycleBinRestore(paths));

  function confirm() {
    const current = ask;
    ask = null;
    if (!current) return;
    void run(() => current.kind === "empty" ? api.recycleBinEmpty() : api.recycleBinDelete(current.paths));
  }

  function key(event: KeyboardEvent) {
    if (event.key === "Delete" && chosen.length) { event.preventDefault(); ask = { kind: "delete", paths: chosen.map((item) => item.path) }; }
    else if (event.key === "a" && event.ctrlKey) { event.preventDefault(); selected = new Set(rows.map((item) => item.path)); }
    else if (event.key === "Escape") { menu = null; selected = new Set(); }
    else if (event.key === "F5") { event.preventDefault(); void refresh(); }
  }

  function sortBy(column: Column) {
    sort = sort.column === column ? { column, descending: !sort.descending } : { column, descending: column === "deleted" || column === "size" };
  }

  const columns: [Column, string][] = [["name", "Name"], ["from", "Original location"], ["deleted", "Date deleted"], ["size", "Size"]];
</script>

<svelte:window onclick={() => (menu = null)} />

<section class="bin" aria-label="Recycle Bin">
  <div class="bar">
    <span class="count">{loaded ? `${items.length} ${items.length === 1 ? "item" : "items"}` : "Reading the Recycle Bin…"}</span>
    <button class="action" disabled={busy || !chosen.length} onclick={() => void restore(chosen.map((item) => item.path))}><Icon name="undo" /> Restore</button>
    <button class="action" disabled={busy || !chosen.length} onclick={() => (ask = { kind: "delete", paths: chosen.map((item) => item.path) })}><Icon name="trash" /> Delete permanently</button>
    <span class="grow"></span>
    <button class="action" disabled={busy || !items.length} onclick={() => void restore(items.map((item) => item.path))}>Restore all</button>
    <button class="action" disabled={busy || !items.length} onclick={() => (ask = { kind: "empty", paths: [] })}>Empty Recycle Bin</button>
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="table" role="grid" aria-multiselectable="true" tabindex="0" bind:this={list} onkeydown={key} oncontextmenu={(event) => openMenu(null, event)}>
    <div class="row head" role="row">
      {#each columns as [column, title] (column)}
        <button role="columnheader" class:sorted={sort.column === column} aria-sort={sort.column === column ? (sort.descending ? "descending" : "ascending") : "none"} onclick={() => sortBy(column)}>
          {title}{#if sort.column === column}<span class="arrow" class:down={sort.descending}><Icon name="sortUp" size={12} /></span>{/if}
        </button>
      {/each}
    </div>
    {#each rows as item (item.path)}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <div class="row" role="row" tabindex="-1" aria-selected={selected.has(item.path)} class:selected={selected.has(item.path)} title={`${item.from}\\${item.name}`} onclick={(event) => choose(item, event)} oncontextmenu={(event) => { event.stopPropagation(); openMenu(item, event); }}>
        <div role="gridcell" class="name"><Icon name={item.directory ? "open" : "file"} size={16} /><span>{visibleName(item.name)}</span></div>
        <div role="gridcell" class="dim">{item.from}</div>
        <div role="gridcell" class="dim">{formatDate(item.deleted)}</div>
        <div role="gridcell" class="dim size">{item.directory ? "" : formatSize(item.size)}</div>
      </div>
    {/each}
    {#if loaded && !items.length}<p class="empty">The Recycle Bin is empty.</p>{/if}
  </div>
</section>

{#if menu}
  <div class="menu" role="menu" style:left={`${menu.x}px`} style:top={`${menu.y}px`}>
    <button role="menuitem" disabled={!chosen.length} onclick={() => void restore(chosen.map((item) => item.path))}><Icon name="undo" size={16} /> Restore</button>
    <button role="menuitem" disabled={!chosen.length} onclick={() => { ask = { kind: "delete", paths: chosen.map((item) => item.path) }; menu = null; }}><Icon name="trash" size={16} /> Delete permanently</button>
    <hr />
    <button role="menuitem" disabled={!items.length} onclick={() => { ask = { kind: "empty", paths: [] }; menu = null; }}>Empty Recycle Bin</button>
    <button role="menuitem" onclick={() => { menu = null; void refresh(); }}><Icon name="refresh" size={16} /> Refresh</button>
  </div>
{/if}

{#if ask}
  {@const count = ask.kind === "empty" ? items.length : ask.paths.length}
  {@const single = ask.kind === "delete" && count === 1 ? items.find((item) => item.path === ask!.paths[0]) : undefined}
  <Sheet title={ask.kind === "empty" ? "Empty the Recycle Bin?" : single ? `Permanently delete “${visibleName(single.name)}”?` : `Permanently delete ${count} items?`} onclose={() => (ask = null)}>
    <p class="muted">{ask.kind === "empty" ? `All ${count} ${count === 1 ? "item" : "items"} are deleted for good.` : "This cannot be undone."} Restore is no longer possible afterwards.</p>
    <div class="actions">
      <button class="action" onclick={() => (ask = null)}>Cancel</button>
      <button class="action primary" onclick={confirm}>{ask.kind === "empty" ? "Empty Recycle Bin" : "Delete permanently"}</button>
    </div>
  </Sheet>
{/if}

<style>
  .bin { position: relative; display: grid; grid-template-rows: auto 1fr; min-height: 0; height: 100%; color: var(--text); }
  .bar { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--sep); }
  .bar .action { display: inline-flex; align-items: center; gap: 6px; }
  .count { color: var(--muted); margin-right: 8px; }
  .grow { flex: 1; }
  .table { overflow: auto; outline: none; min-height: 0; }
  .row { display: grid; grid-template-columns: minmax(200px, 2fr) minmax(160px, 2fr) 170px 100px; align-items: center; min-height: 32px; padding: 0 12px; gap: 12px; border-radius: var(--radius-s); }
  .row:not(.head):hover { background: var(--hover); }
  .row.selected { background: var(--sel); }
  .head { position: sticky; top: 0; z-index: 1; background: var(--surface); border-bottom: 1px solid var(--sep); border-radius: 0; }
  .head button { all: unset; display: inline-flex; align-items: center; gap: 4px; color: var(--muted); font-size: 12px; cursor: pointer; }
  .head button.sorted { color: var(--text); }
  .arrow.down { transform: rotate(180deg); display: inline-flex; }
  .name { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .name span, .dim { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dim { color: var(--muted); }
  .size { text-align: right; }
  .empty { color: var(--muted); padding: 24px; margin: 0; }
  .menu { position: fixed; z-index: 50; display: grid; min-width: 200px; padding: 4px; background: var(--surface-2); border: 1px solid var(--border); border-radius: var(--radius-s); box-shadow: var(--shadow-pop); }
  .menu button { all: unset; display: flex; align-items: center; gap: 8px; height: 28px; padding: 0 10px; border-radius: 6px; cursor: pointer; }
  .menu button:hover:not(:disabled) { background: var(--hover); }
  .menu button:disabled { color: var(--muted); cursor: default; }
  .menu hr { border: none; border-top: 1px solid var(--sep); margin: 4px 0; }
</style>
