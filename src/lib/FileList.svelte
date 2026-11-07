<script lang="ts">
  import { untrack } from "svelte";
  import type { Entry } from "./contract";
  import FolderIcon from "./FolderIcon.svelte";
  import FileIcon from "./FileIcon.svelte";
  import { formatDate, formatSize, location, typeName } from "./format";
  import Icon from "./Icon.svelte";
  import { PAGE, type Pane } from "./pane.svelte";
  import { tint } from "./styles";

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
    onrename,
    onrecycle,
  }: {
    pane: Pane;
    id: string;
    marked: boolean;
    onactivate: () => void;
    onopen: (entry: Entry, newWindow: boolean) => void;
    /** `onRow` is false for the empty area, which acts on the current folder. */
    onmenu: (x: number, y: number, onRow: boolean) => void;
    onrename: (entry: Entry, name: string) => void;
    onrecycle: () => void;
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
  const tagged = $derived(pane.tagged);
  const shownName = (entry: Entry) => entry.style?.alias || entry.name;
  const activeRow = $derived(
    pane.selected >= first && pane.selected < end ? `${id}-row-${pane.selected}` : undefined,
  );

  $effect(() => {
    void pane.pageVersion;
    void pane.searchVersion;
    void pane.searching;
    void pane.searchReady;
    const start = Math.floor(first / PAGE) * PAGE;
    const stop = end;
    untrack(() => {
      for (let offset = start; offset < stop; offset += PAGE) void pane.loadPage(offset);
    });
  });

  // A new folder or new results start at the top; reloading the same folder keeps the position.
  $effect(() => {
    void pane.folder?.path;
    void pane.searchVersion;
    untrack(() => {
      grid.scrollTop = pane.scroll;
      scroll = pane.scroll;
    });
  });

  function reveal(row: number) {
    const position = HEADER + row * ROW;
    if (position < grid.scrollTop + HEADER) grid.scrollTop = position - HEADER;
    else if (position + ROW > grid.scrollTop + height) grid.scrollTop = position + ROW - height;
  }

  async function move(row: number, extend: boolean) {
    await pane.select(row, extend ? "range" : "single");
    reveal(pane.selected);
  }

  function keydown(event: KeyboardEvent) {
    if (event.target instanceof HTMLInputElement) return;
    const step = Math.max(1, Math.floor((height - HEADER) / ROW) - 1);
    const moves: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, PageDown: step, PageUp: -step };
    const entry = pane.focusedEntry;
    if (event.key in moves && !event.altKey) move(pane.selected + moves[event.key], event.shiftKey);
    else if (event.key === "Home") move(0, event.shiftKey);
    else if (event.key === "End") move(rows - 1, event.shiftKey);
    else if (event.key === "Enter" && entry) onopen(entry, event.ctrlKey);
    else if (event.key === "Backspace") void pane.up();
    else if (event.key === "F2" && entry?.id) pane.renaming = pane.selected;
    else if (event.key === "Delete" && entry?.id) onrecycle();
    else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      const bounds = grid.getBoundingClientRect();
      onmenu(bounds.left + 48, bounds.top + HEADER + (pane.selected + 1) * ROW - grid.scrollTop, entry !== undefined);
    } else return;
    event.preventDefault();
  }

  function press(event: MouseEvent, row: number) {
    if (event.button !== 0) return;
    pane.select(row, event.ctrlKey ? "toggle" : event.shiftKey ? "range" : "single");
  }

  function rowMenu(event: MouseEvent, row: number) {
    event.preventDefault();
    event.stopPropagation();
    if (!pane.isSelected(row)) void pane.select(row);
    else pane.selected = row;
    onmenu(event.clientX, event.clientY, true);
  }

  function areaMenu(event: MouseEvent) {
    event.preventDefault();
    onmenu(event.clientX, event.clientY, false);
  }

  /** Focuses the rename field and selects the name without its extension. */
  function renameField(node: HTMLInputElement, directory: boolean) {
    node.focus();
    const dot = node.value.lastIndexOf(".");
    node.setSelectionRange(0, dot > 0 && !directory ? dot : node.value.length);
  }

  function finishRename(entry: Entry, value: string | null) {
    if (pane.renaming === null) return;
    pane.renaming = null;
    const name = value?.trim();
    if (name && name !== entry.name) onrename(entry, name);
    grid.focus();
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
      if (entry && cell?.textContent === shownName(entry)) return true;
    }
    return false;
  }

  export function focus() {
    grid.focus();
  }
</script>

<section class="list" class:marked class:searching={pane.searching} class:tagged aria-label={pane.folder?.path ?? "Folder"}>
  <div
    class="grid"
    role="grid"
    tabindex="0"
    aria-multiselectable="true"
    aria-rowcount={rows + 1}
    aria-colcount={tagged ? 5 : 4}
    aria-activedescendant={activeRow}
    aria-label={pane.searching ? "Search results" : "Files"}
    bind:this={grid}
    bind:clientHeight={height}
    onscroll={() => { scroll = grid.scrollTop; pane.scroll = scroll; }}
    onkeydown={keydown}
    onfocusin={onactivate}
    onmousedown={onactivate}
    oncontextmenu={areaMenu}
  >
    <div class="cols hdr" role="row" aria-rowindex="1">
      <div role="columnheader" aria-sort={pane.sort.column === "name" ? (pane.sort.descending ? "descending" : "ascending") : "none"}><button class="sort" disabled={pane.searching} onclick={() => pane.sortBy("name")}>Name {#if pane.sort.column === "name"}<span class:descending={pane.sort.descending}><Icon name="sortUp" size={12} weight={2} /></span>{/if}</button></div>
      {#if pane.searching}<div role="columnheader">Location</div>{/if}
      <div role="columnheader" aria-sort={pane.sort.column === "modified" ? (pane.sort.descending ? "descending" : "ascending") : "none"}><button class="sort" disabled={pane.searching} onclick={() => pane.sortBy("modified")}>Date modified</button></div>
      {#if !pane.searching}<div role="columnheader" aria-sort={pane.sort.column === "type" ? (pane.sort.descending ? "descending" : "ascending") : "none"}><button class="sort" onclick={() => pane.sortBy("type")}>Type</button></div>{/if}
      <div role="columnheader" class="size" aria-sort={pane.sort.column === "size" ? (pane.sort.descending ? "descending" : "ascending") : "none"}><button class="sort" disabled={pane.searching} onclick={() => pane.sortBy("size")}>Size</button></div>
      {#if tagged}<div role="columnheader" class="tags-head">Tags</div>{/if}
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
            class:sel={pane.isSelected(row)}
            class:focus={row === pane.selected}
            class:hidden={entry?.hidden}
            role="row"
            id={`${id}-row-${row}`}
            aria-rowindex={row + 2}
            aria-selected={pane.isSelected(row)}
            onmousedown={(event) => press(event, row)}
            ondblclick={() => entry && onopen(entry, false)}
            oncontextmenu={(event) => rowMenu(event, row)}
          >
            {#if entry}
              <div role="gridcell" class="name">
                {#if entry.directory}<FolderIcon color={tint(entry.style)} glyph={entry.style?.icon} />{:else}<FileIcon icon={entry.icon} />{/if}
                {#if row === pane.renaming}
                  <input
                    class="rename"
                    aria-label="New name"
                    value={entry.name}
                    spellcheck="false"
                    use:renameField={entry.directory}
                    onmousedown={(event) => event.stopPropagation()}
                    onkeydown={(event) => {
                      if (event.key === "Enter") finishRename(entry, event.currentTarget.value);
                      else if (event.key === "Escape") finishRename(entry, null);
                      else return;
                      event.preventDefault();
                    }}
                    onblur={(event) => finishRename(entry, event.currentTarget.value)}
                  />
                {:else}
                  <span class="nm" title={entry.style?.alias ? entry.name : undefined}>{shownName(entry)}</span>
                {/if}
              </div>
              {#if pane.searching}<div role="gridcell" class="dim">{location(entry)}</div>{/if}
              <div role="gridcell" class="dim">{formatDate(entry.modified)}</div>
              {#if !pane.searching}<div role="gridcell" class="dim">{typeName(entry)}</div>{/if}
              <div role="gridcell" class="dim size">{formatSize(entry.size)}</div>
              {#if tagged}<div role="gridcell" class="tags">{#each entry.style?.tags ?? [] as tag (tag)}<span class="tag">{tag}</span>{/each}</div>{/if}
            {:else}
              <div role="gridcell" class="placeholder"></div>
            {/if}
          </div>
        {/each}
      </div>
    </div>
    {#if rows === 0}
      <p class="empty">
        {#if pane.searching && pane.searchError}{pane.searchError}
        {:else if pane.searching && !pane.searchReady}{pane.volume ? `Indexing ${pane.volume}: — results appear when the first pass finishes.` : "Indexing this folder…"}
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
  .grid:focus-visible .row.focus { outline: 2px solid var(--accent); outline-offset: -2px; }
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
  .tagged .cols { grid-template-columns: minmax(0, 1fr) 150px 128px 84px 168px; }
  .searching.tagged .cols { grid-template-columns: minmax(0, 1fr) minmax(0, .8fr) 150px 84px 168px; }
  .tags-head { padding-left: 4px; }
  .tags { display: flex; gap: 4px; min-width: 0; overflow: hidden; }
  .tag { flex: 0 1 auto; min-width: 0; padding: 1px 8px; border-radius: 999px; background: color-mix(in srgb, var(--accent) 16%, transparent); font-size: 11px; line-height: 16px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row.sel .tag { background: color-mix(in srgb, currentColor 18%, transparent); }
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
  .sort { display: flex; align-items: center; gap: 4px; padding: 0; border: 0; background: transparent; font: inherit; }
  .descending { transform: rotate(180deg); }
  .row:hover { background: var(--hover); }
  .row.sel { background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); }
  .row.hidden .name { opacity: .6; }
  .name { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .nm, .dim { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dim { color: var(--muted); }
  .row.sel .dim { color: inherit; opacity: .85; }
  .size { text-align: right; }
  .rename {
    flex: 1;
    min-width: 0;
    height: 24px;
    margin-left: -4px;
    padding: 0 4px;
    border: 1px solid var(--accent);
    border-radius: 4px;
    background: var(--field);
    color: var(--text);
    font-size: 13px;
    outline: none;
    user-select: text;
  }
  .placeholder { height: 8px; width: 40%; border-radius: 4px; background: var(--sep); }
  .empty { position: absolute; inset: 56px 24px auto 24px; margin: 0; color: var(--muted); white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
