<script lang="ts">
  import { api } from "./api";
  import type { DefaultAppsView } from "./contract";

  let status = $state<DefaultAppsView>();
  let busy = $state(false);
  let error = $state("");

  void api.defaultApps().then((view) => (status = view), (reason) => (error = String(reason)));

  const setFolders = (enabled: boolean) => change(api.setDefaultFolders(enabled));
  const setDialogs = (enabled: boolean) => change(api.setFileDialogs(enabled));

  async function updateCopies() {
    if (status?.folders) await setFolders(true);
    if (status?.file_dialogs) await setDialogs(true);
  }

  async function change(request: Promise<DefaultAppsView>) {
    busy = true;
    error = "";
    try {
      status = await request;
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }
</script>

<h2>Default apps</h2>
<p class="muted">Makes Windows use Firewol where it normally opens File Explorer. This applies to your account only, needs no administrator approval, and Explorer stays installed.</p>

<div class="card" aria-label="Folders and Win+E">
  <div class="text">
    <strong>Folders and Win+E</strong>
    <span class="muted">Folder double-clicks on the desktop, folders opened by other apps, and the Win+E shortcut open in Firewol.</span>
    {#if status?.other_handler && !status.folders}<span class="muted">Currently set to: <span class="mono">{status.other_handler}</span>. Turning this off later restores it.</span>{/if}
  </div>
  {#if status}
    <button class="action" class:primary={!status.folders} disabled={busy} aria-pressed={status.folders} onclick={() => setFolders(!status!.folders)}>{status.folders ? "Turn off" : "Turn on"}</button>
  {/if}
</div>
<div class="card spaced" aria-label="Open windows">
  <div class="text">
    <strong>Open windows in other programs</strong>
    <span class="muted">When a program asks you to choose a file or folder, Firewol opens to answer it. If Firewol cannot, the normal Windows window opens instead. Save windows stay as they are.</span>
  </div>
  {#if status}
    <button class="action" class:primary={!status.file_dialogs} disabled={busy} aria-pressed={status.file_dialogs} onclick={() => setDialogs(!status!.file_dialogs)}>{status.file_dialogs ? "Turn off" : "Turn on"}</button>
  {/if}
</div>
{#if status?.outdated}
  <div class="card notice">
    <span>The installed copy is older than this version of Firewol.</span>
    <button class="action" disabled={busy} onclick={updateCopies}>Update copy</button>
  </div>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

<h3>What stays with Explorer</h3>
<ul class="muted">
  <li>The desktop and the taskbar, which Explorer itself draws.</li>
  <li>The File Explorer icon pinned to the taskbar.</li>
  <li>Virtual folders such as the Recycle Bin, Control Panel and This PC.</li>
  <li>Save windows, 32-bit programs, and programs running as administrator.</li>
</ul>

<h3>Where it runs from</h3>
<p class="muted">Windows is pointed at a copy of Firewol in a fixed folder, so rebuilding or moving your own copy cannot break folder opening.</p>
{#if status}<p class="mono path" title={status.installed_path}>{status.installed_path}</p>{/if}

<h3>If Firewol cannot start</h3>
<p class="muted">This command puts everything back without opening the window:</p>
<p class="mono path">firewol-tauri.exe --restore-explorer</p>

<style>
  h2 { margin: 0 0 2px; font-size: 14px; font-weight: 600; }
  h3 { margin: 24px 0 2px; font-size: 13px; font-weight: 600; }
  p { margin: 0 0 12px; font-size: 12px; }
  ul { margin: 4px 0 0; padding-left: 18px; font-size: 12px; line-height: 20px; }
  .card { display: flex; align-items: center; justify-content: space-between; gap: 16px; max-width: 640px; padding: 12px 14px; border: var(--border-w) solid var(--border); border-radius: var(--radius-s); background: var(--surface-2); }
  .notice, .spaced { margin-top: 8px; }
  .notice { font-size: 13px; }
  .text { display: flex; flex-direction: column; gap: 2px; min-width: 0; font-size: 13px; }
  .text .muted { font-size: 12px; }
  .action { flex: none; }
  .mono { font-family: Consolas, "Cascadia Mono", monospace; font-size: 12px; overflow-wrap: anywhere; }
  .path { max-width: 640px; }
  .error { margin-top: 8px; color: var(--danger); }
</style>
