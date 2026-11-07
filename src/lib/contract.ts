// Mirrors src-tauri/src/contract.rs. Change both files together.

export const FOLDER_PROGRESS = "folder-progress";
export const INDEX_STATE = "index-state";

export interface Segment {
  name: string;
  path: string;
}

export interface Entry {
  name: string;
  path: string;
  directory: boolean;
  size: number | null;
  modified: number | null;
}

export interface Page {
  offset: number;
  items: Entry[];
  hasMore: boolean;
}

export interface FolderView {
  id: number;
  path: string;
  segments: Segment[];
  count: number;
  complete: boolean;
  page: Page;
}

export interface FolderProgress {
  id: number;
  count: number;
  complete: boolean;
  error: string | null;
}

export interface IndexState {
  id: number;
  ready: boolean;
  count: number;
  error: string | null;
}

export interface Startup {
  label: string;
  path: string;
  split: boolean;
  report: boolean;
  extraWindows: number;
  idleMs: number;
  pins: Segment[];
  locale: string;
  sinceStartMs: number;
}
