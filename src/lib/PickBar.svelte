<script lang="ts">
  import type { Entry, PickView } from "./contract";

  let { pick, chosen, folderName, onfilter, onopen, oncancel }: {
    pick: PickView;
    /** Selected items the request accepts. */
    chosen: Entry[];
    /** The open folder, which is the answer of a folder request with nothing selected. */
    folderName: string;
    onfilter: (index: number) => void;
    onopen: () => void;
    oncancel: () => void;
  } = $props();

  const names = $derived(chosen.map((entry) => entry.name).join(", "));
  const shown = $derived(names || (pick.folders ? folderName : pick.file_name ?? ""));
  const canOpen = $derived(chosen.length > 0 || pick.folders);
</script>

<div class="pick-bar" role="group" aria-label={pick.title ?? "Choose a file"}>
  {#if pick.title}<strong class="title" title={pick.title}>{pick.title}</strong>{/if}
  <span class="name" title={shown}>{shown || (pick.multi ? "Select one or more files" : "Select a file")}</span>
  {#if pick.types.length > 0 && !pick.folders}
    <select class="types" aria-label="File type" value={pick.type_index} onchange={(event) => onfilter(Number(event.currentTarget.value))}>
      {#each pick.types as type, index (index)}<option value={index + 1}>{type.name} ({type.spec})</option>{/each}
    </select>
  {/if}
  <button class="action primary pick-open" disabled={!canOpen} onclick={onopen}>{pick.folders ? "Select folder" : "Open"}</button>
  <button class="action pick-cancel" onclick={oncancel}>Cancel</button>
</div>

<style>
  .pick-bar { display: flex; align-items: center; gap: 12px; flex: none; min-height: 48px; padding: 6px 12px; border-top: 1px solid var(--sep); background: var(--surface); font-size: 13px; }
  .title { flex: 0 1 auto; max-width: 30%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .name { flex: 1 1 0; min-width: 0; overflow: hidden; color: var(--muted); text-overflow: ellipsis; white-space: nowrap; }
  .types { flex: 0 1 280px; min-width: 120px; height: 32px; padding: 0 8px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); color: var(--text); font-size: 12px; }
  .action { flex: none; min-width: 84px; }
</style>
