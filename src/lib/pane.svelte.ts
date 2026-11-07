import { api } from "./api";
import type {
  Entry,
  FolderProgress,
  FolderView,
  IndexState,
  ItemRef,
  Navigation,
  Page,
  Step,
  SortView,
  SortColumn,
} from "./contract";

export const PAGE = 128;
export const itemKey = (item: ItemRef) => `${item.volume}:${item.index}:${item.path}`;
// Pages farther than this from the newest page are dropped to bound memory.
const KEEP = PAGE * 6;

/** One folder view. Navigation history lives in the core; this keeps what the list renders. */
export class Pane {
  readonly id: string;
  path = $state("");
  label = $state("Folder");
  scroll = $state(0);
  // Listing data is replaced, never mutated, so it skips Svelte's per-field deep proxies.
  folder = $state.raw<FolderView>();
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
  results = $state.raw<Page>();
  index = $state<"none" | "building" | "ready">("none");
  indexCount = $state(0);
  error = $state("");
  #anchor = -1;
  #pending = new Map<number, number>();
  #revision = 0;
  #selecting = 0;
  #navigation = 0;
  #search = 0;

  constructor(id: string) {
    this.id = id;
  }

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
        if (!this.pages[offset]) await this.loadPage(offset);
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
    this.#revision++;
    this.pageVersion++;
    this.#pending.clear();
    this.folder = view;
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
      this.query = "";
      this.results = undefined;
    }
    this.index = "none";
    this.indexCount = 0;
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
      const next = { ...this.pages, [offset]: page.items };
      for (const key of Object.keys(next).map(Number)) {
        if (Math.abs(key - offset) > KEEP) delete next[key];
      }
      this.pages = next;
    } catch (reason) {
      this.error = String(reason);
    } finally {
      if (this.#pending.get(offset) === revision) this.#pending.delete(offset);
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
    this.selection = new Map();
    if (!text.trim()) {
      this.results = undefined;
      if (this.count > 0) this.select(0);
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
        if (page.items.length > 0) this.select(0);
      }
    } catch (reason) {
      if (id === this.#search) this.error = String(reason);
    }
  }

  close() {
    this.release();
    void api.closePane(this.id);
  }

  release() {
    this.#navigation++;
    this.#search++;
    this.#revision++;
    this.pages = {};
    this.results = undefined;
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
