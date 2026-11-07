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
} from "./contract";

export const PAGE = 128;
// Pages farther than this from the newest page are dropped to bound memory.
const KEEP = PAGE * 6;

/** One folder view. Navigation history lives in the core; this keeps what the list renders. */
export class Pane {
  readonly id: string;
  // Listing data is replaced, never mutated, so it skips Svelte's per-field deep proxies.
  folder = $state.raw<FolderView>();
  count = $state(0);
  complete = $state(false);
  pages = $state.raw<Record<number, Entry[]>>({});
  canBack = $state(false);
  canForward = $state(false);
  /** The focused row; selection holds every selected row. */
  selected = $state(-1);
  selection = $state.raw<Set<number>>(new Set());
  renaming = $state<number | null>(null);
  query = $state("");
  results = $state.raw<Page>();
  index = $state<"none" | "building" | "ready">("none");
  indexCount = $state(0);
  error = $state("");
  #anchor = -1;
  #pending = new Set<number>();
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
    return [...this.selection]
      .sort((a, b) => a - b)
      .map((row) => this.entry(row))
      .filter((entry): entry is Entry => entry !== undefined);
  }

  selectedRefs(): ItemRef[] {
    return this.selectedEntries().flatMap((entry) => (entry.id ? [entry.id] : []));
  }

  select(row: number, mode: "single" | "toggle" | "range" = "single") {
    if (this.rows === 0) return;
    row = Math.max(0, Math.min(this.rows - 1, row));
    if (mode === "toggle") {
      const next = new Set(this.selection);
      if (next.has(row)) next.delete(row);
      else next.add(row);
      this.selection = next;
      this.#anchor = row;
    } else if (mode === "range" && this.#anchor >= 0) {
      const [from, to] = this.#anchor < row ? [this.#anchor, row] : [row, this.#anchor];
      this.selection = new Set(Array.from({ length: to - from + 1 }, (_, index) => from + index));
    } else {
      this.selection = new Set([row]);
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
      result = await api.navigate(this.id, path, step);
    } catch (reason) {
      if (navigation === this.#navigation && !String(reason).includes("newer navigation")) {
        this.error = String(reason);
      }
      return;
    }
    if (navigation !== this.#navigation) return;
    const view = result.folder;
    const reload = step === "reload" && view.path === this.folder?.path;
    const keep = reload ? this.selected : 0;
    this.#pending.clear();
    this.folder = view;
    this.count = view.count;
    this.complete = view.complete;
    this.pages = { 0: view.page.items };
    this.canBack = result.canBack;
    this.canForward = result.canForward;
    this.renaming = null;
    this.error = "";
    if (!reload) {
      this.query = "";
      this.results = undefined;
    }
    this.index = "none";
    this.indexCount = 0;
    if (view.count > 0) this.select(Math.min(keep, view.count - 1));
    else {
      this.selected = -1;
      this.selection = new Set();
    }
    if (reload && this.searching) void this.search(this.query);
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
    this.selection = new Set();
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
    void api.closePane(this.id);
  }
}
