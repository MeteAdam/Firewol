# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec, PLAN.md the plan, docs/DESIGN.md the approved M4 interface spec, docs/HANDOFF.md how to run, verify and continue M4. Memory budget: about 150 MB idle in one window, below 250 MB worst case.
- M0–M3 shipped a native Win32 interface (`target/release/firewol-win32.exe`) with every M3 feature. It stays the reference until M4 reaches parity.
- M4 lives on branch `m4-interface` (local only; `main` ends at M3). Last code commit `a9840cb`. The user accepted the Tauri trade-offs: about 0.5 s cold start and a short memory peak under heavy UI Automation clients.
- M4.2 is done except shell file icons: listings carry size, time and attributes; core `Session` and `History` hold clipboard, file operations, undo, transfer control, pins and pane history; `src-tauri/src/contract.rs` generates `src/lib/bindings/` through ts-rs.
- `target/release/firewol-tauri.exe` matches `a9840cb` and runs as is. It browses, searches folders, splits panes, opens windows, switches seven presets, and runs cut, copy, paste, pane transfers, inline rename, Recycle Bin, undo and pins. Operations act on real files.
- Not ported yet: shell file icons, tabs, settings page, folder styles, search filters, regex and NTFS search, drag and drop, grid view, transfer speed graph, breadcrumb collapsing, folders-first sorting.
- Latest gate (2026-11-07): one idle window 134.9 MB, worst case 200.9 MB, navigation 15 ms, scroll 164 FPS, search p95 18 ms. Core tests, Clippy, fmt, svelte-check and the 18-step self-test pass.

## Next Immediate Step
Finish M4.2 with shell file icons: a core icon cache keyed by extension (per path for executables and shortcuts), served to WebView2 through a custom protocol, then re-run the memory gate. After that, start M4.3 with tabs and the in-window settings page (docs/HANDOFF.md › Remaining roadmap).
