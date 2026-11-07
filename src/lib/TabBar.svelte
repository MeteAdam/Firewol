<script lang="ts">
  import { PaneSlot, type Tab } from "./tabs.svelte";
  import FolderIcon from "./FolderIcon.svelte";
  import Icon from "./Icon.svelte";
  let { slot, onselect, onclose, onnew, ondetach }: {
    slot: PaneSlot; onselect: (tab: Tab) => void; onclose: (tab: Tab) => void;
    onnew: () => void; ondetach: (tab: Tab) => void;
  } = $props();
  let strip: HTMLDivElement;
  let drag: { tab: Tab; x: number; y: number; moved: boolean } | undefined;
  let context = $state<Tab>();
  const label = (tab: Tab) => tab.kind === "settings" ? "Settings" : tab.pane.label;
  function down(event: PointerEvent, tab: Tab) {
    if (event.button !== 0 || (event.target as HTMLElement).closest(".close")) return;
    drag = { tab, x: event.clientX, y: event.clientY, moved: false };
    event.currentTarget instanceof HTMLElement && event.currentTarget.setPointerCapture(event.pointerId);
  }
  function move(event: PointerEvent) {
    if (!drag) return;
    drag.moved ||= Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > 8;
    if (!drag.moved) return;
    for (const element of strip.querySelectorAll<HTMLElement>("[data-tab]")) {
      const box = element.getBoundingClientRect();
      if (event.clientX >= box.left && event.clientX < box.right && event.clientY >= box.top && event.clientY < box.bottom) {
        slot.reorder(drag.tab.id, element.dataset.tab!);
        break;
      }
    }
  }
  function up(event: PointerEvent) {
    const current = drag;
    drag = undefined;
    if (!current) return;
    if (current.moved && (event.clientX < 0 || event.clientX > window.innerWidth || event.clientY < 0 || event.clientY > window.innerHeight)) ondetach(current.tab);
    else if (!current.moved) onselect(current.tab);
  }
</script>

<svelte:window onkeydown={(event) => { if (event.key === "Escape") { drag = undefined; context = undefined; } }} />

<div class="tabbar" bind:this={strip}>
  <div class="tabs" role="tablist" aria-label="Tabs">
    {#each slot.tabs as tab (tab.id)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="tab" class:on={slot.activeTab === tab.id} data-tab={tab.id}
        onpointerdown={(event) => down(event, tab)} onpointermove={move} onpointerup={up} onpointercancel={() => (drag = undefined)}
        onauxclick={(event) => { if (event.button === 1) { event.preventDefault(); onclose(tab); } }}
        oncontextmenu={(event) => { event.preventDefault(); context = tab; }}>
        <button class="label" role="tab" aria-selected={slot.activeTab === tab.id} tabindex={slot.activeTab === tab.id ? 0 : -1}
          onclick={(event) => { if (event.detail === 0) onselect(tab); }}
          onkeydown={(event) => {
            const index = slot.tabs.indexOf(tab);
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); onselect(slot.tabs[(index + (event.key === "ArrowLeft" ? -1 : 1) + slot.tabs.length) % slot.tabs.length]); }
            else if (event.key === "Enter" || event.key === " ") onselect(tab);
          }}>
          {#if tab.kind === "folder"}<FolderIcon />{:else}<Icon name="settings" />{/if}<span>{label(tab)}</span>
        </button>
        <button class="ib sm close" title={`Close ${label(tab)}`} aria-label={`Close ${label(tab)}`} onclick={() => onclose(tab)}><Icon name="close" size={12} /></button>
      </div>
    {/each}
  </div>
  <button class="ib" aria-label="New tab (Ctrl+T)" title="New tab (Ctrl+T)" onclick={onnew}><Icon name="plus" /></button>
  {#if context}<div class="tab-menu"><button class="mi" onclick={() => { const tab = context; context = undefined; if (tab) ondetach(tab); }}>Detach tab <span class="sc">Ctrl+Shift+N</span></button></div>{/if}
</div>

<style>
  .tabbar { position: relative; display: flex; align-items: center; min-width: 0; max-width: 100%; }
  .tabs { display: flex; gap: 4px; min-width: 0; overflow: auto; scrollbar-width: none; }
  .tab { display: flex; align-items: center; flex: 0 1 180px; min-width: 88px; height: 32px; padding: 0 4px 0 2px; border: var(--border-w) solid transparent; border-radius: var(--radius-s); color: var(--muted); touch-action: none; }
  .tab.on { background: var(--tab-active); color: var(--text); border-color: var(--border); box-shadow: var(--shadow-s); backdrop-filter: var(--blur); }
  .label { display: flex; align-items: center; gap: 8px; min-width: 0; height: 100%; padding: 0 8px; border: 0; background: transparent; font-size: 12px; }
  .label span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .close { visibility: hidden; }
  .tab:hover .close, .tab:focus-within .close, .tab.on .close { visibility: visible; }
  .tab-menu { position: absolute; top: 36px; z-index: 25; background: var(--surface-2); border: 1px solid var(--border); box-shadow: var(--shadow-pop); border-radius: var(--radius-s); padding: 6px; min-width: 260px; }
</style>
