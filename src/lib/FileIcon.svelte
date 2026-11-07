<script lang="ts">
  import { devicePixelRatio } from "svelte/reactivity/window";
  import Icon from "./Icon.svelte";

  let { icon, size = 16 }: { icon: string | null; size?: number } = $props();
  const source = $derived(icon ? `http://icon.localhost/?key=${encodeURIComponent(icon)}&s=${Math.min(256, Math.max(16, Math.ceil(size * (devicePixelRatio.current ?? 1))))}` : "");
  let failed = $state("");
</script>

<span class="file-icon" style:width={`${size}px`} style:height={`${size}px`}>
  {#if source && failed !== source}
    <img src={source} width={size} height={size} alt="" decoding="async" draggable="false" onerror={() => (failed = source)} />
  {:else}
    <Icon name="file" {size} />
  {/if}
</span>

<style>
  .file-icon { display: inline-flex; flex: none; color: var(--muted); align-items: center; justify-content: center; }
  img { display: block; object-fit: contain; }
</style>
