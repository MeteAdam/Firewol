import { api } from "./api";
import type {
  Entry,
  FolderProgress,
  FolderView,
  IndexState,
  ItemRef,
  Navigation,
  Page,
  SearchCount,
  SearchFilters,
  SearchRequest,
  Step,
  SortView,
  SortColumn,
  VolumeState,
} from "./contract";
import { hasTags } from "./styles";
import { volumes } from "./volumes.svelte";

export const PAGE = 128;
export const itemKey = (item: ItemRef) => `${item.volume}:${item.index}:${item.path}`;
// Pages farther than this from the newest page are dropped to bound memory.
const KEEP = PAGE * 6;
// Typing waits this long before searching, so a burst of keys runs one query.
const DEBOUNCE_MS = 150;
/** Ctrl+A selects at most this many rows; file operations accept 32 items at a time anyway. */
export const SELECT_ALL_LIMIT = 10_000;
const tagged = (items: Entry[]) => items.some((entry) => hasTags(entry.style));
const prune = (pages: Record<number, Entry[]>, offset: number) => {
  for (const key of Object.keys(pages).map(Number)) {
    if (Math.abs(key - offset) > KEEP) delete pages[key];
  }
  return pages;
};

export const NO_FILTERS: SearchFilters = { extension: null, minBytes: null, maxBytes: null, modifiedAfter: null, modifiedBefore: null };
export const filterCount = (filters: SearchFilters) => Object.values(filters).filter((value) => value !== null).length;

/** One folder view. Navigation history lives in the core; this keeps what the list renders. */
export class Pane {
  readonly id: string;
  path = $state("");
  label = $state("Folder");
  scroll = $state(0);
  // Listing data is replaced, never mutated, so it skips Svelte's per-field deep proxies.
  folder = $state.raw<FolderView>();
  /** The open folder's identity; kept while the tab is suspended so its alias still shows. */
  identity = $state.raw<ItemRef | null>(null);
  /** The open folder's own Windows icon; kept while the tab is suspended, like its identity. */
  icon = $state<string | null>(null);
  count = $state(0);
  complete = $state(false);
  pages = $state.raw<Record<number, Entry[]>>({});
  canBack = $state(false);
  canForward = $state(false);
  /** The focused row; selection holds every selected row. */
  selected = $state(-1);
  selection = $state.raw<Map<string, Entry>>(new Map());
  sort = $state.raw<SortView>({ column: "name", descending: false });
  sortMs = $state(0);
  pageVersion = $state(0);
  renaming = $state<number | null>(null);
  query = $state("");
  regex = $state(false);
  filters = $state.raw<SearchFilters>(NO_FILTERS);
  /** The drive letter while this tab searches a whole NTFS volume; null searches the folder. */
  volume = $state<string | null>(null);
  /** Result pages by offset; the rows grow while pages report more and settle at the count. */
  resultPages = $state.raw<Record<number, Entry[]>>({});
  resultRows = $state(0);
  resultCount = $state.raw<SearchCount | null>(null);
  /** Bumped when a new search replaces the results, so the list returns to the top. */
  searchVersion = $state(0);
  searchError = $state("");
  index = $state<"none" | "building" | "ready">("none");
  indexCount = $state(0);
  error = $state("");
  // Set once a listed page shows a tagged folder and kept until the next folder, so the Tags
  // column does not come and go while scrolling.
  #tagged = $state(false);
  #anchor = -1;
  #pending = new Map<number, number>();
  #revision = 0;
  #selecting = 0;
  #navigation = 0;
  #search = 0;
  #resultPending = new Set<number>();
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor(id: string) {
    this.id = id;
  }

  get searching() {
    return this.query.trim().length > 0;
  }

  /** Whether the index the current scope searches has finished its first pass. */
  get searchReady() {
    return this.volume ? !!volumes.states[this.volume]?.ready : this.index === "ready";
  }

  get tagged() {
    return this.searching ? Object.values(this.resultPages).some(tagged) : this.#tagged;
  }

  get rows() {
    return this.searching ? this.resultRows : this.count;
  }

  entry(row: number): Entry | undefined {
    const pages = this.searching ? this.resultPages : this.pages;
    return pages[Math.floor(row / PAGE) * PAGE]?.[row % PAGE];
  }

  #loaded(offset: number) {
    return !!(this.searching ? this.resultPages : this.pages)[offset];
  }

  /** Loaded entries in the selection, in row order. */
  selectedEntries(): Entry[] {
    return [...this.selection.values()];
  }

  isSelected(row: number) {
    const id = this.entry(row)?.id;
    return !!id && this.selection.has(itemKey(id));
  }

  get focusedEntry() { return this.isSelected(this.selected) ? this.entry(this.selected) : undefined; }

  selectedRefs(): ItemRef[] {
    return this.selectedEntries().flatMap((entry) => (entry.id ? [entry.id] : []));
  }

  async select(row: number, mode: "single" | "toggle" | "range" = "single") {
    if (this.rows === 0) return;
    row = Math.max(0, Math.min(this.rows - 1, row));
    const selecting = ++this.#selecting;
    const revision = this.#revision;
    if (!this.entry(row)) await this.loadPage(Math.floor(row / PAGE) * PAGE);
    if (selecting !== this.#selecting || revision !== this.#revision) return;
    const entry = this.entry(row);
    if (!entry?.id) return;
    const key = itemKey(entry.id);
    if (mode === "toggle") {
      const next = new Map(this.selection);
      if (next.has(key)) next.delete(key);
      else next.set(key, entry);
      this.selection = next;
      this.#anchor = row;
    } else if (mode === "range" && this.#anchor >= 0) {
      const [from, to] = this.#anchor < row ? [this.#anchor, row] : [row, this.#anchor];
      const next = new Map<string, Entry>();
      for (let offset = Math.floor(from / PAGE) * PAGE; offset <= to; offset += PAGE) {
        if (!this.#loaded(offset)) await this.loadPage(offset);
        if (selecting !== this.#selecting || revision !== this.#revision) return;
        for (let index = Math.max(from, offset); index <= Math.min(to, offset + PAGE - 1); index++) {
          const item = this.entry(index);
          if (item?.id) next.set(itemKey(item.id), item);
        }
      }
      this.selection = next;
    } else {
      this.selection = new Map([[key, entry]]);
      this.#anchor = row;
    }
    this.selected = row;
  }

  /**
   * Selects every row (Ctrl+A), up to `limit` rows so a 100k-item folder does not load every page.
   * Returns whether every row is selected.
   */
  async selectAll(limit = SELECT_ALL_LIMIT) {
    const last = Math.min(this.rows, limit) - 1;
    if (last < 0) return true;
    const focused = this.selected;
    this.#anchor = 0;
    await this.select(last, "range");
    if (focused >= 0 && focused <= last) this.selected = focused;
    return this.rows <= limit;
  }

  open(path: string) {
    return this.#go("new", path);
  }

  back() {
    return this.canBack ? this.#go("back") : undefined;
  }

  forward() {
    return this.canForward ? this.#go("forward") : undefined;
  }

  up() {
    const parent = this.folder?.segments.at(-2);
    return parent ? this.open(parent.path) : undefined;
  }

  refresh() {
    return this.folder ? this.#go("reload", this.folder.path) : undefined;
  }

  async #go(step: Step, path: string | null = null) {
    const navigation = ++this.#navigation;
    let result: Navigation;
    try {
      result = await api.navigate(this.id, path, step, this.sort);
    } catch (reason) {
      if (navigation === this.#navigation && !String(reason).includes("newer navigation")) {
        this.error = String(reason);
      }
      return;
    }
    if (navigation !== this.#navigation) return;
    const view = result.folder;
    const reload = step === "reload" && view.path === (this.folder?.path ?? this.path);
    // The core reloads an open folder in place; its live index survives under the same id.
    const sameFolder = reload && view.id === this.folder?.id;
    this.#revision++;
    this.pageVersion++;
    this.#pending.clear();
    this.folder = view;
    this.identity = view.item;
    this.icon = view.icon;
    this.#tagged = tagged(view.page.items);
    this.path = view.path;
    this.label = view.segments.at(-1)?.name ?? view.path;
    this.count = view.count;
    this.complete = view.complete;
    this.sortMs = view.sort_ms;
    this.pages = { 0: view.page.items };
    this.canBack = result.canBack;
    this.canForward = result.canForward;
    this.renaming = null;
    this.error = "";
    if (!reload) {
      this.scroll = 0;
      clearTimeout(this.#timer);
      this.#search++;
      this.query = "";
      this.#clearResults();
    }
    if (!sameFolder) {
      this.index = "none";
      this.indexCount = 0;
    }
    if (reload) await this.remap();
    else if (view.count > 0) void this.select(0);
    else {
      this.selected = -1;
      this.selection = new Map();
    }
    // Enumeration may finish before the navigation response reaches this pane.
    void api.folderProgress(view.id).then(event => this.progress(event)).catch(() => {});
    if (reload && this.searching) void this.search(this.query);
  }

  progress(event: FolderProgress) {
    if (event.id !== this.folder?.id) return;
    this.count = event.count;
    const finished = event.complete && !this.complete;
    this.complete = event.complete;
    this.sortMs = event.sort_ms;
    if (event.error) this.error = event.error;
    if (finished && !this.searching) void this.reloadPages();
  }

  async sortBy(column: SortColumn) {
    this.renaming = null;
    this.sort = { column, descending: this.sort.column === column ? !this.sort.descending : false };
    await this.reloadPages();
    if (this.folder) this.progress(await api.folderProgress(this.folder.id));
  }

  async reloadPages() {
    this.#revision++;
    this.pageVersion++;
    this.pages = {};
    this.#pending.clear();
    await this.loadPage(0);
    await this.remap();
  }

  async remap() {
    if (!this.folder || this.searching) return;
    const revision = this.#revision;
    const items = this.selectedEntries();
    const positions = await api.locateSelection(this.folder.id, this.selectedRefs());
    if (revision !== this.#revision) return;
    this.selection = new Map(items.flatMap((entry, index) => positions[index] !== null && entry.id ? [[itemKey(entry.id), entry] as const] : []));
    this.selected = positions.find((row): row is number => row !== null) ?? -1;
    this.#anchor = this.selected;
    if (this.selected >= 0) await this.loadPage(Math.floor(this.selected / PAGE) * PAGE);
  }

  async loadPage(offset: number) {
    if (this.searching) return this.#loadResults(offset);
    const folder = this.folder;
    if (!folder || this.pages[offset] || this.#pending.has(offset)) return;
    const revision = this.#revision;
    const sort = this.sort;
    this.#pending.set(offset, revision);
    try {
      const page = await api.listPage(folder.id, offset, sort);
      if (this.folder?.id !== folder.id || revision !== this.#revision) return;
      if (page.sorted && !this.complete) {
        this.progress(await api.folderProgress(folder.id));
        if (revision !== this.#revision) return;
      }
      // A short page that is still growing is fetched again after the next progress event.
      if (page.items.length < PAGE && page.hasMore) return;
      if (tagged(page.items)) this.#tagged = true;
      this.pages = prune({ ...this.pages, [offset]: page.items }, offset);
    } catch (reason) {
      this.error = String(reason);
    } finally {
      if (this.#pending.get(offset) === revision) this.#pending.delete(offset);
    }
  }

  /** Re-reads the loaded pages (or results) in place so rows show changed folder styles. */
  async restyle() {
    const folder = this.folder;
    if (!folder) return;
    if (this.searching) {
      const search = this.#search;
      if (!this.searchReady) return;
      const offsets = Object.keys(this.resultPages).map(Number);
      const pages = await Promise.all(offsets.map((offset) => {
        const request = this.#request(offset);
        return request ? api.search(request).catch(() => undefined) : undefined;
      }));
      if (search !== this.#search) return;
      const next = { ...this.resultPages };
      offsets.forEach((offset, index) => { const page = pages[index]; if (page) next[offset] = page.items; });
      this.resultPages = next;
      return;
    }
    const revision = this.#revision;
    const offsets = Object.keys(this.pages).map(Number);
    try {
      const pages = await Promise.all(offsets.map((offset) => api.listPage(folder.id, offset, this.sort)));
      if (revision !== this.#revision || this.folder?.id !== folder.id) return;
      const next = { ...this.pages };
      offsets.forEach((offset, index) => {
        const page = pages[index];
        if (page.items.length === PAGE || !page.hasMore) next[offset] = page.items;
      });
      this.pages = next;
      this.#tagged = Object.values(next).some(tagged);
    } catch (reason) {
      this.error = String(reason);
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
      if (first && this.searching && !this.volume) void this.search(this.query);
    }
  }

  /** Follows the shared NTFS index this tab searches: shows results once it is ready and
   *  returns to folder search when it stops or Windows refuses the helper. */
  volumeChanged(state: VolumeState) {
    if (state.drive !== this.volume) return;
    if (!state.running) {
      this.volume = null;
      if (this.searching) void this.search(this.query);
    } else if (state.ready && this.searching && this.resultRows === 0) {
      void this.search(this.query);
    }
  }

  /** Typing: updates the draft now and searches once the keys pause. */
  input(text: string) {
    this.query = text;
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => void this.search(this.query), DEBOUNCE_MS);
  }

  setRegex(regex: boolean) {
    this.regex = regex;
    this.#again();
  }

  setFilters(filters: SearchFilters) {
    this.filters = filters;
    this.#again();
  }

  setVolume(drive: string | null) {
    this.volume = drive;
    this.#again();
  }

  #again() {
    if (this.searching) this.input(this.query);
  }

  #request(offset: number): SearchRequest | undefined {
    const folder = this.folder;
    if (!folder) return undefined;
    return {
      id: folder.id,
      text: this.query,
      regex: this.regex,
      filters: this.filters,
      scope: this.volume ? { kind: "volume", drive: this.volume } : { kind: "folder" },
      offset,
    };
  }

  #clearResults() {
    this.resultPages = {};
    this.resultRows = 0;
    this.resultCount = null;
    this.searchError = "";
    this.#resultPending.clear();
  }

  /** New results (or the listing again) start at the top. */
  #toTop() {
    this.scroll = 0;
    this.searchVersion++;
  }

  /** Grows the rows past the loaded pages while the count is unknown or capped. */
  #grow(offset: number, page: Page) {
    if (this.resultCount && !this.resultCount.more) return;
    this.resultRows = Math.max(this.resultRows, offset + page.items.length + (page.hasMore ? PAGE : 0));
  }

  /** Searches now (the field debounces through `input`) and shows the first page. */
  async search(text: string) {
    clearTimeout(this.#timer);
    const id = ++this.#search;
    this.query = text;
    this.selected = -1;
    this.selection = new Map();
    if (!text.trim()) {
      this.#clearResults();
      this.#toTop();
      if (this.count > 0) void this.select(0);
      return;
    }
    if (!this.volume && this.index === "none") void this.startIndex();
    const request = this.#request(0);
    if (!request || !this.searchReady) {
      this.#clearResults();
      return;
    }
    try {
      const page = await api.search(request);
      if (id !== this.#search) return;
      this.#clearResults();
      this.#toTop();
      this.resultPages = { 0: page.items };
      this.#grow(0, page);
      if (!page.hasMore) this.resultCount = { count: page.items.length, more: false };
      if (page.items.length > 0) void this.select(0);
      if (page.hasMore) {
        void api.searchCount(request).then((count) => {
          if (id !== this.#search) return;
          this.resultCount = count;
          this.resultRows = count.more ? Math.max(this.resultRows, count.count) : count.count;
        }, () => {});
      }
    } catch (reason) {
      if (id !== this.#search) return;
      this.#clearResults();
      this.searchError = String(reason);
    }
  }

  async #loadResults(offset: number) {
    if (this.resultPages[offset] || this.#resultPending.has(offset)) return;
    const id = this.#search;
    const request = this.#request(offset);
    if (!request || !this.searchReady) return;
    this.#resultPending.add(offset);
    try {
      const page = await api.search(request);
      if (id !== this.#search) return;
      this.resultPages = prune({ ...this.resultPages, [offset]: page.items }, offset);
      this.#grow(offset, page);
    } catch (reason) {
      if (id === this.#search) this.searchError = String(reason);
    } finally {
      if (id === this.#search) this.#resultPending.delete(offset);
    }
  }

  close() {
    this.release();
    void api.closePane(this.id);
  }

  release() {
    clearTimeout(this.#timer);
    this.#navigation++;
    this.#search++;
    this.#revision++;
    this.pages = {};
    this.#clearResults();
    this.folder = undefined;
    this.#pending.clear();
    this.index = "none";
  }

  async suspend() {
    this.release();
    await api.suspendPane(this.id);
  }

  resume() { return this.path ? this.#go("reload", this.path) : Promise.resolve(); }
}
