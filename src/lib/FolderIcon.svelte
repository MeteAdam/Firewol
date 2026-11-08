<script lang="ts">
  import { devicePixelRatio } from "svelte/reactivity/window";
  import { iconSource } from "./icons.svelte";

  // A folder style's glyph comes first, drawn in the style's colour (as in Win32); then the
  // folder's own Windows icon; the theme's folder shape is the default and the fallback.
  let { color = "var(--folder)", glyph = "", size = 16, icon = null }: { color?: string; glyph?: string; size?: number; icon?: string | null } = $props();
  const source = $derived(glyph ? "" : iconSource(icon, size, devicePixelRatio.current ?? 1));
  let failed = $state("");
</script>

{#if glyph}
  <span class="folder-glyph" aria-hidden="true" style:color={color} style:width={`${size}px`} style:height={`${size}px`} style:font-size={`${Math.round(size * 0.9)}px`}>{glyph}</span>
{:else if source && failed !== source}
  <img class="folder-image" src={source} width={size} height={size} alt="" decoding="async" draggable="false" onerror={() => (failed = source)} />
{:else}
  <svg class="folder-icon" viewBox="0 0 24 24" width={size} height={size} aria-hidden="true" style:color={color}>
    <path
      d="M3 6.5A2.5 2.5 0 0 1 5.5 4h3.6c.6 0 1.2.3 1.6.7L12 6h6.5A2.5 2.5 0 0 1 21 8.5v9a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z"
      fill="currentColor"
    ></path>
    <path d="M3 9.5h18v8a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z" fill="#ffffff" fill-opacity=".22"></path>
  </svg>
{/if}

<style>
  .folder-icon { flex: none; }
  .folder-image { display: block; flex: none; object-fit: contain; }
  .folder-glyph { display: inline-flex; align-items: center; justify-content: center; flex: none; overflow: hidden; line-height: 1; font-family: "Segoe UI Symbol", "Segoe UI Emoji", var(--font, "Segoe UI"), sans-serif; white-space: nowrap; }
</style>
