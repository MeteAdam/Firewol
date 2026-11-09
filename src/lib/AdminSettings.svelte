<script lang="ts">
  import type { ElevationView, SettingsPatch } from "./contract";
  import Icon from "./Icon.svelte";

  let { elevation, onsave }: { elevation: ElevationView; onsave: (patch: SettingsPatch) => Promise<void> } = $props();
  let confirmAny = $state(false);
  let error = $state("");
  let path = $state("");

  const MODES = [
    ["standard", "Standard", "Firewol never asks Windows to run a program as administrator."],
    ["allowlist", "Allowlisted executables", "Run as administrator appears only for the executables listed below."],
    ["all_executables", "Any executable", "Run as administrator appears for every .exe file. Windows still asks for consent each time."],
  ] as const;

  async function save(patch: SettingsPatch) {
    error = "";
    try {
      await onsave(patch);
      return true;
    } catch (reason) {
      error = String(reason);
      return false;
    }
  }

  function choose(mode: ElevationView["mode"]) {
    if (mode === elevation.mode) return;
    if (mode === "all_executables") confirmAny = true;
    else {
      confirmAny = false;
      void save({ elevation: { mode, executables: elevation.executables } });
    }
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    if (await save({ elevation: { mode: elevation.mode, executables: [...elevation.executables, path.trim()] } })) path = "";
  }
</script>

<h2>Administrator</h2>
<p class="muted">Controls when the context menu offers “Run as administrator”. Windows shows its own consent prompt (UAC) every time.</p>
<div class="modes" role="radiogroup" aria-label="Elevation mode">
  {#each MODES as [id, name, help] (id)}
    <button class="mode" class:on={elevation.mode === id} role="radio" aria-checked={elevation.mode === id} onclick={() => choose(id)}>
      <span class="dot" aria-hidden="true"></span>
      <span class="text"><strong>{name}</strong><span class="muted">{help}</span></span>
    </button>
  {/each}
</div>
{#if confirmAny}
  <div class="confirm" role="alertdialog" aria-label="Allow any executable">
    <strong>Allow any executable to run as administrator?</strong>
    <span>Every .exe file gets “Run as administrator”. A mistaken click on the consent prompt gives that program full control of this PC.</span>
    <div class="buttons">
      <button class="action primary" onclick={async () => { if (await save({ elevation: { mode: "all_executables", executables: elevation.executables } })) confirmAny = false; }}>Allow any executable</button>
      <button class="action" onclick={() => (confirmAny = false)}>Cancel</button>
    </div>
  </div>
{/if}

<h3>Allowlisted executables</h3>
<p class="muted">Each entry is pinned to the file's identity, so a replaced file stops matching until you add it again.</p>
<ul class="list" aria-label="Allowlisted executables">
  {#each elevation.executables as executable (executable)}
    <li>
      <span class="path" title={executable}>{executable}</span>
      <button class="ib sm" aria-label={`Remove ${executable}`} title="Remove" onclick={() => save({ elevation: { mode: elevation.mode, executables: elevation.executables.filter((item) => item !== executable) } })}><Icon name="trash" size={14} /></button>
    </li>
  {:else}
    <li class="muted empty">No executables yet.</li>
  {/each}
</ul>
<form class="add" onsubmit={add}>
  <input class="field-text" placeholder="C:\Tools\tool.exe" aria-label="Executable to allow" bind:value={path} spellcheck="false" required />
  <button class="action" disabled={!path.trim()}>Add executable</button>
</form>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  h2 { margin: 0 0 2px; font-size: 14px; font-weight: 600; }
  h3 { margin: 24px 0 2px; font-size: 13px; font-weight: 600; }
  p { margin: 0 0 12px; font-size: 12px; }
  .modes { display: flex; flex-direction: column; gap: 6px; max-width: 640px; }
  .mode { display: flex; align-items: flex-start; gap: 12px; padding: 10px 12px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); text-align: left; }
  .mode:hover { background: var(--hover); }
  .mode.on { outline: 2px solid var(--accent); outline-offset: -1px; }
  .dot { flex: none; width: 16px; height: 16px; margin-top: 1px; border: 2px solid var(--muted); border-radius: 50%; }
  .mode.on .dot { border: 5px solid var(--accent); }
  .text { display: flex; flex-direction: column; gap: 2px; font-size: 13px; }
  .text .muted { font-size: 12px; }
  .confirm { display: flex; flex-direction: column; gap: 8px; max-width: 640px; margin-top: 12px; padding: 12px 14px; border: 1px solid #c42b1c; border-radius: var(--radius-s); background: var(--surface-2); font-size: 13px; }
  .buttons { display: flex; gap: 8px; }
  .list { display: flex; flex-direction: column; gap: 2px; max-width: 640px; margin: 0 0 8px; padding: 0; list-style: none; }
  .list li { display: flex; align-items: center; gap: 8px; height: 34px; padding: 0 6px 0 10px; border-radius: var(--radius-s); }
  .list li:hover { background: var(--hover); }
  .list .empty:hover { background: none; }
  .path { flex: 1; min-width: 0; overflow: hidden; font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
  .add { display: flex; gap: 8px; max-width: 640px; }
  .field-text { flex: 1; height: 34px; padding: 0 10px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; }
  .error { margin-top: 8px; color: #c42b1c; }
</style>
