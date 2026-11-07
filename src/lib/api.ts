import { invoke } from "@tauri-apps/api/core";
import type { FolderView, IndexState, Page, Startup } from "./contract";

export const api = {
  startup: () => invoke<Startup>("startup"),
  openFolder: (path: string) => invoke<FolderView>("open_folder", { path }),
  listPage: (id: number, offset: number) => invoke<Page>("list_page", { id, offset }),
  startIndex: (id: number) => invoke<IndexState>("start_index", { id }),
  search: (id: number, text: string, offset: number) =>
    invoke<Page>("search", { id, text, offset }),
  closeFolder: (id: number) => invoke<void>("close_folder", { id }),
  openWindow: (path: string, split: boolean) => invoke<void>("open_window", { path, split }),
  saveReport: (report: object) => invoke<void>("save_report", { report }),
};
