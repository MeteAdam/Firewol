<script lang="ts">
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import type { Entry, PickMatch, PickView } from "./contract";

  let { pick, chosen, onfilter, onopen, oncancel, onsearch, onmatch, ontyped, onresize }: {
    pick: PickView;
    /** Selected items the request accepts. */
    chosen: Entry[];
    onfilter: (index: number) => void;
    onopen: () => void;
    oncancel: () => void;
    /** Items of the open folder whose names contain the text. */
    onsearch: (text: string) => Promise<PickMatch[]>;
    onmatch: (match: PickMatch) => void;
    /** Enter on a name or path that was typed rather than picked from the list. */
    ontyped: (text: string) => void;
    onresize: () => void;
  } = $props();

  const SEARCH_DELAY_MS = 80;
  let field: HTMLInputElement;
  let text = $state(untrack(() => pick.file_name ?? ""));
  /** The field holds what the person typed, not the selected items' names. */
  let typing = $state(false);
  let matches = $state<PickMatch[]>([]);
  let highlight = $state(-1);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let searchId = 0;

  const canOpen = $derived(chosen.length > 0 || pick.folders || (typing && text.trim().length > 0));

  // Choosing in the list writes the names here, the way a Windows Open window does.
  $effect(() => {
    const names = chosen.map((entry) => entry.name);
    untrack(() => {
      if (names.length === 0) return;
      typing = false;
      closeList();
      text = names.length === 1 ? names[0] : names.map((name) => `"${name}"`).join(" ");
    });
  });

  export function focus() {
    field.focus();
  }

  function closeList() {
    clearTimeout(timer);
    searchId++;
    matches = [];
    highlight = -1;
  }

  function input() {
    typing = true;
    highlight = -1;
    clearTimeout(timer);
    const query = text.trim();
    if (!query) {
      closeList();
      return;
    }
    const id = ++searchId;
    timer = setTimeout(async () => {
      const found = await onsearch(query).catch(() => []);
      if (id === searchId) matches = found;
    }, SEARCH_DELAY_MS);
  }

  function choose(match: PickMatch) {
    closeList();
    text = match.name;
    typing = false;
    onmatch(match);
  }

  function submit() {
    closeList();
    if (typing && text.trim()) ontyped(text);
    else onopen();
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" && matches.length > 0) highlight = (highlight + 1) % matches.length;
    else if (event.key === "ArrowUp" && matches.length > 0) highlight = (highlight - 1 + matches.length) % matches.length;
    else if (event.key === "Enter") {
      if (highlight >= 0) choose(matches[highlight]);
      else submit();
    } else if (event.key === "Escape") {
      if (matches.length > 0) closeList();
      else oncancel();
    } else return;
    event.preventDefault();
    event.stopPropagation();
  }
</script>

<div class="pick-bar" role="group" aria-label={pick.title ?? "Choose a file"}>
  <div class="row">
    <label class="label" for="pick-name">{pick.folders ? "Folder" : "File name"}</label>
    <div class="name">
      {#if matches.length > 0}
        <div class="matches" role="listbox" aria-label="Matching items">
          {#each matches as match, index (match.path)}
            <button class="match" class:on={index === highlight} role="option" aria-selected={index === highlight} tabindex="-1" title={match.path} onmousedown={(event) => event.preventDefault()} onclick={() => choose(match)}>
              <Icon name={match.directory ? "open" : "file"} size={14} /><span>{match.name}</span>
            </button>
          {/each}
        </div>
      {/if}
      <input id="pick-name" class="field-text pick-name" spellcheck="false" autocomplete="off" role="combobox" aria-expanded={matches.length > 0} aria-controls="pick-name" placeholder={pick.folders ? "Type a folder name or path" : "Type to find a file, or paste a path"} bind:this={field} bind:value={text} oninput={input} onkeydown={keydown} onblur={closeList} />
    </div>
    {#if pick.types.length > 0 && !pick.folders}
      <select class="field-text types" aria-label="File type" value={pick.type_index} onchange={(event) => onfilter(Number(event.currentTarget.value))}>
        {#each pick.types as type, index (index)}<option value={index + 1}>{type.name} ({type.spec})</option>{/each}
      </select>
    {/if}
  </div>
  <div class="row end">
    {#if pick.title}<strong class="title" title={pick.title}>{pick.title}</strong>{/if}
    <button class="action primary pick-open" disabled={!canOpen} onclick={submit}>{pick.folders ? "Select folder" : "Open"}</button>
    <button class="action pick-cancel" onclick={oncancel}>Cancel</button>
  </div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="grip" title="Drag to resize" onpointerdown={(event) => { if (event.button === 0) { event.preventDefault(); onresize(); } }}>
    <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><circle cx="10" cy="10" r="1.1" /><circle cx="6" cy="10" r="1.1" /><circle cx="2" cy="10" r="1.1" /><circle cx="10" cy="6" r="1.1" /><circle cx="6" cy="6" r="1.1" /><circle cx="10" cy="2" r="1.1" /></svg>
  </div>
</div>

<style>
  .pick-bar { position: relative; display: flex; flex-direction: column; gap: 8px; flex: none; padding: 10px 28px 12px 16px; border-top: 1px solid var(--sep); background: var(--surface); font-size: 13px; }
  .row { display: flex; align-items: center; gap: 12px; min-width: 0; }
  .end { justify-content: flex-end; }
  .label { flex: none; color: var(--muted); }
  .name { position: relative; flex: 1 1 0; min-width: 0; }
  .field-text { width: 100%; height: 34px; padding: 0 10px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); color: var(--text); font-size: 13px; user-select: text; }
  .types { flex: 0 1 300px; width: auto; min-width: 140px; font-size: 12px; }
  .title { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .action { flex: none; min-width: 92px; }
  .matches { position: absolute; left: 0; right: 0; bottom: calc(100% + 4px); z-index: 20; display: flex; flex-direction: column; max-height: 320px; padding: 4px; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); box-shadow: var(--shadow-pop); }
  .match { display: flex; align-items: center; gap: 8px; flex: none; height: 28px; padding: 0 8px; border: 0; border-radius: calc(var(--radius-s) - 2px); background: transparent; color: var(--text); font-size: 13px; text-align: left; }
  .match span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .match:hover { background: var(--hover); }
  .match.on { background: var(--sel); color: var(--sel-text); }
  .grip { position: absolute; right: 2px; bottom: 2px; display: flex; padding: 4px; color: var(--muted); cursor: se-resize; touch-action: none; }
  .grip svg { fill: currentColor; }
</style>
