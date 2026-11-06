# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec. PLAN.md holds the execution plan and sources.
- M0 selected native Win32 after Tauri exceeded the 150,000,000-byte private working set cap.
- M1 provides paged directory browsing, folder search, opt-in read-only NTFS search, identity-checked operations and session undo.
- M2 added tabs, up to four windows, split panes, manual pins, seven themes, folder styles and an editable in-app menu.
- M3 batches up to 32 items, copies regular local files in up to two lanes, and shows speed, pause, resume and cancel in Transfers.
- M3 verifies SHA-256 for main and named streams before publishing. Cross-volume moves of regular local files remove the source after publication and use verified transfers for undo.
- Admin settings gate explicit administrator launches and elevated custom actions. Settings stay in local app data and Windows handles UAC consent.
- Eleven Windows release tests passed, including native CopyFile2 pause, resume and cancellation and cross-volume rollback and undo. Clippy and Svelte checks passed.
- A sampled run with one open window, two parallel 1 GiB copies and concurrent million-record indexing peaked at 16,977,920 bytes of combined private working set. The prior four-window M2 run peaked at 91,344,896 bytes.
- The M3 executable is `target/release/firewol-win32.exe`.

## Next Immediate Step
Complete interactive transfer and UAC cancellation checks, then repeat the four-window performance run with M3 transfers active before release.
