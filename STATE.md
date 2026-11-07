# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec, PLAN.md the plan and handoff, docs/DESIGN.md the approved M4 interface spec. Memory budget: about 150 MB idle in one window, below 250 MB worst case.
- M0–M3 shipped a native Win32 interface (`target/release/firewol-win32.exe`): browsing, search, opt-in NTFS search, verified operations and undo, tabs, split panes, themes, verified transfers and elevation policies. Admin-launch and NTFS-helper UAC denials are verified.
- M4.1 Tauri v2 + Svelte spike (`src-tauri`, `src`, `target/release/firewol-tauri.exe`) reuses firewol-core for paged listing and folder search. Settings, file operations, tabs and transfers are not ported yet.
- WebView2 runs with low-end device mode and one shared renderer (process-per-site); unfocused windows use the low memory target. Directory snapshots use a 1 MB SQLite cache.
- Gate on 2026-11-07 (`scripts/Measure-M4Gate.ps1`, 100k-file fixture): one window 148.5 MB idle; four windows with eight panes plus a 1M-record index build and a 256 MiB copy peak at 200.5 MB. Navigation 24 ms, scroll 165 FPS, search p95 19.6 ms.
- UIA stress (`scripts/Test-UiaStress.ps1`): the spike never hangs and exposes at most 235 elements (59 ms walks); Win32 took 43.8 s for one walk of 500,037 elements. A slow, event-heavy client briefly pushes the spike to 268 MB.
- Open trade-offs: cold start to first list takes 522 ms (374 ms WebView2 startup) against a 300 ms target; Win32 takes about 100 ms.

## Next Immediate Step
User decides whether to accept the cold-start and heavy-UIA trade-offs. If accepted, start M4.2: typed contracts, a UI-independent session layer in firewol-core, and size and date in `list_dir`. Otherwise, build the Direct2D fallback from docs/DESIGN.md.
