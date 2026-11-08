<script lang="ts">
  import type { MenuEntryView, SettingsPatch } from "./contract";
  import { defaultMenu, details, entryKey, isCustom, labelOf } from "./menu";
  import Icon from "./Icon.svelte";

  let { menu, shellItems, onsave }: { menu: MenuEntryView[]; shellItems: boolean; onsave: (patch: SettingsPatch) => Promise<void> } = $props();
  let error = $state("");
  let confirmReset = $state(false);
  let label = $state("");
  let path = $state("");
  let administrator = $state(false);

  // Stable keys keep focus on a row's buttons while it moves.
  const rows = $derived.by(() => {
    const seen = new Map<string, number>();
    return menu.map((entry, index) => {
      const base = entryKey(entry);
      const count = seen.get(base) ?? 0;
      seen.set(base, count + 1);
      return { entry, index, key: `${base}#${count}` };
    });
  });

  async function save(next: MenuEntryView[]) {
    error = "";
    try {
      await onsave({ menu: next });
      return true;
    } catch (reason) {
      error = String(reason);
      return false;
    }
  }

  async function toggleShellItems(enabled: boolean) {
    error = "";
    try { await onsave({ shell_items: enabled }); }
    catch (reason) { error = String(reason); }
  }

  function move(index: number, by: number) {
    const next = [...menu];
    next.splice(index + by, 0, ...next.splice(index, 1));
    void save(next);
  }

  const change = (index: number, patch: Partial<MenuEntryView>) => save(menu.map((entry, at) => (at === index ? { ...entry, ...patch } : entry)));

  function rename(index: number, value: string) {
    const text = value.trim();
    void change(index, { label: text === details(menu[index].action).label ? "" : text });
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    const entry: MenuEntryView = { action: { custom: { executable: path.trim(), administrator } }, visible: true, label: label.trim(), icon: "", color: null };
    if (await save([...menu, entry])) {
      label = "";
      path = "";
      administrator = false;
    }
  }
</script>

<h2>Context menu</h2>
<p class="muted">Choose which actions appear when you right-click, and in what order. Actions that do not apply to the clicked item stay hidden.</p>
<ol class="entries" aria-label="Menu entries">
  {#each rows as { entry, index, key } (key)}
    {@const custom = isCustom(entry.action) ? entry.action.custom : undefined}
    <li class="entry" class:off={!entry.visible}>
      <input type="checkbox" checked={entry.visible} aria-label={`Show ${labelOf(entry)}`} onchange={() => change(index, { visible: !entry.visible })} />
      <span class="glyph"><Icon name={details(entry.action).icon} /></span>
      <input class="name" value={labelOf(entry)} aria-label={`Label for ${labelOf(entry)}`} spellcheck="false" onchange={(event) => rename(index, event.currentTarget.value)} />
      {#if custom}
        <span class="path" title={custom.executable}>{custom.executable}</span>
        {#if custom.administrator}<span class="badge">Admin</span>{/if}
      {:else}
        <span class="hint">{details(entry.action).shortcut}</span>
      {/if}
      <button class="ib sm" aria-label={`Move ${labelOf(entry)} up`} title="Move up" disabled={index === 0} onclick={() => move(index, -1)}><Icon name="up" size={14} /></button>
      <button class="ib sm" aria-label={`Move ${labelOf(entry)} down`} title="Move down" disabled={index === menu.length - 1} onclick={() => move(index, 1)}><Icon name="down" size={14} /></button>
      {#if custom}
        <button class="ib sm" aria-label={`Remove ${labelOf(entry)}`} title="Remove" onclick={() => save(menu.filter((_, at) => at !== index))}><Icon name="trash" size={14} /></button>
      {:else}
        <span class="ib-space"></span>
      {/if}
    </li>
  {/each}
</ol>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<h3>Windows app commands</h3>
<label class="check shell"><input type="checkbox" checked={shellItems} aria-label="Show Windows app commands" onchange={(event) => toggleShellItems(event.currentTarget.checked)} /> Show commands from Windows and installed apps, such as VS Code or WinRAR</label>

<h3>Add a custom action</h3>
<p class="muted">Runs a program with the clicked item's path as its argument (the open folder when you click empty space).</p>
<form class="add" onsubmit={add}>
  <input class="text" placeholder="Label, e.g. Open in VS Code" aria-label="Custom action label" bind:value={label} spellcheck="false" />
  <input class="text grow" placeholder="C:\Program Files\App\app.exe" aria-label="Executable path" bind:value={path} spellcheck="false" required />
  <label class="check"><input type="checkbox" bind:checked={administrator} /> Run as administrator</label>
  <button class="action primary" disabled={!path.trim()}>Add action</button>
</form>

<div class="reset">
  {#if confirmReset}
    <div class="confirm" role="alertdialog" aria-label="Reset the context menu">
      <span>Restore the default actions and order? Custom actions and labels are removed.</span>
      <button class="action primary" onclick={async () => { if (await save(defaultMenu())) confirmReset = false; }}>Reset menu</button>
      <button class="action" onclick={() => (confirmReset = false)}>Cancel</button>
    </div>
  {:else}
    <button class="link" onclick={() => (confirmReset = true)}>Reset to default…</button>
  {/if}
</div>

<style>
  h2 { margin: 0 0 2px; font-size: 14px; font-weight: 600; }
  h3 { margin: 24px 0 2px; font-size: 13px; font-weight: 600; }
  p { margin: 0 0 12px; font-size: 12px; }
  .entries { display: flex; flex-direction: column; gap: 2px; max-width: 760px; margin: 0; padding: 0; list-style: none; }
  .entry { display: flex; align-items: center; gap: 10px; height: 36px; padding: 0 6px 0 10px; border-radius: var(--radius-s); }
  .entry:hover { background: var(--hover); }
  .entry.off .glyph, .entry.off .name { opacity: .5; }
  .entry input[type="checkbox"], .check input { accent-color: var(--accent); }
  .glyph { display: flex; color: var(--muted); }
  .name { flex: 0 1 260px; min-width: 120px; height: 28px; padding: 0 8px; border: 1px solid transparent; border-radius: 6px; background: transparent; font-size: 13px; }
  .name:hover, .name:focus { border-color: var(--border); background: var(--field); }
  .path { flex: 1; min-width: 0; overflow: hidden; color: var(--muted); font-family: Consolas, "Cascadia Mono", monospace; font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
  .hint { flex: 1; color: var(--muted); font-size: 12px; }
  .badge { flex: none; padding: 1px 6px; border-radius: 4px; background: var(--sel); color: var(--sel-text); font-size: 11px; }
  .ib-space { width: 26px; flex: none; }
  .error { color: #c42b1c; }
  .add { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; max-width: 760px; }
  .text { height: 34px; padding: 0 10px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); font-size: 13px; }
  .text.grow { flex: 1; min-width: 240px; font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; }
  .check { display: flex; align-items: center; gap: 6px; font-size: 13px; }
  .shell { margin-top: 8px; }
  .reset { margin-top: 24px; }
  .confirm { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; max-width: 760px; padding: 12px 14px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); }
  .confirm span { flex: 1; min-width: 240px; }
  .link { padding: 4px 0; border: 0; background: transparent; color: var(--accent); font-size: 13px; }
  .link:hover { text-decoration: underline; }
</style>
