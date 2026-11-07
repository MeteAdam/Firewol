<script lang="ts">
  import { onMount, type Snippet } from "svelte";
  let { title, onclose, children }: { title: string; onclose: () => void; children: Snippet } = $props();
  let dialog: HTMLDialogElement;
  onMount(() => { dialog.showModal(); return () => dialog.close(); });
</script>

<dialog bind:this={dialog} aria-label={title} oncancel={(event) => { event.preventDefault(); onclose(); }}>
  <header><h2>{title}</h2><button class="ib" aria-label="Close dialog" onclick={onclose}>×</button></header>
  {@render children()}
</dialog>

<style>
  dialog { width: min(520px, calc(100vw - 32px)); max-height: calc(100vh - 64px); padding: 24px; border: var(--border-w) solid var(--border); border-radius: var(--radius); background: var(--surface-2); color: var(--text); box-shadow: var(--shadow-pop); backdrop-filter: var(--blur); }
  dialog::backdrop { background: rgb(0 0 0 / .25); }
  header { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
  h2 { font-size: 20px; margin: 0; }
  :global(dialog .actions) { display: flex; justify-content: flex-end; gap: 8px; margin-top: 24px; }
  :global(.action) { padding: 8px 14px; border: 1px solid var(--border); border-radius: var(--radius-s); background: var(--field); }
  :global(.action.primary) { background: var(--accent); color: var(--on-accent); }
</style>
