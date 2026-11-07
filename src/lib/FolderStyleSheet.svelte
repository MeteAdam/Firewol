<script lang="ts">
  import { untrack } from "svelte";
  import type { FolderStyleView } from "./contract";
  import { hexOf, parseHex, type Rgb } from "./color";
  import FolderIcon from "./FolderIcon.svelte";
  import Sheet from "./Sheet.svelte";
  import { GLYPHS, LIMITS, SWATCHES, splitTags } from "./styles";

  let { name, path, style, onsave, onclose }: {
    name: string;
    path: string;
    /** The saved style, if the folder has one. */
    style?: FolderStyleView;
    /** Saves the style (`null` clears it); a rejection shows inline. */
    onsave: (style: FolderStyleView | null) => Promise<void>;
    onclose: () => void;
  } = $props();

  // The sheet edits a copy; nothing is saved until Save or Clear style.
  const saved = untrack(() => style);
  let alias = $state(saved?.alias ?? "");
  let color = $state<Rgb | null>(saved?.color ?? null);
  let icon = $state(saved?.icon ?? "");
  let tags = $state<string[]>([...(saved?.tags ?? [])]);
  let draft = $state("");
  let error = $state("");
  let saving = $state(false);
  const tint = $derived(color ? hexOf(color) : undefined);
  const glyph = $derived(icon.trim());
  const same = (a: Rgb | null, b: Rgb) => !!a && a.every((channel, index) => channel === b[index]);

  /** Adds comma-separated tags; returns false (with an inline message) when one is refused. */
  function addTags(text: string) {
    for (const tag of splitTags(text)) {
      if (tags.includes(tag)) continue;
      if (tag.length > LIMITS.tag) { error = `Keep each tag within ${LIMITS.tag} characters`; return false; }
      if (tags.length >= LIMITS.tags) { error = `Use up to ${LIMITS.tags} tags per folder`; return false; }
      tags = [...tags, tag];
    }
    return true;
  }

  function typeTags(input: HTMLInputElement) {
    error = "";
    const cut = input.value.lastIndexOf(",");
    if (cut >= 0 && addTags(input.value.slice(0, cut))) input.value = input.value.slice(cut + 1).trimStart();
    draft = input.value;
  }

  function tagKey(event: KeyboardEvent & { currentTarget: HTMLInputElement }) {
    if (event.key === "Enter") {
      event.preventDefault();
      if (addTags(draft)) draft = event.currentTarget.value = "";
    } else if (event.key === "Backspace" && !event.currentTarget.value && tags.length) {
      tags = tags.slice(0, -1);
    }
  }

  function setHex(input: HTMLInputElement) {
    const value = parseHex(input.value);
    input.setCustomValidity(value === undefined ? "Enter a colour as #RRGGBB" : "");
    if (value === undefined) input.reportValidity();
    else color = value;
  }

  async function save(next: FolderStyleView | null) {
    saving = true;
    try { await onsave(next); }
    catch (reason) { error = String(reason); }
    finally { saving = false; }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    error = "";
    if (!addTags(draft)) return;
    draft = "";
    void save({ alias: alias.trim(), color, icon: glyph, tags });
  }
</script>

<Sheet title="Customize folder" {onclose}>
  <form class="folder-style" onsubmit={submit}>
    <div class="preview">
      <span class="badge"><FolderIcon color={tint} glyph={glyph} size={28} /></span>
      <div class="who">
        <strong>{alias.trim() || name}</strong>
        <span class="muted where" title={path}>{alias.trim() && alias.trim() !== name ? `${name} · ${path}` : path}</span>
      </div>
    </div>

    <label class="group">
      <span class="caption">Display name</span>
      <input class="text" aria-label="Display name" placeholder={name} maxlength={LIMITS.alias} spellcheck="false" bind:value={alias} />
    </label>

    <div class="group">
      <span class="caption" id="folder-colour">Icon colour</span>
      <div class="choices" role="radiogroup" aria-labelledby="folder-colour">
        <button type="button" class="swatch theme" role="radio" aria-checked={!color} aria-label="Theme colour" title="Theme colour" onclick={() => (color = null)}></button>
        {#each SWATCHES as [label, rgb] (label)}
          <button type="button" class="swatch" role="radio" aria-checked={same(color, rgb)} aria-label={label} title={label} style:background={hexOf(rgb)} onclick={() => (color = rgb)}></button>
        {/each}
        <input class="text hex" aria-label="Custom icon colour" placeholder="#RRGGBB" spellcheck="false" value={tint ?? ""} onchange={(event) => setHex(event.currentTarget)} />
      </div>
    </div>

    <div class="group">
      <span class="caption" id="folder-glyph">Icon</span>
      <div class="choices" role="radiogroup" aria-labelledby="folder-glyph">
        <button type="button" class="glyph" role="radio" aria-checked={!glyph} aria-label="Folder icon" title="Folder icon" onclick={() => (icon = "")}><FolderIcon color={tint} /></button>
        {#each GLYPHS as option (option)}
          <button type="button" class="glyph" role="radio" aria-checked={glyph === option} aria-label={`Icon ${option}`} style:color={tint ?? "var(--folder)"} onclick={() => (icon = option)}>{option}</button>
        {/each}
        <input class="text custom" aria-label="Custom icon" placeholder="Other" maxlength={LIMITS.icon} spellcheck="false" bind:value={icon} />
      </div>
    </div>

    <div class="group">
      <span class="caption" id="folder-tags">Tags</span>
      <div class="tagbox" role="group" aria-labelledby="folder-tags">
        {#each tags as tag (tag)}
          <span class="chip">{tag}<button type="button" aria-label={`Remove tag ${tag}`} onclick={() => (tags = tags.filter((item) => item !== tag))}>×</button></span>
        {/each}
        <input aria-label="Add tag" placeholder={tags.length ? "" : "Type a tag, then Enter or a comma"} maxlength={300} spellcheck="false" disabled={tags.length >= LIMITS.tags} value={draft} oninput={(event) => typeTags(event.currentTarget)} onkeydown={tagKey} />
      </div>
      <span class="hint muted">Up to {LIMITS.tags} tags. A Tags column appears in folders that list a tagged folder.</span>
    </div>

    {#if error}<p class="error" role="alert">{error}</p>{/if}

    <div class="actions">
      {#if saved}<button type="button" class="action clear" disabled={saving} onclick={() => save(null)}>Clear style</button>{/if}
      <span class="grow"></span>
      <button type="button" class="action" onclick={onclose}>Cancel</button>
      <button type="submit" class="action primary" disabled={saving}>Save</button>
    </div>
  </form>
</Sheet>

<style>
  .folder-style { display: flex; flex-direction: column; gap: 16px; margin-top: 16px; }
  .preview { display: flex; align-items: center; gap: 12px; min-width: 0; padding: 12px 14px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); box-shadow: var(--field-shadow); }
  .badge { display: flex; flex: none; }
  .who { display: flex; flex-direction: column; min-width: 0; }
  .who strong { overflow: hidden; font-size: 14px; text-overflow: ellipsis; white-space: nowrap; }
  .where { overflow: hidden; font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
  .group { display: flex; flex-direction: column; gap: 6px; }
  .caption { font-size: 12px; font-weight: 600; color: var(--muted); }
  .text { height: 34px; padding: 0 10px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); box-shadow: var(--field-shadow); font-size: 13px; outline: none; }
  .text:focus { border-color: var(--accent); }
  .choices { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
  .swatch { width: 26px; height: 26px; padding: 0; border: 2px solid transparent; border-radius: 50%; box-shadow: inset 0 0 0 1px rgb(0 0 0 / .12); }
  .swatch.theme { background: conic-gradient(var(--folder) 0 50%, var(--surface) 0 100%); }
  .swatch[aria-checked="true"] { border-color: var(--surface); box-shadow: 0 0 0 2px var(--accent); }
  .hex { width: 104px; height: 28px; margin-left: 4px; font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; }
  .hex:invalid { border-color: #c42b1c; }
  .glyph { display: inline-flex; align-items: center; justify-content: center; width: 30px; height: 30px; padding: 0; border: var(--border-w) solid transparent; border-radius: var(--radius-s); background: transparent; font-family: "Segoe UI Symbol", "Segoe UI Emoji", sans-serif; font-size: 16px; line-height: 1; }
  .glyph:hover { background: var(--hover); }
  .glyph[aria-checked="true"] { background: var(--sel); border-color: var(--accent); }
  .custom { width: 72px; height: 30px; margin-left: 4px; text-align: center; }
  .tagbox { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; min-height: 34px; padding: 4px 6px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); box-shadow: var(--field-shadow); }
  .tagbox:focus-within { border-color: var(--accent); }
  .tagbox input { flex: 1; min-width: 120px; height: 24px; padding: 0 4px; border: 0; background: transparent; font-size: 13px; outline: none; }
  .chip { display: inline-flex; align-items: center; gap: 2px; height: 22px; padding: 0 2px 0 9px; border-radius: 999px; background: color-mix(in srgb, var(--accent) 18%, transparent); font-size: 12px; }
  .chip button { display: inline-flex; align-items: center; justify-content: center; width: 18px; height: 18px; padding: 0; border: 0; border-radius: 50%; background: transparent; font-size: 13px; line-height: 1; }
  .chip button:hover { background: var(--hover); }
  .hint { font-size: 12px; }
  .error { margin: 0; padding: 8px 12px; border: 1px solid #c42b1c; border-radius: var(--radius-s); font-size: 13px; }
  .actions { align-items: center; margin-top: 4px !important; }
  .grow { flex: 1; }
  .clear { background: transparent; }
</style>
