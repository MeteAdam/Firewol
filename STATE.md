# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec, PLAN.md the plan, docs/DESIGN.md the approved M4 interface spec, docs/HANDOFF.md how to run, verify and continue M4. Memory budget: about 150 MB idle in one window, below 250 MB worst case.
- M0–M3 shipped a native Win32 interface (`target/release/firewol-win32.exe`) with every M3 feature. It stays the reference until M4 reaches parity.
- M4 lives on branch `m4-interface` (local only; `main` ends at M3). The user accepted the Tauri trade-offs: about 0.5 s cold start and a short memory peak under heavy UI Automation clients.
- M4.2 is complete: metadata, core Session/History, generated TypeScript contracts, file operations and bounded asynchronous Windows Shell icons.
- The release Tauri build includes M4.2 and M4.3-a/b/c: shared versioned settings, theme migration, live appearance/sidebar, last folder, busy-close warning, folders-first natural sorting, identity-based selection, and tabs (per-tab history/search/scroll, inactive tabs free their listing, drag reorder, detach to a new window, four-window limit, split close merges tabs). Operations act on real files.
- Not ported yet: settings page (only a placeholder tab), folder styles, search filters, regex and NTFS search, drag and drop, grid view, transfer speed graph, breadcrumb collapsing.
- Latest gate (2026-11-07, tabs): idle 132.3 MB, peak 201.1 MB, navigation median 24.2 ms, full 100k sort 340 ms, scroll 165 FPS, search p95 20.1 ms. Cold start 458 ms (accepted). Self-test 40/40 incl. tab shortcuts and detach; Clippy, fmt, Svelte checks pass.

## Next Immediate Step
Follow `nce-kalan-ad-mlar-i-in-cuddly-harbor.md`: M4.3-d settings tab, then folder styles, search, grid/transfers and final parity; M4.4 acceptance precedes Win32 retirement.

## Completion Rule (AI reminder)
When M4.4 is accepted and the project is truly finished, do not close it silently: ask the user whether they want design corrections before the final wrap-up.
