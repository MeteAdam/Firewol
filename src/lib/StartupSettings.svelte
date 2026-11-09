<script lang="ts">
  import type { Pin, SettingsPatch, StartupView } from "./contract";

  let { startup, pins, current, onsave }: {
    startup: StartupView;
    pins: Pin[];
    /** The folder open in the active tab, offered as a one-click choice. */
    current: string;
    onsave: (patch: SettingsPatch) => Promise<void>;
  } = $props();

  let error = $state("");
  let typed = $state<string | null>(null);
  const folder = $derived(typed ?? (startup.kind === "folder" ? startup.path ?? "" : ""));
  const pinned = $derived(startup.kind === "pin" ? startup.path ?? "" : pins[0]?.path ?? "");

  async function save(next: StartupView) {
    error = "";
    try {
      await onsave({ startup: next });
      typed = null;
    } catch (reason) {
      error = String(reason);
    }
  }

  const chooseFolder = (event: SubmitEvent) => {
    event.preventDefault();
    if (folder.trim()) void save({ kind: "folder", path: folder.trim() });
  };
</script>

<h2>Startup</h2>
<p class="muted">What Firewol shows when it opens. A folder opened from another app or from Windows always opens that folder instead.</p>

<div class="modes" role="radiogroup" aria-label="When Firewol opens">
  <button class="mode" class:on={startup.kind === "last"} role="radio" aria-checked={startup.kind === "last"} onclick={() => save({ kind: "last", path: null })}>
    <span class="dot" aria-hidden="true"></span>
    <span class="text"><strong>Last used window</strong><span class="muted">Brings back the tabs of the window you closed last.</span></span>
  </button>

  <div class="mode column" class:on={startup.kind === "pin"}>
    <button class="plain" role="radio" aria-checked={startup.kind === "pin"} disabled={pins.length === 0} onclick={() => save({ kind: "pin", path: pinned })}>
      <span class="dot" aria-hidden="true"></span>
      <span class="text"><strong>A pinned folder</strong><span class="muted">{pins.length ? "Always opens the pin you select." : "Pin a folder to the sidebar first."}</span></span>
    </button>
    {#if pins.length}
      <select class="field-text" aria-label="Pinned folder" value={pinned} onchange={(event) => save({ kind: "pin", path: event.currentTarget.value })}>
        {#if startup.kind === "pin" && !pins.some((pin) => pin.path === pinned)}<option value={pinned}>{pinned}</option>{/if}
        {#each pins as pin (pin.path)}<option value={pin.path}>{pin.name} ({pin.path})</option>{/each}
      </select>
    {/if}
  </div>

  <div class="mode column" class:on={startup.kind === "folder"}>
    <div class="plain static">
      <span class="dot" aria-hidden="true"></span>
      <span class="text"><strong>A folder you choose</strong><span class="muted">Always opens this folder.</span></span>
    </div>
    <form class="row" onsubmit={chooseFolder}>
      <input class="field-text" placeholder="C:\Projects" aria-label="Folder to open" spellcheck="false" value={folder} oninput={(event) => (typed = event.currentTarget.value)} />
      <button class="action" disabled={!folder.trim()}>Use this folder</button>
      <button class="action" type="button" disabled={!current} onclick={() => save({ kind: "folder", path: current })}>Use open folder</button>
    </form>
  </div>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  h2 { margin: 0 0 2px; font-size: 14px; font-weight: 600; }
  p { margin: 0 0 12px; font-size: 12px; }
  .modes { display: flex; flex-direction: column; gap: 6px; max-width: 640px; }
  .mode { display: flex; align-items: flex-start; gap: 12px; padding: 10px 12px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); text-align: left; }
  button.mode:hover, .plain:not(.static):not(:disabled):hover { background: var(--hover); }
  .mode.on { outline: 2px solid var(--accent); outline-offset: -1px; }
  .column { flex-direction: column; gap: 8px; }
  .plain { display: flex; align-items: flex-start; gap: 12px; width: 100%; padding: 0; border: 0; background: transparent; text-align: left; }
  .dot { flex: none; width: 16px; height: 16px; margin-top: 1px; border: 2px solid var(--muted); border-radius: 50%; }
  .on .dot { border: 5px solid var(--accent); }
  .text { display: flex; flex-direction: column; gap: 2px; font-size: 13px; }
  .text .muted { font-size: 12px; }
  .row { display: flex; gap: 8px; width: 100%; }
  .field-text { flex: 1; width: 100%; min-width: 0; height: 34px; padding: 0 10px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--field); font-size: 12px; }
  .action { flex: none; }
  .error { margin-top: 8px; color: var(--danger); }
</style>
