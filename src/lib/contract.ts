// Types come from src-tauri/src/contract.rs; `cargo test -p firewol-tauri` regenerates ./bindings.
export type { ClipboardState } from "./bindings/ClipboardState";
export type { Entry } from "./bindings/Entry";
export type { FolderProgress } from "./bindings/FolderProgress";
export type { FolderView } from "./bindings/FolderView";
export type { IndexState } from "./bindings/IndexState";
export type { ItemRef } from "./bindings/ItemRef";
export type { Navigation } from "./bindings/Navigation";
export type { OperationDone } from "./bindings/OperationDone";
export type { Page } from "./bindings/Page";
export type { Segment } from "./bindings/Segment";
export type { SessionState } from "./bindings/SessionState";
export type { Startup } from "./bindings/Startup";
export type { Step } from "./bindings/Step";
export type { TransferStatus } from "./bindings/TransferStatus";

// Event names mirror the constants in contract.rs.
export const FOLDER_PROGRESS = "folder-progress";
export const INDEX_STATE = "index-state";
export const OPERATION_DONE = "operation-done";
export const SESSION_CHANGED = "session-changed";
