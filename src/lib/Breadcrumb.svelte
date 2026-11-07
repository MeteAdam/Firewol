<script lang="ts">
  // DESIGN › Breadcrumb: leading segments that do not fit collapse into `…`; a chevron lists the
  // folders inside the segment before it; empty space (or Ctrl+L) edits the path.
  import { tick } from "svelte";
  import { api } from "./api";
  import type { Segment } from "./contract";
  import FolderIcon from "./FolderIcon.svelte";
  import Icon from "./Icon.svelte";

  let { segments, onopen, onedit, onrefresh, onerror }: {
    segments: Segment[];
    onopen: (path: string) => void;
    onedit: () => void;
    onrefresh: () => void;
    onerror: (message: string) => void;
  } = $props();

  let nav: HTMLElement;
  let crumbs: HTMLElement;
  /** How many leading segments sit in the `…` list. */
  let hidden = $state(0);
  let fitting = 0;
  type Drop = { anchor: HTMLElement; x: number; y: number; folders: Segment[]; current?: string; more: boolean; label: string };
  let drop = $state<Drop>();
  let list: HTMLElement | undefined = $state();

  /** Collapses as few leading segments as fit; runs before paint, so nothing flickers. */
  async function fit() {
    const run = ++fitting;
    for (let count = 0; count < segments.length; count++) {
      hidden = count;
      await tick();
      if (run !== fitting || !crumbs) return;
      // The current folder may shrink only once it is the last segment left.
      const current = crumbs.querySelector<HTMLElement>(".crumb.cur");
      if (crumbs.scrollWidth <= crumbs.clientWidth + 1 && (!current || current.scrollWidth <= current.clientWidth + 1)) return;
    }
    hidden = Math.max(0, segments.length - 1);
  }

  $effect(() => {
    void segments;
    drop = undefined;
    void fit();
  });

  $effect(() => {
    let width = nav.clientWidth;
    const observer = new ResizeObserver(() => {
      if (nav.clientWidth !== width) { width = nav.clientWidth; void fit(); }
    });
    observer.observe(nav);
    return () => observer.disconnect();
  });

  async function show(anchor: HTMLElement, label: string, load: () => Promise<{ folders: Segment[]; more: boolean; current?: string }>) {
    if (drop?.anchor === anchor) { drop = undefined; return; }
    const box = anchor.getBoundingClientRect();
    try {
      const { folders, more, current } = await load();
      drop = { anchor, label, folders, more, current, x: Math.max(8, Math.min(box.left, window.innerWidth - 288)), y: box.bottom + 4 };
      await tick();
      const items = [...(list?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
      (items.find((item) => item.classList.contains("on")) ?? items[0])?.focus();
      list?.querySelector(".on")?.scrollIntoView({ block: "nearest" });
    } catch (reason) {
      onerror(String(reason));
    }
  }

  function chevron(event: MouseEvent, index: number) {
    const parent = segments[index - 1];
    void show(event.currentTarget as HTMLElement, `Folders in ${parent.name}`, async () => ({ ...(await api.subfolders(parent.path)), current: segments[index]?.path }));
  }

  function ellipsis(event: MouseEvent) {
    void show(event.currentTarget as HTMLElement, "Hidden folders", async () => ({ folders: segments.slice(0, hidden), more: false }));
  }

  function choose(path: string) {
    drop = undefined;
    onopen(path);
  }

  function close(focusAnchor: boolean) {
    const anchor = drop?.anchor;
    drop = undefined;
    if (focusAnchor) anchor?.focus();
  }

  function listKey(event: KeyboardEvent) {
    const items = [...(list?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const to = (index: number) => items[(index + items.length) % items.length]?.focus();
    if (event.key === "ArrowDown") to(at + 1);
    else if (event.key === "ArrowUp") to(at - 1);
    else if (event.key === "Home") to(0);
    else if (event.key === "End") to(items.length - 1);
    else if (event.key === "Escape") close(true);
    else if (event.key === "Tab") { close(false); return; }
    else return;
    event.preventDefault();
    event.stopPropagation();
  }

  function outside(event: MouseEvent) {
    if (drop && !list?.contains(event.target as Node) && !drop.anchor.contains(event.target as Node)) drop = undefined;
  }
</script>

<svelte:window onmousedown={outside} onresize={() => (drop = undefined)} />

<nav class="field address" aria-label="Folder path" bind:this={nav}>
  <span class="muted icon"><Icon name="drive" /></span>
  <div class="crumbs" bind:this={crumbs}>
    {#if hidden > 0}
      <button class="crumb more" aria-label="Hidden folders" aria-haspopup="menu" aria-expanded={drop?.label === "Hidden folders"} title={segments.slice(0, hidden).map((segment) => segment.name).join(" › ")} onclick={ellipsis}>…</button>
    {/if}
    {#each segments as segment, index (segment.path)}
      {#if index >= hidden}
        {#if index > 0}
          <button class="chev" aria-label={`Folders in ${segments[index - 1].name}`} aria-haspopup="menu" aria-expanded={drop?.label === `Folders in ${segments[index - 1].name}`} onclick={(event) => chevron(event, index)}><Icon name="chevron" size={12} weight={2} /></button>
        {/if}
        <button class="crumb" class:cur={index === segments.length - 1} title={segment.path} onclick={() => onopen(segment.path)}>{segment.name}</button>
      {/if}
    {/each}
  </div>
  <button class="crumb-space" aria-label="Edit path (Ctrl+L)" onclick={onedit}></button>
  <button class="ib sm" aria-label="Refresh (F5)" title="Refresh (F5)" onclick={onrefresh}><Icon name="refresh" size={14} /></button>
</nav>

{#if drop}
  <div class="menu crumb-menu" role="menu" aria-label={drop.label} tabindex="-1" bind:this={list} style:left={`${drop.x}px`} style:top={`${drop.y}px`} onkeydown={listKey}>
    {#each drop.folders as folder (folder.path)}
      <button class="mi" class:on={folder.path === drop.current} role="menuitem" title={folder.path} onclick={() => choose(folder.path)}><FolderIcon /><span class="label">{folder.name}</span></button>
    {:else}
      <p class="muted empty">No folders</p>
    {/each}
    {#if drop.more}<p class="muted empty">Showing the first {drop.folders.length} folders</p>{/if}
  </div>
{/if}

<style>
  .address { flex: 1 1 360px; min-width: 0; margin-left: 6px; overflow: hidden; }
  .icon { display: flex; flex: none; }
  .crumbs { display: flex; align-items: center; flex: 0 1 auto; min-width: 0; overflow: hidden; }
  .crumb { flex: none; padding: 3px 6px; border: 0; border-radius: 6px; background: transparent; font-size: 13px; white-space: nowrap; }
  .crumb.cur { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; font-weight: 600; }
  .crumb.more { padding: 3px 8px; }
  .chev { display: flex; align-items: center; justify-content: center; flex: none; width: 18px; height: 24px; padding: 0; border: 0; border-radius: 6px; background: transparent; color: var(--muted); }
  .crumb:hover, .chev:hover, .chev[aria-expanded="true"] { background: var(--hover); }
  .crumb-space { flex-grow: 1; align-self: stretch; min-width: 24px; border: 0; background: transparent; }
  .crumb-menu { max-width: 280px; max-height: min(420px, calc(100vh - 120px)); overflow: auto; }
  .crumb-menu .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { margin: 0; padding: 6px 10px; font-size: 12px; }
</style>
