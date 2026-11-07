import type { TabSnapshot } from "./contract";
import { Pane } from "./pane.svelte";

export type Tab = { kind: "folder"; id: string; pane: Pane } | { kind: "settings"; id: string };
export const folderTab = (pane: Pane): Tab => ({ kind: "folder", id: pane.id, pane });

export class PaneSlot {
  tabs = $state<Tab[]>([]);
  activeTab = $state("");
  activation = 0;
  constructor(tab: Tab) { this.tabs = [tab]; this.activeTab = tab.id; }
  get active() { return this.tabs.find(tab => tab.id === this.activeTab) ?? this.tabs[0]; }
  reorder(id: string, target: string) {
    const tabs = [...this.tabs];
    const from = tabs.findIndex(tab => tab.id === id);
    const to = tabs.findIndex(tab => tab.id === target);
    if (from < 0 || to < 0 || from === to) return;
    tabs.splice(to, 0, ...tabs.splice(from, 1));
    this.tabs = tabs;
  }
}

export function snapshot(tab: Tab): TabSnapshot {
  return tab.kind === "settings" ? { kind: "settings", pane: tab.id } : {
    kind: "folder", pane: tab.id, path: tab.pane.path, query: tab.pane.query,
    scroll: tab.pane.scroll, sort: tab.pane.sort,
  };
}

export function restore(tab: TabSnapshot): Tab {
  if (tab.kind === "settings") return { kind: "settings", id: tab.pane };
  const pane = new Pane(tab.pane);
  pane.path = tab.path;
  pane.label = tab.path.split(/[\\/]/).filter(Boolean).at(-1) ?? tab.path;
  pane.query = tab.query;
  pane.scroll = tab.scroll;
  pane.sort = tab.sort;
  return folderTab(pane);
}
