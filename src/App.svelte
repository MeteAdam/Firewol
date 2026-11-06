<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, tick, untrack } from "svelte";

  interface Entry {
    name: string;
    directory: boolean;
  }

  interface Page {
    items: Entry[];
    has_more: boolean;
  }

  interface Session {
    fixture: { entries: number; index_rows: number };
    benchmark: boolean;
  }

  const PAGE = 128;
  const ROW = 28;
  const OVERSCAN = 8;
  let session = $state<Session>();
  let error = $state("");
  let query = $state("");
  let results = $state<Page>();
  let searchOffset = $state(0);
  let requestId = 0;
  let pages = $state<Record<number, Entry[]>>({});
  const pending = new Set<number>();
  let viewport: HTMLDivElement;
  let height = $state(0);
  let scroll = $state(0);
  const searching = $derived(query.trim().length > 0);
  const first = $derived(Math.max(0, Math.floor(scroll / ROW) - OVERSCAN));
  const end = $derived(
    Math.min(session?.fixture.entries ?? 0, first + Math.ceil(height / ROW) + 2 * OVERSCAN),
  );
  const visible = $derived(
    Array.from({ length: Math.max(0, end - first) }, (_, index) => {
      const row = first + index;
      return { row, item: pages[Math.floor(row / PAGE) * PAGE]?.[row % PAGE] };
    }),
  );

  $effect(() => {
    if (searching || !session) return;
    const start = Math.floor(first / PAGE) * PAGE;
    const stop = end;
    untrack(() => {
      for (let offset = start; offset < stop; offset += PAGE) {
        void loadPage(offset).catch(showError);
      }
    });
  });

  async function loadPage(offset: number) {
    if (pages[offset] || pending.has(offset)) return;
    pending.add(offset);
    try {
      const response = await invoke<Page>("list_dir", { request: { offset, limit: PAGE } });
      const center = Math.floor(scroll / ROW / PAGE) * PAGE;
      if (Math.abs(offset - center) > PAGE * 4) return;
      const next = { ...pages, [offset]: response.items };
      for (const key of Object.keys(next)) {
        if (Math.abs(Number(key) - center) > PAGE * 4) delete next[Number(key)];
      }
      pages = next;
    } finally {
      pending.delete(offset);
    }
  }

  function showError(reason: unknown) {
    error = String(reason);
  }

  async function runSearch(text: string, offset = 0) {
    const id = ++requestId;
    query = text;
    searchOffset = offset;
    results = undefined;
    viewport.scrollTop = 0;
    if (!text.trim()) return;
    try {
      const response = await invoke<Page>("search", {
        request: { text, page: { offset, limit: PAGE } },
      });
      if (id === requestId) results = response;
    } catch (reason) {
      if (id === requestId) throw reason;
    }
  }

  function frame() {
    return new Promise<number>((resolve) => requestAnimationFrame(resolve));
  }

  async function painted() {
    await tick();
    await frame();
    await frame();
  }

  async function benchmark(firstListMs: number, entries: number) {
    const times: number[] = [];
    for (let sample = 0; sample < 25; sample++) {
      const start = performance.now();
      await runSearch(["ré", "cord", "port", "résumé", "record-0000"][sample % 5]);
      if (!results?.items.length) throw new Error("Expected search results in the benchmark fixture");
      await painted();
      times.push(performance.now() - start);
    }
    await runSearch("");
    await painted();
    const start = await frame();
    let now = start;
    let frames = 0;
    while (now - start < 3000) {
      viewport.scrollTop = Math.min(entries * ROW, (now - start) * 10);
      now = await frame();
      frames++;
    }
    viewport.scrollTop = 0;
    times.sort((a, b) => a - b);
    await invoke("save_benchmark", {
      report: {
        first_list_ms: firstListMs,
        search_p95_ms: times[Math.ceil(times.length * 0.95) - 1],
        scroll_fps: (1000 * frames) / (now - start),
        search_samples: times.length,
      },
    });
  }

  onMount(() => {
    async function open() {
      const start = performance.now();
      const info = await invoke<Session>("session");
      await loadPage(0);
      session = info;
      await painted();
      if (info.benchmark) await benchmark(performance.now() - start, info.fixture.entries);
    }
    void open().catch(showError);
  });
</script>

<header>
  <div class="identity"><span class="mark" aria-hidden="true"></span> Firewol</div>
  <span class="badge">M0 / Windows</span>
</header>
<main>
  <div class="toolbar">
    <div>
      <div class="eyebrow">Synthetic NTFS fixture</div>
      <h1>{searching ? "Search results" : "Files"}</h1>
    </div>
    <input
      type="search"
      aria-label="Search indexed names"
      placeholder="Search indexed names"
      maxlength="256"
      value={query}
      disabled={!session}
      oninput={(event) => void runSearch(event.currentTarget.value).catch(showError)}
    />
  </div>
  {#if error}
    <p class="message" role="alert">{error}</p>
  {/if}
  <div class="heading">Name</div>
  <div
    class="viewport"
    bind:this={viewport}
    bind:clientHeight={height}
    onscroll={() => (scroll = viewport.scrollTop)}
  >
    {#if searching}
      {#each results?.items ?? [] as item}
        <div class="row"><span class="file-icon" aria-hidden="true"></span>{item.name}</div>
      {/each}
      {#if results && results.items.length === 0}
        <p class="message">No matching names.</p>
      {/if}
    {:else if session}
      <div style:height={`${session.fixture.entries * ROW}px`}>
        <div style:transform={`translateY(${first * ROW}px)`}>
          {#each visible as { row, item } (row)}
            <div class="row">
              {#if item}
                <span class="file-icon" class:folder={item.directory} aria-hidden="true"></span>
                {item.name}
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>
  <footer>
    {#if session}
      {session.fixture.entries.toLocaleString()} files
      <span class="separator">·</span>
      {session.fixture.index_rows.toLocaleString()} indexed names
      {#if searching}
        <div class="paging">
          <button
            disabled={searchOffset === 0 || !results}
            onclick={() => void runSearch(query, searchOffset - PAGE).catch(showError)}
          >Previous</button>
          <button
            disabled={!results?.has_more}
            onclick={() => void runSearch(query, searchOffset + PAGE).catch(showError)}
          >Next</button>
        </div>
      {/if}
    {:else}
      {error ? "Fixture unavailable" : "Opening fixture…"}
    {/if}
  </footer>
</main>

<style>
  :global(*) { box-sizing: border-box; }
  :global(body) {
    margin: 0;
    background: #17191d;
    color: #e9ebee;
    font: 14px "Segoe UI", system-ui, sans-serif;
    color-scheme: dark;
  }
  header, footer, .toolbar, .identity, .row, .paging { display: flex; align-items: center; }
  header {
    height: 52px;
    padding: 0 24px;
    justify-content: space-between;
    background: #202227;
    border-bottom: 1px solid #34363c;
  }
  .identity { gap: 10px; font-weight: 650; font-size: 16px; }
  .mark { width: 11px; height: 11px; background: #fa9a62; border-radius: 3px; }
  .badge, .eyebrow, footer { color: #aeb3bd; font-size: 12px; }
  main { display: flex; flex-direction: column; height: calc(100vh - 52px); padding: 0 24px; }
  .toolbar { padding: 20px 0; gap: 16px; justify-content: space-between; }
  h1 { margin: 3px 0 0; font-size: 22px; font-weight: 600; }
  input, button {
    font: inherit;
    color: inherit;
    background: #272a30;
    border: 1px solid #464951;
    border-radius: 5px;
    padding: 7px 10px;
  }
  input { width: min(320px, 52%); }
  :focus-visible { outline: 2px solid #fa9a62; outline-offset: 2px; }
  .heading {
    padding: 6px 12px;
    background: #24272d;
    color: #bec2ca;
    border-top: 1px solid #34363c;
    border-bottom: 1px solid #34363c;
  }
  .viewport { flex: 1; min-height: 0; overflow: auto; overflow-anchor: none; }
  .row { height: 28px; gap: 10px; padding: 0 12px; white-space: nowrap; border-bottom: 1px solid #24262b; }
  .row:hover { background: #292c32; }
  .file-icon { width: 11px; height: 13px; border: 1px solid #888f9c; border-radius: 2px; }
  .folder { border-color: #fa9a62; }
  .message { color: #faad87; padding: 0 12px; }
  footer { gap: 8px; height: 47px; border-top: 1px solid #34363c; }
  .separator { color: #767c87; }
  .paging { gap: 6px; margin-left: auto; }
  button:disabled { opacity: 0.55; }
</style>
