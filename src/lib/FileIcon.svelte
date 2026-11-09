<script lang="ts">
  import { devicePixelRatio } from "svelte/reactivity/window";
  import Icon from "./Icon.svelte";
  import { iconSource } from "./icons.svelte";

  let { icon, size = 16, fallback = true }: { icon: string | null; size?: number; fallback?: boolean } = $props();
  const source = $derived(iconSource(icon, size, devicePixelRatio.current ?? 1));
  let failed = $state("");
</script>

<span class="file-icon" style:width={`${size}px`} style:height={`${size}px`}>
  {#if source && failed !== source}
    <img src={source} width={size} height={size} alt="" decoding="async" draggable="false" onerror={() => (failed = source)} />
  {:else if fallback}
    <Icon name="file" {size} />
  {/if}
</span>

<style>
  .file-icon { display: inline-flex; flex: none; color: var(--muted); align-items: center; justify-content: center; }
  img { display: block; object-fit: contain; }
</style>
