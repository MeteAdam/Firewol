<script lang="ts">
  import { api } from "./api";
  import type { TransferStatus } from "./contract";
  import { formatCount, formatSize } from "./format";
  import Icon from "./Icon.svelte";

  let { status, ondismiss }: { status: TransferStatus; ondismiss: () => void } = $props();

  const W = 300;
  const H = 72;
  let open = $state(false);

  const done = $derived(status.phase === "finished");
  const percent = $derived(status.totalBytes > 0 ? Math.min(100, (100 * status.bytes) / status.totalBytes) : null);
  const peak = $derived(Math.max(1, ...status.samples));
  const verb = $derived(
    done ? (status.succeeded ? "Done" : "Stopped")
    : status.phase === "paused" ? "Paused"
    : status.phase === "cancelled" ? "Cancelling"
    : status.verifying > 0 ? "Verifying"
    : "Transferring",
  );
  const summary = $derived(
    done ? status.message ?? "Transfer finished"
    : [percent !== null ? `${Math.floor(percent)}%` : formatSize(status.bytes), status.phase === "running" && status.speed > 0 ? `${formatSize(status.speed)}/s` : ""].filter(Boolean).join(" · "),
  );
  const left = $derived.by(() => {
    if (done || status.phase !== "running" || status.speed <= 0 || status.totalBytes <= status.bytes) return "";
    const seconds = Math.ceil((status.totalBytes - status.bytes) / status.speed);
    return seconds < 60 ? `about ${seconds} s left` : `about ${Math.ceil(seconds / 60)} min left`;
  });
  // Area under the per-tick speed, newest on the right; 60 ticks fill the width.
  const area = $derived.by(() => {
    const samples = status.samples;
    if (samples.length === 0) return "";
    const step = W / 59;
    const x0 = W - (samples.length - 1) * step;
    const points = samples.map((value, index) => `${(x0 + index * step).toFixed(1)},${(H - (value / peak) * (H - 4)).toFixed(1)}`);
    return `M${x0.toFixed(1)},${H} L${points.join(" L")} L${W},${H} Z`;
  });
  const seconds = (ms: number) => (ms < 60_000 ? `${Math.round(ms / 1000)} s` : `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1000)} s`);
</script>

<svelte:window onmousedown={() => (open = false)} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="wrap" onmousedown={(event) => event.stopPropagation()} onkeydown={(event) => { if (event.key === "Escape" && open) { open = false; event.stopPropagation(); } }}>
  <span class="pill" class:done>
    <button class="open" aria-haspopup="dialog" aria-expanded={open} aria-label={`Transfer details: ${verb}, ${summary}`} title="Transfer details" onclick={() => (open = !open)}>
      <span class="accent icon"><Icon name={done ? (status.succeeded ? "check" : "info") : "transfer"} size={14} /></span>
      <span class="text">{verb} · {summary}</span>
    </button>
    {#if done}
      <button class="ib xs" aria-label="Dismiss transfer result" onclick={() => { open = false; ondismiss(); }}><Icon name="close" size={12} /></button>
    {:else}
      <button class="ib xs" aria-label={status.phase === "paused" ? "Resume transfer" : "Pause transfer"} disabled={status.phase === "cancelled"} onclick={() => api.pauseTransfer()}><Icon name={status.phase === "paused" ? "play" : "pause"} size={12} /></button>
      <button class="ib xs" aria-label="Cancel transfer" disabled={status.phase === "cancelled"} onclick={() => api.cancelTransfer()}><Icon name="close" size={12} /></button>
    {/if}
  </span>
  {#if open}
    <div class="pop" role="dialog" aria-label="Transfer details">
      <header>
        <strong>{done ? "Last transfer" : verb}</strong>
        <span class="muted">{seconds(status.elapsedMs)}</span>
      </header>
      <figure aria-label={`Transfer speed, peak ${formatSize(peak)}/s`}>
        <svg viewBox={`0 0 ${W} ${H}`} width={W} height={H} aria-hidden="true">
          <line x1="0" y1={H - 0.5} x2={W} y2={H - 0.5} class="axis" />
          <line x1="0" y1="4" x2={W} y2="4" class="grid" />
          {#if area}<path d={area} class="area" />{/if}
        </svg>
        <figcaption class="muted"><span>Peak {formatSize(peak)}/s</span><span>last 15 s</span></figcaption>
      </figure>
      {#if percent !== null}
        <div class="bar" role="progressbar" aria-label="Bytes copied" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.floor(percent)}><span style:width={`${percent}%`}></span></div>
      {/if}
      <dl>
        <dt>Data</dt><dd>{formatSize(status.bytes)}{status.totalBytes > 0 ? ` of ${formatSize(status.totalBytes)}` : ""}</dd>
        <dt>Items</dt><dd>{formatCount(status.finished)}{status.totalItems > 0 ? ` of ${formatCount(status.totalItems)}` : ""}{status.verifying > 0 ? ` · ${status.verifying} verifying` : ""}</dd>
        {#if !done}<dt>Speed</dt><dd>{formatSize(status.speed)}/s{left ? ` · ${left}` : ""}</dd>{/if}
      </dl>
      {#if done && status.message}<p class="result" class:failed={!status.succeeded}>{status.message}</p>{/if}
      {#if !done}
        <div class="actions">
          <button class="action" disabled={status.phase === "cancelled"} onclick={() => api.pauseTransfer()}>{status.phase === "paused" ? "Resume" : "Pause"}</button>
          <button class="action" disabled={status.phase === "cancelled"} onclick={() => api.cancelTransfer()}>Cancel</button>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .wrap { position: relative; display: flex; min-width: 0; }
  .pill { display: flex; align-items: center; gap: 2px; min-width: 0; height: 26px; padding: 0 4px 0 2px; border: var(--border-w) solid var(--border); border-radius: 999px; background: var(--surface); box-shadow: var(--shadow-s); backdrop-filter: var(--blur); color: var(--text); }
  .pill.done { color: var(--muted); }
  .open { display: flex; align-items: center; gap: 6px; min-width: 0; max-width: 420px; height: 22px; padding: 0 8px; border: 0; border-radius: 999px; background: transparent; font-size: 12px; }
  .open:hover { background: var(--hover); }
  .text { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .icon { display: flex; flex: none; }
  .accent { color: var(--accent); }
  .pop { position: absolute; right: 0; bottom: calc(100% + 8px); z-index: 25; display: flex; flex-direction: column; gap: 10px; width: 332px; padding: 14px 16px; border: var(--border-w) solid var(--border); border-radius: var(--radius); background: var(--surface-2); box-shadow: var(--shadow-pop); backdrop-filter: var(--blur); color: var(--text); font-size: 13px; }
  header { display: flex; justify-content: space-between; align-items: baseline; }
  figure { margin: 0; }
  svg { display: block; }
  .area { fill: color-mix(in srgb, var(--accent) 28%, transparent); stroke: var(--accent); stroke-width: 1.5; stroke-linejoin: round; }
  .axis { stroke: var(--sep); }
  .grid { stroke: var(--sep); stroke-dasharray: 3 4; }
  figcaption { display: flex; justify-content: space-between; margin-top: 4px; font-size: 11px; }
  .bar { height: 6px; overflow: hidden; border-radius: 3px; background: var(--sep); }
  .bar span { display: block; height: 100%; background: var(--accent); }
  dl { display: grid; grid-template-columns: 56px 1fr; gap: 4px 8px; margin: 0; }
  dt { color: var(--muted); }
  dd { margin: 0; }
  .result { margin: 0; overflow-wrap: anywhere; }
  .result.failed::before { content: "⚠ "; color: var(--danger); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
  .actions .action { padding: 5px 12px; font-size: 13px; }
</style>
