<script lang="ts">
  import { api } from "./api";
  import type { SearchFilters } from "./contract";
  import { formatCount } from "./format";
  import Icon from "./Icon.svelte";
  import { NO_FILTERS, filterCount, type Pane } from "./pane.svelte";
  import { volumes } from "./volumes.svelte";

  let {
    pane,
    label,
    compact = false,
    onvolume,
  }: {
    pane: Pane;
    /** The folder name the placeholder and accessible name use. */
    label: string;
    /** The split-view variant inside a pane's navigation row. */
    compact?: boolean;
    /** Asks the window to confirm and start the NTFS index for `drive`. */
    onvolume: (drive: string) => void;
  } = $props();

  const UNITS = [["KB", 1024], ["MB", 1024 ** 2], ["GB", 1024 ** 3]] as const;
  type Unit = (typeof UNITS)[number][0];

  let field: HTMLInputElement;
  let open = $state<"scope" | "filters" | null>(null);
  let drive = $state<string | null | undefined>();
  let draft = $state({ extension: "", min: "", minUnit: "MB" as Unit, max: "", maxUnit: "MB" as Unit, from: "", through: "" });
  let invalid = $state("");

  const active = $derived(pane.searching || pane.regex || pane.volume !== null || filterCount(pane.filters) > 0 || open !== null);
  const scopeLabel = $derived(pane.volume ? `${pane.volume}: · NTFS` : "This folder");
  const running = $derived(Object.values(volumes.states));

  export function focus() {
    field.focus();
    field.select();
  }

  async function showScope() {
    if (open === "scope") { open = null; return; }
    open = "scope";
    const path = pane.path;
    drive = undefined;
    drive = await api.ntfsDrive(path).catch(() => null);
  }

  function chooseFolder() {
    open = null;
    pane.setVolume(null);
    field.focus();
  }

  function chooseVolume(letter: string) {
    open = null;
    if (volumes.states[letter]) pane.setVolume(letter);
    else onvolume(letter);
    field.focus();
  }

  function stop(letter: string) {
    open = null;
    void api.stopVolumeIndex(letter);
  }

  const bytes = (value: string, unit: Unit) => {
    if (!value.trim()) return null;
    const number = Number(value.replace(",", "."));
    if (!Number.isFinite(number) || number < 0) throw new Error("Enter sizes as positive numbers");
    return Math.round(number * UNITS.find(([name]) => name === unit)![1]);
  };
  const seconds = (value: string, end: boolean) =>
    value ? Math.floor(new Date(`${value}T${end ? "23:59:59" : "00:00:00"}`).getTime() / 1000) : null;
  const sizeText = (value: number | null): [string, Unit] => {
    if (value === null) return ["", "MB"];
    const [unit, scale] = [...UNITS].reverse().find(([, scale]) => value >= scale && value % scale === 0) ?? UNITS[0];
    return [String(+(value / scale).toFixed(3)), unit];
  };
  const dateText = (value: number | null) => {
    if (value === null) return "";
    const date = new Date(value * 1000);
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  };

  function showFilters() {
    if (open === "filters") { open = null; return; }
    const filters = pane.filters;
    const [min, minUnit] = sizeText(filters.minBytes);
    const [max, maxUnit] = sizeText(filters.maxBytes);
    draft = { extension: filters.extension ?? "", min, minUnit, max, maxUnit, from: dateText(filters.modifiedAfter), through: dateText(filters.modifiedBefore) };
    invalid = "";
    open = "filters";
  }

  /** Applies the filters while they are typed; an incomplete or reversed range waits. */
  function applyFilters() {
    let filters: SearchFilters;
    try {
      const extension = draft.extension.trim().replace(/^\.+/, "");
      if (/[\\/:*?"<>|]/.test(extension)) throw new Error("Enter one extension such as pdf, without wildcards");
      filters = {
        extension: extension || null,
        minBytes: bytes(draft.min, draft.minUnit),
        maxBytes: bytes(draft.max, draft.maxUnit),
        modifiedAfter: seconds(draft.from, false),
        modifiedBefore: seconds(draft.through, true),
      };
      if (filters.minBytes !== null && filters.maxBytes !== null && filters.minBytes > filters.maxBytes) throw new Error("The minimum size is larger than the maximum size");
      if (filters.modifiedAfter !== null && filters.modifiedBefore !== null && filters.modifiedAfter > filters.modifiedBefore) throw new Error("The date range is reversed");
    } catch (reason) {
      invalid = (reason as Error).message;
      return;
    }
    invalid = "";
    pane.setFilters(filters);
  }

  function clearFilters() {
    draft = { extension: "", min: "", minUnit: "MB", max: "", maxUnit: "MB", from: "", through: "" };
    invalid = "";
    pane.setFilters(NO_FILTERS);
  }

  function keydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (open) open = null;
    else if (pane.query) pane.input("");
    else return;
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<svelte:window onmousedown={() => (open = null)} />

<!-- The box keeps its popovers open while used and closes them on Escape. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="box" class:active class:compact role="search" onmousedown={(event) => event.stopPropagation()} onkeydown={keydown}>
  <!-- A plain container rather than a label, so the field's name excludes the buttons. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="field" onclick={(event) => { if (event.target === event.currentTarget) field.focus(); }}>
    <span class="muted icon"><Icon name="search" /></span>
    <input
      bind:this={field}
      type="search"
      aria-label={pane.volume ? `Search ${pane.volume}:` : `Search ${label}`}
      placeholder={pane.volume ? `Search ${pane.volume}:` : `Search ${label}`}
      aria-invalid={pane.searchError ? "true" : undefined}
      value={pane.query}
      oninput={(event) => pane.input(event.currentTarget.value)}
    />
    {#if active}
      <button class="chip" class:on={pane.volume !== null} aria-haspopup="menu" aria-expanded={open === "scope"} title="Search scope" onclick={showScope}>
        {scopeLabel}<span class="caret"><Icon name="chevron" size={10} weight={2.2} /></span>
      </button>
      <button class="ib sm rx" class:on={pane.regex} aria-pressed={pane.regex} aria-label="Regular expression" title="Regular expression" onclick={() => pane.setRegex(!pane.regex)}>.*</button>
      <button class="ib sm" class:on={filterCount(pane.filters) > 0} aria-expanded={open === "filters"} aria-label={filterCount(pane.filters) ? `Filters (${filterCount(pane.filters)} active)` : "Filters"} title="Filters" onclick={showFilters}>
        <Icon name="filter" size={14} />
        {#if filterCount(pane.filters)}<span class="count">{filterCount(pane.filters)}</span>{/if}
      </button>
    {:else if !compact}
      <kbd class="kbd" aria-hidden="true">Ctrl F</kbd>
    {/if}
  </div>

  {#if open === "scope"}
    <div class="menu pop" role="menu" aria-label="Search scope">
      <button class="mi" class:on={!pane.volume} role="menuitemradio" aria-checked={!pane.volume} onclick={chooseFolder}>
        <Icon name="open" /><span class="label">This folder and subfolders</span>
      </button>
      {#if drive === undefined}
        <div class="note">Checking the drive…</div>
      {:else if drive === null}
        <div class="note">NTFS search needs a folder on a local NTFS drive.</div>
      {:else}
        {@const state = volumes.states[drive]}
        <button class="mi" class:on={pane.volume === drive} role="menuitemradio" aria-checked={pane.volume === drive} onclick={() => chooseVolume(drive!)}>
          <Icon name="drive" /><span class="label">{drive}: · NTFS</span>
          <span class="sc">{state ? (state.ready ? `${formatCount(state.count)} items` : "Indexing…") : "Needs approval"}</span>
        </button>
      {/if}
      {#if running.length}
        <div class="sep"></div>
        {#each running as volume (volume.drive)}
          <button class="mi" role="menuitem" onclick={() => stop(volume.drive)}><Icon name="close" /><span class="label">Stop the {volume.drive}: index</span></button>
        {/each}
      {/if}
    </div>
  {:else if open === "filters"}
    <form class="menu pop filters" aria-label="Search filters" oninput={applyFilters} onchange={applyFilters} onsubmit={(event) => { event.preventDefault(); open = null; }}>
      <label class="row"><span>Extension</span><input class="field" placeholder="pdf" spellcheck="false" bind:value={draft.extension} /></label>
      <div class="row">
        <span>Size</span>
        <div class="pair">
          <input class="field" inputmode="decimal" placeholder="At least" aria-label="Minimum size" bind:value={draft.min} />
          <select class="field" aria-label="Minimum size unit" bind:value={draft.minUnit}>{#each UNITS as [unit] (unit)}<option>{unit}</option>{/each}</select>
          <input class="field" inputmode="decimal" placeholder="At most" aria-label="Maximum size" bind:value={draft.max} />
          <select class="field" aria-label="Maximum size unit" bind:value={draft.maxUnit}>{#each UNITS as [unit] (unit)}<option>{unit}</option>{/each}</select>
        </div>
      </div>
      <div class="row">
        <span>Modified</span>
        <div class="pair">
          <input class="field" type="date" aria-label="Modified from" bind:value={draft.from} />
          <span class="muted">to</span>
          <input class="field" type="date" aria-label="Modified through" bind:value={draft.through} />
        </div>
      </div>
      {#if invalid}<p class="invalid" role="alert">{invalid}</p>{/if}
      <div class="foot">
        <span class="muted">Applies while you type.</span>
        <button type="button" class="action" onclick={clearFilters}>Clear</button>
        <button type="submit" class="action primary">Done</button>
      </div>
    </form>
  {/if}
</div>

<style>
  .box { position: relative; flex: 0 1 280px; min-width: 180px; margin: 0 6px; transition: flex-basis .12s ease; }
  .box.active { flex-basis: 460px; }
  .box.compact { flex: 0 1 35%; min-width: 120px; margin: 0; }
  .box.compact.active { flex-basis: 60%; }
  .icon { display: flex; flex: none; }
  .chip { display: inline-flex; align-items: center; gap: 4px; flex: none; max-width: 140px; height: 24px; padding: 0 8px; overflow: hidden; border: 1px solid var(--sep); border-radius: 999px; background: transparent; font-size: 12px; white-space: nowrap; text-overflow: ellipsis; }
  .chip:hover { background: var(--hover); }
  .chip.on { border-color: transparent; background: var(--sel); color: var(--sel-text); box-shadow: var(--sel-shadow); }
  .caret { display: flex; transform: rotate(90deg); }
  .rx { font: 600 12px/1 ui-monospace, Consolas, monospace; }
  .count { margin-left: 1px; font-size: 11px; font-weight: 600; }
  .pop { position: absolute; top: calc(100% + 6px); right: 0; z-index: 25; }
  .note { padding: 6px 10px; max-width: 300px; font-size: 12px; color: var(--muted); }
  .filters { display: flex; flex-direction: column; gap: 10px; width: 400px; max-width: calc(100vw - 32px); padding: 14px; }
  .row { display: grid; grid-template-columns: 76px 1fr; align-items: center; gap: 8px; font-size: 13px; }
  .pair { display: flex; align-items: center; gap: 6px; min-width: 0; }
  .filters .field { height: 30px; min-width: 0; padding: 0 8px; font-size: 13px; }
  .pair input.field { flex: 1 1 0; width: 0; }
  .pair select.field { flex: none; padding: 0 4px; }
  .invalid { margin: 0; font-size: 12px; color: var(--text); }
  .invalid::before { content: "⚠ "; color: var(--danger); }
  .foot { display: flex; align-items: center; gap: 8px; font-size: 12px; }
  .foot .muted { flex-grow: 1; }
  .foot .action { padding: 5px 12px; font-size: 13px; }
  input[aria-invalid="true"] { text-decoration: underline wavy var(--danger); }
</style>
