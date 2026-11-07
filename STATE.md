# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec, PLAN.md the plan, docs/DESIGN.md the approved M4 interface spec, docs/HANDOFF.md how to run, verify and continue M4. Memory budget: about 150 MB idle in one window, below 250 MB worst case.
- M0–M3 shipped a native Win32 interface (`target/release/firewol-win32.exe`) with every M3 feature. It stays the reference until M4 reaches parity.
- M4 lives on branch `m4-interface` (local only; `main` ends at M3). The user accepted the Tauri trade-offs: about 0.5 s cold start and a short memory peak under heavy UI Automation clients.
- M4.2 is complete: metadata, core Session/History, generated TypeScript contracts, file operations and bounded asynchronous Windows Shell icons.
- The release Tauri build includes M4.2 and M4.3-a/b/c/d/e: shared versioned settings, theme migration, last folder, busy-close warning, folders-first natural sorting, identity-based selection, tabs (per-tab state, inactive tabs free their listing, drag reorder, detach, four-window limit, split close merges tabs) and the Settings tab (presets, HSV colour wheel, window controls, context-menu editor driving the real menu, custom actions and Run as administrator through core `explorer::actions`, elevation policy with inline confirmation) and folder styles (Customize folder… sheet with alias, icon colour, glyph and tags saved by identity through core `Session::set_folder_style`; listing rows carry their style from the core; alias in list, tabs, title and pins; Tags column only when a listed folder has tags). Operations act on real files.
- Not ported yet: search filters, regex and NTFS search, drag and drop, grid view, transfer speed graph, breadcrumb collapsing, opening files with their default app.
- Latest gate (2026-11-07, folder styles): idle 129.8 MB, peak 198.5 MB, navigation median 24.4 ms, full 100k sort 314 ms, scroll 164 FPS, search p95 16.2 ms. Cold start 485 ms (accepted). Self-test 71/71 (`scripts/Test-Interface.ps1`); core/Tauri tests, Clippy, fmt, Svelte checks pass.

## Next Immediate Step
Follow `nce-kalan-ad-mlar-i-in-cuddly-harbor.md`: M4.3-f search (filters, regex, NTFS scope chip, `--ntfs-helper` caught before Tauri starts), then grid/transfers and final parity; M4.4 acceptance precedes Win32 retirement.

## Completion Rule (AI reminder)
When M4.4 is accepted and the project is truly finished, do not close it silently: ask the user whether they want design corrections before the final wrap-up.
