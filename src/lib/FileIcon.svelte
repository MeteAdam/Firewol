<script lang="ts">
  import { devicePixelRatio } from "svelte/reactivity/window";
  import Icon from "./Icon.svelte";
  import { iconSource } from "./icons.svelte";

  let { icon, size = 16, fallback = true, thumb = null, box }: {
    icon: string | null;
    size?: number;
    fallback?: boolean;
    /** A thumbnail key; it replaces the type icon once it has loaded. */
    thumb?: string | null;
    /** The space a thumbnail may fill. */
    box?: { width: number; height: number };
  } = $props();
  const scale = $derived(devicePixelRatio.current ?? 1);
  const source = $derived(iconSource(icon, size, scale));
  const preview = $derived(thumb && box ? iconSource(thumb, Math.max(box.width, box.height), scale) : "");
  let failed = $state("");
  let previewFailed = $state("");
  let previewReady = $state("");
  const wanted = $derived(!!preview && previewFailed !== preview);
  const showing = $derived(wanted && previewReady === preview);
</script>

<span class="file-icon" style:width={`${wanted ? box!.width : size}px`} style:height={`${wanted ? box!.height : size}px`}>
  {#if wanted}
    <img class="preview" class:ready={showing} src={preview} width={box!.width} height={box!.height} alt="" decoding="async" draggable="false" onload={() => (previewReady = preview)} onerror={() => (previewFailed = preview)} />
  {/if}
  {#if !showing}
    {#if source && failed !== source}
      <img src={source} width={size} height={size} alt="" decoding="async" draggable="false" onerror={() => (failed = source)} />
    {:else if fallback}
      <Icon name="file" {size} />
    {/if}
  {/if}
</span>

<style>
  .file-icon { display: inline-flex; flex: none; color: var(--muted); align-items: center; justify-content: center; }
  img { display: block; object-fit: contain; }
  .preview { display: none; border-radius: 4px; }
  .preview.ready { display: block; }
</style>
