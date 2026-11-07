# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec, PLAN.md the plan, docs/DESIGN.md the approved M4 interface spec, docs/HANDOFF.md how to run, verify and continue M4. Memory budget: about 150 MB idle in one window, below 250 MB worst case.
- M0–M3 shipped a native Win32 interface (`target/release/firewol-win32.exe`) with every M3 feature. It stays the reference until M4 reaches parity.
- M4 lives on branch `m4-interface` (local only; `main` ends at M3). The user accepted the Tauri trade-offs: about 0.5 s cold start and a short memory peak under heavy UI Automation clients.
- M4.2 is complete: metadata, core Session/History, generated TypeScript contracts, file operations and bounded asynchronous Windows Shell icons.
- The release Tauri build includes M4.2 and M4.3-a/b/c/d/e/f/g: shared versioned settings, theme migration, last folder, busy-close warning, folders-first natural sorting, identity-based selection, tabs (per-tab state, inactive tabs free their listing, drag reorder, detach, four-window limit, split close merges tabs) and the Settings tab (presets, HSV colour wheel, window controls, context-menu editor driving the real menu, custom actions and Run as administrator through core `explorer::actions`, elevation policy with inline confirmation) and folder styles (Customize folder… sheet with alias, icon colour, glyph and tags saved by identity through core `Session::set_folder_style`; listing rows carry their style from the core; alias in list, tabs, title and pins; Tags column only when a listed folder has tags) and search (150 ms debounce, virtual result pages with a capped count, Regex toggle, Filters popover for extension/size/local dates, scope chip for a whole NTFS drive with in-window confirmation then UAC; one shared drive index per drive in `src-tauri/src/volumes.rs`; `--ntfs-helper` handled before Tauri starts; stale `search-<pid>-*` index folders removed at start) and M4.3-g (Transfers popover fed by core `transfer-progress` events every 250 ms with a 60-sample speed graph and kept last result, `src-tauri/src/transfers.rs`; grid view with 96/128/160 px tiles, 2D keys, large Shell icons from the system image lists, view saved as core `grid`/`tile`; in-window pointer drag and drop onto folder rows/tiles, the other pane, tabs and pins, Shift moves, Esc cancels, edge auto-scroll). Operations act on real files.
- Not ported yet: breadcrumb collapsing, multi-item recycle, opening files with their default app.
- Latest gate (2026-11-07, M4.3-g): idle 131.8 MB, peak 204.9 MB, navigation median 24.4 ms, full 100k sort 328 ms, scroll 164 FPS, search p95 15.9 ms. Cold start 551 ms (accepted). UIA stress 60 s: list peak 268.5 MB (the accepted transient), grid peak 230.5 MB, no hangs. Self-test 112/112 (`scripts/Test-Interface.ps1`); core/Tauri tests, Clippy, fmt, Svelte checks pass.

## Next Immediate Step
Follow `nce-kalan-ad-mlar-i-in-cuddly-harbor.md`: M4.3-h parity (multi-item recycle with one undo, breadcrumb collapsing, context-menu icon row and arrow keys, sizes, package name); M4.4 acceptance precedes Win32 retirement. Still unverified by a person: accepting UAC for a full NTFS drive index from the Tauri build.

## Completion Rule (AI reminder)
When M4.4 is accepted and the project is truly finished, do not close it silently: ask the user whether they want design corrections before the final wrap-up.
