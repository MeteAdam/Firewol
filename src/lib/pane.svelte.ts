import { api } from "./api";
import type { Entry, FolderProgress, FolderView, IndexState, Page } from "./contract";

export const PAGE = 128;
// Pages farther than this from the newest page are dropped to bound memory.
const KEEP = PAGE * 6;

/** One folder view: navigation history, paged listing and folder search. */
export class Pane {
  // Listing data is replaced, never mutated, so it skips Svelte's per-field deep proxies.
  folder = $state.raw<FolderView>();
  count = $state(0);
  complete = $state(false);
  pages = $state.raw<Record<number, Entry[]>>({});
  selected = $state(-1);
  query = $state("");
  results = $state.raw<Page>();
  index = $state<"none" | "building" | "ready">("none");
  indexCount = $state(0);
  error = $state("");
  history = $state.raw<string[]>([]);
  future = $state.raw<string[]>([]);
  #pending = new Set<number>();
  #navigation = 0;
  #search = 0;

  get searching() {
    return this.query.trim().length > 0;
  }

  get rows() {
    return this.searching ? (this.results?.items.length ?? 0) : this.count;
  }

  entry(row: number): Entry | undefined {
    if (this.searching) return this.results?.items[row];
    return this.pages[Math.floor(row / PAGE) * PAGE]?.[row % PAGE];
  }

  async open(path: string, record = true) {
    const navigation = ++this.#navigation;
    let view: FolderView;
    try {
      view = await api.openFolder(path);
    } catch (reason) {
      if (navigation === this.#navigation) this.error = String(reason);
      return;
    }
    if (navigation !== this.#navigation) {
      void api.closeFolder(view.id);
      return;
    }
    const previous = this.folder;
    if (previous) {
      void api.closeFolder(previous.id);
      if (record) {
        this.history = [...this.history, previous.path];
        this.future = [];
      }
    }
    this.#pending.clear();
    this.folder = view;
    this.count = view.count;
    this.complete = view.complete;
    this.pages = { 0: view.page.items };
    this.selected = view.count > 0 ? 0 : -1;
    this.query = "";
    this.results = undefined;
    this.index = "none";
    this.indexCount = 0;
    this.error = "";
  }

  back() {
    const path = this.history.at(-1);
    if (!path || !this.folder) return;
    this.future = [this.folder.path, ...this.future];
    this.history = this.history.slice(0, -1);
    return this.open(path, false);
  }

  forward() {
    const [path, ...rest] = this.future;
    if (!path || !this.folder) return;
    this.history = [...this.history, this.folder.path];
    this.future = rest;
    return this.open(path, false);
  }

  up() {
    const parent = this.folder?.segments.at(-2);
    return parent ? this.open(parent.path) : undefined;
  }

  refresh() {
    return this.folder ? this.open(this.folder.path, false) : undefined;
  }

  progress(event: FolderProgress) {
    if (event.id !== this.folder?.id) return;
    this.count = event.count;
    this.complete = event.complete;
    if (event.error) this.error = event.error;
  }

  async loadPage(offset: number) {
    const folder = this.folder;
    if (!folder || this.pages[offset] || this.#pending.has(offset)) return;
    this.#pending.add(offset);
    try {
      const page = await api.listPage(folder.id, offset);
      if (this.folder?.id !== folder.id) return;
      // A short page that is still growing is fetched again after the next progress event.
      if (page.items.length < PAGE && page.hasMore) return;
      const next = { ...this.pages, [offset]: page.items };
      for (const key of Object.keys(next).map(Number)) {
        if (Math.abs(key - offset) > KEEP) delete next[key];
      }
      this.pages = next;
    } catch (reason) {
      this.error = String(reason);
    } finally {
      this.#pending.delete(offset);
    }
  }

  async startIndex() {
    const folder = this.folder;
    if (!folder || this.index !== "none") return;
    this.index = "building";
    try {
      this.indexed(await api.startIndex(folder.id));
    } catch (reason) {
      this.index = "none";
      this.error = String(reason);
    }
  }

  indexed(state: IndexState) {
    if (state.id !== this.folder?.id) return;
    if (state.error) {
      this.index = "none";
      this.error = state.error;
    } else if (state.ready) {
      const first = this.index !== "ready";
      this.index = "ready";
      this.indexCount = state.count;
      if (first && this.searching) void this.search(this.query);
    }
  }

  async search(text: string, offset = 0) {
    const id = ++this.#search;
    this.query = text;
    this.selected = -1;
    if (!text.trim()) {
      this.results = undefined;
      return;
    }
    if (this.index === "none") void this.startIndex();
    const folder = this.folder;
    if (!folder || this.index !== "ready") {
      this.results = undefined;
      return;
    }
    try {
      const page = await api.search(folder.id, text, offset);
      if (id === this.#search) {
        this.results = page;
        this.selected = page.items.length > 0 ? 0 : -1;
      }
    } catch (reason) {
      if (id === this.#search) this.error = String(reason);
    }
  }

  close() {
    if (this.folder) void api.closeFolder(this.folder.id);
  }
}
