# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec. PLAN.md holds the execution plan, sources and handoff; this record supersedes its original M0 bootstrap handoff.
- M0 selected native Win32 after Tauri exceeded the 150,000,000-byte private working set cap.
- M1 provides paged directory browsing, folder search, opt-in read-only NTFS search, identity-checked operations and session undo.
- M2 added tabs, up to four windows, split panes, manual pins, seven themes, folder styles and an editable in-app menu.
- M3 batches up to 32 items, copies regular local files in up to two lanes, and shows speed, pause, resume and cancel in Transfers.
- M3 verifies SHA-256 for main and named streams before publishing. Cross-volume moves of regular local files remove the source after publication and use verified transfers for undo.
- Admin settings gate explicit administrator launches and elevated custom actions. Settings stay in local app data and Windows handles UAC consent.
- On 2026-11-07, nine standard Windows release tests and the extended native pause/resume/cancel test passed. Rust formatting, Clippy and Svelte checks were clean; cross-volume tests were not rerun.
- Interactive two-lane 4 GiB copies, pause, resume, cancellation, staging cleanup and recycle/restore were verified on synthetic NTFS files with isolated settings.
- A 100,000-row list initially presented in 103 ms at 88 FPS, but later stalled with UI Automation active and reached 555,610,112 bytes of private working set in one window. Stacks point into Windows accessibility code; the root cause is not yet isolated.
- The four-window M3 run is not accepted. Prior M2 and M3 memory measurements do not cover this failing scenario. Session sampling is reproducible with scripts/Measure-Session.ps1.
- The M3 executable is `target/release/firewol-win32.exe`.

## Next Immediate Step
Isolate and fix the large-directory stall with UI Automation active, then repeat four-window/eight-pane M3 transfer and million-record indexing measurements. Complete explicit user-driven UAC denial before release; do not restart M0.
