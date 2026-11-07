# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec, PLAN.md the plan and handoff, docs/DESIGN.md the approved M4 interface spec. Memory budget: about 150 MB idle in one window, below 250 MB worst case.
- M0–M3 shipped a native Win32 interface (`target/release/firewol-win32.exe`) with every M3 feature. It stays as the reference until M4 reaches parity.
- M4 work lives on branch `m4-interface`. The user accepted the Tauri trade-offs: cold start of about 0.5 s and a short memory peak under heavy UI Automation clients.
- M4.2 is done: listings carry size, time and attributes; `firewol_core::explorer::Session` and `History` hold clipboard, file operations, undo, transfer control, pins and pane history; `src-tauri/src/contract.rs` generates `src/lib/bindings/` through ts-rs.
- The Tauri interface browses, searches folders, splits panes, opens windows, switches seven presets, and runs cut, copy, paste, pane transfers, inline rename, Recycle Bin, undo and pins. Not yet ported: shell file icons, tabs, settings page (appearance editor, menu editor, admin policy), folder styles, search filters, regex and NTFS search, drag and drop, grid view, transfer speed graph, breadcrumb collapsing.
- Latest gate (2026-11-07, `scripts/Measure-M4Gate.ps1`): one idle window 134.9 MB, worst case 200.9 MB, navigation 15 ms, scroll 164 FPS, search p95 18 ms. The 18-step interface self-test (`FIREWOL_SELF_TEST`) passes.

## Next Immediate Step
Finish M4.2 with shell file icons: a core icon cache keyed by extension or path, served to WebView2 through a custom protocol, measured against the memory gate. Then start M4.3 parity with tabs and the in-window settings page.
