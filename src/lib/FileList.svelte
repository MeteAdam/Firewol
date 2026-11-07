<script lang="ts">
  import { untrack } from "svelte";
  import type { Entry } from "./contract";
  import FolderIcon from "./FolderIcon.svelte";
  import { formatDate, formatSize, location, typeName } from "./format";
  import Icon from "./Icon.svelte";
  import { PAGE, type Pane } from "./pane.svelte";

  const ROW = 32;
  const HEADER = 32;
  const OVERSCAN = 6;

  let {
    pane,
    id,
    marked,
    onactivate,
    onopen,
    onmenu,
  }: {
    pane: Pane;
    id: string;
    marked: boolean;
    onactivate: () => void;
    onopen: (entry: Entry, newWindow: boolean) => void;
    onmenu: (x: number, y: number) => void;
  } = $props();

  // The grid is its own scroll container so assistive technology gets a scrollable table.
  let grid: HTMLDivElement;
  let height = $state(0);
  let scroll = $state(0);
  const rows = $derived(pane.rows);
  const top = $derived(Math.floor(scroll / ROW));
  const first = $derived(Math.max(0, top - OVERSCAN));
  const end = $derived(Math.min(rows, top + Math.ceil(height / ROW) + OVERSCAN));
  const visible = $derived(Array.from({ length: Math.max(0, end - first) }, (_, index) => first + index));
  const activeRow = $derived(
    pane.selected >= first && pane.selected < end ? `${id}-row-${pane.selected}` : undefined,
  );

  $effect(() => {
    if (pane.searching) return;
    const start = Math.floor(first / PAGE) * PAGE;
    const stop = end;
    untrack(() => {
      for (let offset = start; offset < stop; offset += PAGE) void pane.loadPage(offset);
    });
  });

  $effect(() => {
    void pane.folder?.id;
    void pane.results;
    untrack(() => {
      grid.scrollTop = 0;
      scroll = 0;
    });
  });

  function select(row: number) {
    if (rows === 0) return;
    pane.selected = Math.max(0, Math.min(rows - 1, row));
    const position = HEADER + pane.selected * ROW;
    if (position < grid.scrollTop + HEADER) grid.scrollTop = position - HEADER;
    else if (position + ROW > grid.scrollTop + height) grid.scrollTop = position + ROW - height;
  }

  function keydown(event: KeyboardEvent) {
    const step = Math.max(1, Math.floor((height - HEADER) / ROW) - 1);
    const moves: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, PageDown: step, PageUp: -step };
    const entry = pane.entry(pane.selected);
    if (event.key in moves && !event.altKey) select(pane.selected + moves[event.key]);
    else if (event.key === "Home") select(0);
    else if (event.key === "End") select(rows - 1);
    else if (event.key === "Enter" && entry) onopen(entry, event.ctrlKey);
    else if (event.key === "Backspace") void pane.up();
    else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      const bounds = grid.getBoundingClientRect();
      onmenu(bounds.left + 48, bounds.top + HEADER + (pane.selected + 1) * ROW - grid.scrollTop);
    } else return;
    event.preventDefault();
  }

  function context(event: MouseEvent, row: number) {
    event.preventDefault();
    select(row);
    onmenu(event.clientX, event.clientY);
  }

  const frame = () => new Promise<number>((resolve) => requestAnimationFrame(resolve));

  /** Scrolls at 10 px per millisecond and returns the frame rate. */
  export async function benchmarkScroll(duration: number) {
    const start = await frame();
    let now = start;
    let frames = 0;
    while (now - start < duration) {
      grid.scrollTop = Math.min(rows * ROW, (now - start) * 10);
      now = await frame();
      frames++;
    }
    return (1000 * frames) / (now - start);
  }

  /** Scrolls to the end and waits until the last row shows its name. */
  export async function verifyLast() {
    grid.scrollTop = HEADER + rows * ROW;
    const last = rows - 1;
    for (let attempt = 0; attempt < 600; attempt++) {
      await frame();
      const entry = pane.entry(last);
      const cell = grid.querySelector(`[aria-rowindex="${last + 2}"] .nm`);
      if (entry && cell?.textContent === entry.name) return true;
    }
    return false;
  }

  export function focus() {
    grid.focus();
  }
</script>

<section class="list" class:marked class:searching={pane.searching} aria-label={pane.folder?.path ?? "Folder"}>
  <div
    class="grid"
    role="grid"
    tabindex="0"
    aria-rowcount={rows + 1}
    aria-colcount={4}
    aria-activedescendant={activeRow}
    aria-label={pane.searching ? "Search results" : "Files"}
    bind:this={grid}
    bind:clientHeight={height}
    onscroll={() => (scroll = grid.scrollTop)}
    onkeydown={keydown}
    onfocusin={onactivate}
    onmousedown={onactivate}
  >
    <div class="cols hdr" role="row" aria-rowindex="1">
      <div role="columnheader" aria-sort="ascending" class="sort">Name <Icon name="sortUp" size={12} weight={2} /></div>
      {#if pane.searching}<div role="columnheader">Location</div>{/if}
      <div role="columnheader">Date modified</div>
      {#if !pane.searching}<div role="columnheader">Type</div>{/if}
      <div role="columnheader" class="size">Size</div>
    </div>
    <div class="spacer" role="rowgroup" style:height={`${rows * ROW}px`}>
      <div class="rows" style:transform={`translateY(${first * ROW}px)`}>
        <!-- Keyed by row so scrolling only adds and removes edge rows, which keeps
             accessibility events proportional to what actually enters the view. -->
        {#each visible as row (row)}
          {@const entry = pane.entry(row)}
          <!-- svelte-ignore a11y_interactive_supports_focus -->
          <div
            class="cols row"
            class:sel={row === pane.selected}
            role="row"
            id={`${id}-row-${row}`}
            aria-rowindex={row + 2}
            aria-selected={row === pane.selected}
            onmousedown={() => select(row)}
            ondblclick={() => entry && onopen(entry, false)}
            oncontextmenu={(event) => context(event, row)}
          >
            {#if entry}
              <div role="gridcell" class="name">
                {#if entry.directory}<FolderIcon />{:else}<span class="ficon"><Icon name="file" /></span>{/if}
                <span class="nm">{entry.name}</span>
              </div>
              {#if pane.searching}<div role="gridcell" class="dim">{location(entry)}</div>{/if}
              <div role="gridcell" class="dim">{formatDate(entry.modified)}</div>
              {#if !pane.searching}<div role="gridcell" class="dim">{typeName(entry)}</div>{/if}
              <div role="gridcell" class="dim size">{formatSize(entry.size)}</div>
            {:else}
              <div role="gridcell" class="placeholder"></div>
            {/if}
          </div>
        {/each}
      </div>
    </div>
    {#if rows === 0}
      <p class="empty">
        {#if pane.searching && pane.index !== "ready"}Indexing this folder…
        {:else if pane.searching}No matching items.
        {:else if pane.complete}This folder is empty.
        {:else}Reading folder…{/if}
      </p>
    {/if}
  </div>
</section>

<style>
  .list {
    display: flex;
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
    background: var(--surface);
    border: var(--border-w) solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    backdrop-filter: var(--blur);
    outline: var(--stitch);
    outline-offset: -6px;
    overflow: hidden;
  }
  .list.marked { outline: 2px solid var(--accent); outline-offset: -2px; }
  .grid { position: relative; flex: 1; min-width: 0; min-height: 0; padding: 0 6px 6px; overflow: auto; overflow-anchor: none; }
  .grid:focus-visible { outline: none; }
  .grid:focus-visible .row.sel { outline: 2px solid var(--accent); outline-offset: -2px; }
  .cols {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 150px 128px 84px;
    column-gap: 12px;
    align-items: center;
    height: 32px;
    padding: 0 10px;
    border-radius: var(--radius-s);
  }
  .searching .cols { grid-template-columns: minmax(0, 1fr) minmax(0, .8fr) 150px 84px; }
  .hdr {
    position: sticky;
    top: 0;
    z-index: 1;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
    background: var(--surface);
    backdrop-filter: var(--blur);
    border-bottom: 1px solid var(--sep);
    border-radius: 0;
  }
  .sort { display: flex; align-items: center; gap: 4px; }
  .row:hover { background: var(--hover); }
  .row.sel { background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); }
  .name { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .nm, .dim { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dim { color: var(--muted); }
  .row.sel .dim { color: inherit; opacity: .85; }
  .size { text-align: right; }
  .ficon { display: flex; flex: none; color: var(--muted); }
  .placeholder { height: 8px; width: 40%; border-radius: 4px; background: var(--sep); }
  .empty { position: absolute; inset: 56px auto auto 24px; margin: 0; color: var(--muted); }
</style>
