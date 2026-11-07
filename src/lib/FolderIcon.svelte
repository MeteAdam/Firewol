<script lang="ts">
  let { color = "var(--folder)", size = 16 }: { color?: string; size?: number } = $props();
</script>

<svg class="folder-icon" viewBox="0 0 24 24" width={size} height={size} aria-hidden="true" style:color={color}>
  <path
    d="M3 6.5A2.5 2.5 0 0 1 5.5 4h3.6c.6 0 1.2.3 1.6.7L12 6h6.5A2.5 2.5 0 0 1 21 8.5v9a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z"
    fill="currentColor"
  ></path>
  <path d="M3 9.5h18v8a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z" fill="#ffffff" fill-opacity=".22"></path>
</svg>

<style>
  .folder-icon { flex: none; }
</style>
