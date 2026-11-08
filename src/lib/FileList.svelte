<script lang="ts">
  import { untrack } from "svelte";
  import type { Entry } from "./contract";
  import FolderIcon from "./FolderIcon.svelte";
  import FileIcon from "./FileIcon.svelte";
  import { formatDate, formatSize, location, typeName } from "./format";
  import { edgeScroll } from "./autoscroll";
  import Icon from "./Icon.svelte";
  import { PAGE, type Pane } from "./pane.svelte";
  import { tint } from "./styles";

  const HEADER = 32;
  const OVERSCAN = 6;
  // Grid view: tiles sit in lines of `columns`, with this gap and top padding.
  const GAP = 8;
  const PAD = 8;

  let {
    pane,
    id,
    marked,
    grid: tiles = false,
    tile = 128,
    row: ROW = 32,
    onactivate,
    onopen,
    onmenu,
    onrename,
    onrecycle,
    ondrag,
  }: {
    pane: Pane;
    id: string;
    marked: boolean;
    /** Shows tiles of `tile` pixels instead of the details table. */
    grid?: boolean;
    tile?: number;
    /** Details row height: 24, 32, 40 or 52 pixels. */
    row?: number;
    onactivate: () => void;
    onopen: (entry: Entry, newWindow: boolean) => void;
    /** `onRow` is false for the empty area, which acts on the current folder. */
    onmenu: (x: number, y: number, onRow: boolean) => void;
    onrename: (entry: Entry, name: string) => void;
    onrecycle: () => void;
    /** A pressed row moved far enough to drag the selection; the window takes over. */
    ondrag?: (event: PointerEvent) => void;
  } = $props();

  // The grid is its own scroll container so assistive technology gets a scrollable table.
  let grid: HTMLDivElement;
  let height = $state(0);
  let width = $state(0);
  let scroll = $state(0);
  const rows = $derived(pane.rows);
  // Details view is the one-column case: a line is a row.
  const columns = $derived(tiles ? Math.max(1, Math.floor((width - 12 - 2 * PAD + GAP) / (tile + GAP))) : 1);
  const line = $derived(tiles ? tile + GAP : ROW);
  const head = $derived(tiles ? PAD : HEADER);
  const lines = $derived(Math.ceil(rows / columns));
  const top = $derived(Math.floor(Math.max(0, scroll - head) / line));
  const firstLine = $derived(Math.max(0, top - (tiles ? 2 : OVERSCAN)));
  const endLine = $derived(Math.min(lines, top + Math.ceil(height / line) + (tiles ? 2 : OVERSCAN)));
  const first = $derived(firstLine * columns);
  const end = $derived(Math.min(rows, endLine * columns));
  const visible = $derived(Array.from({ length: Math.max(0, end - first) }, (_, index) => first + index));
  const visibleLines = $derived(Array.from({ length: Math.max(0, endLine - firstLine) }, (_, index) => firstLine + index));
  const iconSize = $derived(Math.round(tile / 2));
  const rowIcon = $derived(ROW <= 32 ? 16 : ROW === 40 ? 24 : 32);
  const rowFont = $derived(ROW === 24 ? 12 : ROW === 32 ? 13 : ROW === 40 ? 14 : 15);
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
    const position = head + Math.floor(row / columns) * line;
    if (position < grid.scrollTop + head) grid.scrollTop = position - head;
    else if (position + line > grid.scrollTop + height) grid.scrollTop = position + line - height;
  }

  async function move(row: number, extend: boolean) {
    await pane.select(row, extend ? "range" : "single");
    reveal(pane.selected);
  }

  function keydown(event: KeyboardEvent) {
    if (event.target instanceof HTMLInputElement) return;
    const step = Math.max(1, Math.floor((height - head) / line) - 1) * columns;
    const moves: Record<string, number> = { ArrowDown: columns, ArrowUp: -columns, PageDown: step, PageUp: -step };
    if (tiles) Object.assign(moves, { ArrowRight: 1, ArrowLeft: -1 });
    const entry = pane.focusedEntry;
    // Up and down from a partial last line stay in their column, or stop at the last item.
    if (event.key in moves && !event.altKey) move(Math.max(0, Math.min(rows - 1, pane.selected + moves[event.key])), event.shiftKey);
    else if (event.key === "Home") move(0, event.shiftKey);
    else if (event.key === "End") move(rows - 1, event.shiftKey);
    else if (event.key === "Enter" && entry) onopen(entry, event.ctrlKey);
    else if (event.key === "Backspace") void pane.up();
    else if (event.key === "F2" && entry?.id) pane.renaming = pane.selected;
    else if (event.key === "Delete" && pane.selection.size > 0) onrecycle();
    else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      const bounds = grid.getBoundingClientRect();
      const column = pane.selected % columns;
      onmenu(bounds.left + PAD + column * (tile + GAP) + (tiles ? tile / 2 : 48), bounds.top + head + (Math.floor(pane.selected / columns) + 1) * line - grid.scrollTop, entry !== undefined);
    } else return;
    event.preventDefault();
  }

  // A plain press on a selected row keeps the selection, so it can be dragged; it selects
  // only that row if the button comes up without a drag.
  let pressed: { row: number; x: number; y: number; keep: boolean } | undefined;

  function press(event: MouseEvent, row: number) {
    if (event.button !== 0) return;
    // As in Explorer: outside the name of an unselected row, a press starts a selection box.
    if (!tiles && !pane.isSelected(row) && !(event.target as Element).closest(".name")) {
      beginBox(event, row);
      return;
    }
    const keep = !event.ctrlKey && !event.shiftKey && pane.isSelected(row);
    if (keep) pane.selected = row;
    else void pane.select(row, event.ctrlKey ? "toggle" : event.shiftKey ? "range" : "single");
    pressed = pane.renaming === null ? { row, x: event.clientX, y: event.clientY, keep } : undefined;
  }

  function pointerMove(event: PointerEvent) {
    if (!pressed || !ondrag || !(event.buttons & 1)) return;
    if (Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) < 6) return;
    pressed = undefined;
    ondrag(event);
  }

  function pointerUp() {
    if (pressed?.keep) void pane.select(pressed.row);
    pressed = undefined;
  }

  // Drag selection: a box in content coordinates, so it stays put while the list scrolls.
  const BOX_THRESHOLD = 4;
  // The grid's side padding plus the tile lines' own padding.
  const TILES_LEFT = 14;
  let box = $state<{ left: number; top: number; width: number; height: number }>();
  let boxing: {
    x: number; y: number; pointerX: number; pointerY: number;
    moved: boolean; row?: number; ctrl: boolean; shift: boolean;
    base: Map<string, Entry>; last: string; frame: number;
  } | undefined;

  const contentX = (clientX: number) => clientX - grid.getBoundingClientRect().left + grid.scrollLeft;
  const contentY = (clientY: number) => clientY - grid.getBoundingClientRect().top + grid.scrollTop;

  function gridDown(event: MouseEvent) {
    onactivate();
    if (event.button !== 0 || (event.target as Element).closest("[data-row], .hdr, .rename")) return;
    const bounds = grid.getBoundingClientRect();
    // A press on the list's own scroll bars is not a press on the list.
    if (event.clientX >= bounds.left + grid.clientWidth || event.clientY >= bounds.top + grid.clientHeight) return;
    beginBox(event);
  }

  function beginBox(event: MouseEvent, row?: number) {
    if (pane.renaming !== null) return;
    boxing = {
      x: contentX(event.clientX), y: contentY(event.clientY),
      pointerX: event.clientX, pointerY: event.clientY,
      moved: false, row, ctrl: event.ctrlKey, shift: event.shiftKey,
      base: event.ctrlKey ? new Map(pane.selection) : new Map(), last: "", frame: 0,
    };
    window.addEventListener("pointermove", boxMove, true);
    window.addEventListener("pointerup", boxUp, true);
    window.addEventListener("pointercancel", endBox, true);
    window.addEventListener("keydown", boxKey, true);
  }

  /** The rows inside the box, in order from where the drag started. */
  function rowsInBox(left: number, top: number, right: number, bottom: number, upwards: boolean) {
    if (rows === 0) return [];
    const inRange = (value: number, last: number) => Math.max(0, Math.min(last, value));
    const result: number[] = [];
    if (!tiles) {
      if (bottom < HEADER || top >= HEADER + rows * ROW) return [];
      const from = inRange(Math.floor((top - HEADER) / ROW), rows - 1);
      const to = inRange(Math.floor((bottom - HEADER) / ROW), rows - 1);
      for (let row = from; row <= to; row++) result.push(row);
    } else {
      const stride = tile + GAP;
      const lastLine = lines - 1;
      if (bottom < PAD || right < TILES_LEFT) return [];
      const firstColumn = Math.max(0, Math.ceil((left - TILES_LEFT - tile) / stride));
      const lastColumn = Math.min(columns - 1, Math.floor((right - TILES_LEFT) / stride));
      const firstRow = Math.max(0, Math.ceil((top - PAD - tile) / stride));
      const lastRow = Math.min(lastLine, Math.floor((bottom - PAD) / stride));
      for (let index = firstRow; index <= lastRow; index++) {
        for (let column = firstColumn; column <= lastColumn; column++) {
          const row = index * columns + column;
          if (row < rows) result.push(row);
        }
      }
    }
    return upwards ? result.reverse() : result;
  }

  function drawBox() {
    if (!boxing) return;
    const x = Math.max(0, Math.min(grid.scrollWidth, contentX(boxing.pointerX)));
    const y = Math.max(0, Math.min(Math.max(grid.scrollHeight, grid.clientHeight), contentY(boxing.pointerY)));
    const left = Math.min(x, boxing.x);
    const top = Math.min(y, boxing.y);
    const right = Math.max(x, boxing.x);
    const bottom = Math.max(y, boxing.y);
    box = { left, top, width: right - left, height: bottom - top };
    const chosen = rowsInBox(left, top, right, bottom, y < boxing.y);
    const shape = chosen.length ? `${chosen[0]}:${chosen.at(-1)}:${chosen.length}` : "";
    if (shape !== boxing.last) {
      boxing.last = shape;
      void pane.selectRows(chosen, boxing.base);
    }
  }

  /** Keeps the box growing while the pointer rests near an edge. */
  function boxFrame() {
    if (!boxing) return;
    if (edgeScroll(grid, boxing.pointerY)) drawBox();
    boxing.frame = requestAnimationFrame(boxFrame);
  }

  function boxMove(event: PointerEvent) {
    if (!boxing) return;
    boxing.pointerX = event.clientX;
    boxing.pointerY = event.clientY;
    if (!boxing.moved) {
      if (Math.hypot(contentX(event.clientX) - boxing.x, contentY(event.clientY) - boxing.y) < BOX_THRESHOLD) return;
      boxing.moved = true;
      boxing.frame = requestAnimationFrame(boxFrame);
    }
    drawBox();
  }

  function boxUp() {
    const finished = boxing;
    endBox();
    if (!finished || finished.moved) return;
    if (finished.row !== undefined) void pane.select(finished.row, finished.ctrl ? "toggle" : finished.shift ? "range" : "single");
    else if (!finished.ctrl) pane.clearSelection();
  }

  function boxKey(event: KeyboardEvent) {
    if (event.key !== "Escape" || !boxing) return;
    event.preventDefault();
    event.stopPropagation();
    const base = boxing.base;
    endBox();
    pane.selection = base;
  }

  function endBox() {
    if (boxing) cancelAnimationFrame(boxing.frame);
    boxing = undefined;
    box = undefined;
    window.removeEventListener("pointermove", boxMove, true);
    window.removeEventListener("pointerup", boxUp, true);
    window.removeEventListener("pointercancel", endBox, true);
    window.removeEventListener("keydown", boxKey, true);
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

{#snippet renameInput(entry: Entry)}
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
{/snippet}

<section data-pane={pane.id} class="list" class:tiles class:marked class:searching={pane.searching} class:tagged aria-label={pane.folder?.path ?? "Folder"} style:--row={`${ROW}px`} style:--row-font={`${rowFont}px`}>
  <div
    class="grid"
    role="grid"
    tabindex="0"
    aria-multiselectable="true"
    aria-rowcount={tiles ? lines : rows + 1}
    aria-colcount={tiles ? columns : tagged ? 5 : 4}
    aria-activedescendant={activeRow}
    aria-label={pane.searching ? "Search results" : "Files"}
    bind:this={grid}
    bind:clientHeight={height}
    bind:clientWidth={width}
    onscroll={() => { scroll = grid.scrollTop; pane.scroll = scroll; }}
    onkeydown={keydown}
    onpointermove={pointerMove}
    onpointerup={pointerUp}
    onfocusin={onactivate}
    onmousedown={gridDown}
    oncontextmenu={areaMenu}
  >
    {#if tiles}
    <div class="spacer" role="rowgroup" style:height={`${lines * line + PAD}px`}>
      <div class="lines" style:transform={`translateY(${PAD + firstLine * line}px)`}>
        <!-- Lines are keyed like rows, so scrolling only adds and removes edge lines. -->
        {#each visibleLines as index (index)}
          <div class="line" role="row" aria-rowindex={index + 1} style:height={`${line}px`} style:gap={`${GAP}px`}>
            {#each visible.slice((index - firstLine) * columns, (index - firstLine + 1) * columns) as row (row)}
              {@const entry = pane.entry(row)}
              <!-- svelte-ignore a11y_interactive_supports_focus -->
              <div
                class="tile"
                class:sel={pane.isSelected(row)}
                class:focus={row === pane.selected}
                class:hidden={entry?.hidden}
                role="gridcell"
                id={`${id}-row-${row}`}
                data-row={row}
                aria-colindex={(row % columns) + 1}
                aria-selected={pane.isSelected(row)}
                title={entry ? (pane.searching ? location(entry) : entry.style?.alias ? entry.name : undefined) : undefined}
                style:width={`${tile}px`}
                style:height={`${tile}px`}
                onmousedown={(event) => press(event, row)}
                ondblclick={() => entry && onopen(entry, false)}
                oncontextmenu={(event) => rowMenu(event, row)}
              >
                {#if entry}
                  <span class="art">{#if entry.directory}<FolderIcon color={tint(entry.style)} glyph={entry.style?.icon} icon={entry.icon} size={iconSize} />{:else}<FileIcon icon={entry.icon} size={iconSize} />{/if}</span>
                  {#if row === pane.renaming}{@render renameInput(entry)}{:else}<span class="nm">{shownName(entry)}</span>{/if}
                {:else}
                  <span class="placeholder"></span>
                {/if}
              </div>
            {/each}
          </div>
        {/each}
      </div>
    </div>
    {:else}
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
            data-row={row}
            aria-rowindex={row + 2}
            aria-selected={pane.isSelected(row)}
            onmousedown={(event) => press(event, row)}
            ondblclick={() => entry && onopen(entry, false)}
            oncontextmenu={(event) => rowMenu(event, row)}
          >
            {#if entry}
              <div role="gridcell" class="name">
                {#if entry.directory}<FolderIcon color={tint(entry.style)} glyph={entry.style?.icon} icon={entry.icon} size={rowIcon} />{:else}<FileIcon icon={entry.icon} size={rowIcon} />{/if}
                {#if row === pane.renaming}
                  {@render renameInput(entry)}
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
    {/if}
    {#if box}
      <div class="box" aria-hidden="true" style:left={`${box.left}px`} style:top={`${box.top}px`} style:width={`${box.width}px`} style:height={`${box.height}px`}></div>
    {/if}
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
    height: var(--row);
    padding: 0 10px;
    border-radius: var(--radius-s);
  }
  .row { font-size: var(--row-font); }
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
    height: 32px;
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
  .lines { display: flex; flex-direction: column; padding: 0 8px; }
  .line { display: flex; flex: none; }
  .tile { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; flex: none; padding: 8px; border-radius: var(--radius-s); text-align: center; font-size: 12px; }
  .tile:hover { background: var(--hover); }
  .tile.sel { background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); }
  .tile.hidden .art { opacity: .6; }
  .grid:focus-visible .tile.focus { outline: 2px solid var(--accent); outline-offset: -2px; }
  .art { display: flex; flex: none; }
  .tile .nm { display: -webkit-box; max-width: 100%; overflow: hidden; line-height: 16px; white-space: normal; overflow-wrap: anywhere; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-clamp: 2; }
  .tile .rename { flex: none; width: 100%; margin: 0; text-align: center; }
  .tile .placeholder { width: 60%; }
  :global(.drop-target).row, :global(.drop-target).tile { outline: 2px dashed var(--accent); outline-offset: -2px; background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .list:global(.drop-target) { outline: 2px dashed var(--accent); outline-offset: -4px; }
  .box { position: absolute; z-index: 2; border: 1px solid var(--accent); border-radius: 2px; background: color-mix(in srgb, var(--accent) 18%, transparent); pointer-events: none; }
  .placeholder { height: 8px; width: 40%; border-radius: 4px; background: var(--sep); }
  .empty { position: absolute; inset: 56px 24px auto 24px; margin: 0; color: var(--muted); white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
