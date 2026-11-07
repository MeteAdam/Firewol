# Firewol M4 handoff notes

Details that do not fit in the 25-line `STATE.md`. Read `STATE.md` first; this file explains how to run, verify and continue the M4 interface work. Last updated 2026-11-07.

## Where the work stopped

- Branch `m4-interface` (local repository, no remote). `main` still ends at the M3 commit `f8a5659`; the M4 branch fast-forwards onto it.
- Commits: `4c2cdb0` (M4 plan, NTFS-helper UAC verification, design, M4.1 spike and gate), `a9840cb` (M4.2 session layer, contract, file operations).
- M4.2 is done except shell file icons. M4.3 (feature parity) and M4.4 (acceptance and Win32 retirement) have not started.
- The Win32 build (`target/release/firewol-win32.exe`, built 2026-11-06 from M3) still has every M3 feature and stays the reference until M4.4. It does not include the M4 core changes until it is rebuilt.

## Try the new interface

The release build at `target/release/firewol-tauri.exe` is current with `a9840cb`. It needs the WebView2 Runtime, which Windows 11 includes. It opens your user folder unless `FIREWOL_OPEN` names another one.

```powershell
.\target\release\firewol-tauri.exe
$env:FIREWOL_OPEN = 'D:\Some\Folder'; .\target\release\firewol-tauri.exe
```

File operations are real: cut, paste, rename and Recycle Bin change files on disk. Undo (Ctrl+Z or the toast button) reverses them within the running session. Settings and pins live in `%LOCALAPPDATA%\Firewol\explorer.json`, the same file the Win32 build uses. WebView2 keeps its profile in `%LOCALAPPDATA%\dev.firewol.app`. The theme choice is stored per WebView2 profile in `localStorage` until the settings page moves it into the core settings.

Shortcuts: Ctrl+L path, Ctrl+F search, Alt+Left/Right/Up, F5, Ctrl+N new window, Ctrl+\ split view, Ctrl+B sidebar, Ctrl+X/C/V, F2 rename, Del recycle, Ctrl+Z undo, Ctrl or Shift click for multi-selection, Shift+F10 or the Menu key for the context menu. Themes are under `⋯` in the toolbar.

## Build from source

```powershell
npm ci
npm run check
npm run build
cargo build --release --locked -p firewol-tauri --features custom-protocol
```

`npm run build` must run before the Rust build, because the release binary embeds `dist/`. A dependency change needs `cargo build` without `--locked` once (or `--offline` when every crate is already in the local registry).

## Verify

| Check | Command |
| --- | --- |
| Core tests (explorer, session, transfer) | `cargo test --release -p firewol-core` |
| Regenerate the TypeScript contract | `cargo test -p firewol-tauri` (writes `src/lib/bindings/`) |
| Lint | `cargo clippy --release -p firewol-core -p firewol-tauri -p firewol-win32 --all-targets -- -D warnings` |
| Format | `cargo fmt --all -- --check` |
| Interface types | `npm run check` |
| Memory and performance gate | `.\scripts\Measure-M4Gate.ps1 -App .\target\release\firewol-tauri.exe -Fixture <M0 fixture>` |
| UI Automation stress | `.\scripts\Test-UiaStress.ps1 -App <exe> -Folder <100k folder>` |
| Interface self-test | `.\scripts\Test-Interface.ps1` (disposable fixture, isolated `LOCALAPPDATA`); `main.json` lists 40 steps and the detached tab reports from `window-1.json` |
| Window screenshot | `powershell.exe -File .\scripts\Capture-Window.ps1 -ProcessId <pid> -OutputPath <png> -TitleLike Firewol` |

The 100,000-file and 1,000,000-record fixture used by every gate run is `target/verify-m3-be5d47eaa6cc4d62b64e597855550377/fixture` (ignored by Git). Recreate one with `firewol-m0.exe prepare 100000 1000000` and `FIREWOL_M0_ROOT` (see README › Reproduce the M0 measurement). The self-test fixture is a folder with `source\note.txt` and an empty `dest`.

The gate and UIA scripts isolate `LOCALAPPDATA` and `WEBVIEW2_USER_DATA_FOLDER` under `target/`, so they never touch your real settings. Their reports land in `target/measure-m4-*` and `target/uia-*`.

## Environment variables

| Variable | Effect |
| --- | --- |
| `FIREWOL_OPEN` | Folder for the first window (default: user profile). |
| `FIREWOL_SPLIT` | Any value: the first window opens with split panes. |
| `FIREWOL_SPIKE_REPORT` | Report directory; enables the benchmark (or the self-test) and `save_report`. |
| `FIREWOL_SPIKE_WINDOWS` | With a report directory: total windows the benchmark opens (gate uses 4). |
| `FIREWOL_SPIKE_IDLE_MS` | Idle time before the benchmark opens more windows (default 10,000). |
| `FIREWOL_SELF_TEST` | With a report directory: run the file-operation self-test instead of the benchmark. |
| `FIREWOL_WEBVIEW_ARGS` | Extra Chromium arguments for experiments, appended to the built-in ones. |
| `FIREWOL_KEEP_MEMORY` | Any value: unfocused windows keep the normal WebView2 memory target. |

## Code map

| Area | Files |
| --- | --- |
| Core listing, search, operations | `crates/firewol-core/src/explorer/{directory,search,operations,transfer}.rs` |
| Core session layer (M4.2) | `crates/firewol-core/src/explorer/session.rs`, tests in `crates/firewol-core/tests/windows_session.rs` |
| Tauri contract | `src-tauri/src/contract.rs` → generated `src/lib/bindings/*.ts`, re-exported by `src/lib/contract.ts` |
| Tauri commands, windows, WebView2 settings | `src-tauri/src/main.rs` |
| Panes, history, folder snapshots, item identities | `src-tauri/src/folders.rs` |
| Interface | `src/App.svelte` (shell, menus, toast, benchmark, self-test), `src/lib/FileList.svelte` (virtual grid), `src/lib/pane.svelte.ts` (pane model), `src/lib/theme.css` (tokens and shared controls), `src/lib/Icon.svelte` |
| Design | `docs/DESIGN.md`; mockup canvas linked there (private Artifact) |

## Decisions and why

- **Tauri v2 + Svelte over native Win32 or Direct2D.** Highest visual ceiling with the most reliable polish; the user relaxed the memory budget to about 150 MB idle and 250 MB worst case and accepted the cold-start and UIA trade-offs.
- **`--enable-low-end-device-mode`.** Without it, fast scrolling grew a Chromium raster and cache pool by about 70 MB per renderer. With it the renderer stays near 40–60 MB, with no visible quality loss in the Light, Glass and Metal presets.
- **`--process-per-site`.** All windows share one renderer instead of one per window (the untuned four-window run peaked at 521 MB). Side effect: windows share one renderer main thread, so a long JavaScript task in one window would stall the others. Keep interface work small and asynchronous.
- **Low memory target when unfocused.** `ICoreWebView2_19::SetMemoryUsageTargetLevel` on focus changes.
- **Keyed virtual rows, overscan 6.** Unkeyed rows used less memory but rewrote every visible row on each scroll step, flooding UI Automation with events (391 MB peak under the stress client); keyed rows cut that to 268 MB.
- **`$state.raw` for listing data.** Deep Svelte proxies are unnecessary for replaced, never-mutated pages.
- **The grid is its own scroll container** with a sticky header, so UI Automation exposes a scrollable table.
- **Item identity as path, volume and decimal file index.** NTFS file indexes exceed 2^53; every operation re-verifies the identity.
- **Win32 is frozen, not refactored.** Its session logic is duplicated in the core until M4.4 retires it.

## Known issues and loose ends

- Cold start to the first list takes about 0.52 s, of which WebView2 startup is about 0.37 s (target 0.3 s; Win32 about 0.1 s). Accepted.
- A slow, event-heavy UI Automation client can push the WebView2 browser process to a 268 MB transient peak. Accepted; Narrator-style use is far lighter.
- Long paths overflow the breadcrumb; DESIGN asks for leading segments to collapse into `…`.
- Listing order is the file system's enumeration order (NTFS: by name, folders mixed with files). Folders-first and column sorting are not implemented.
- Search covers folder search without filters or regex; NTFS volume search, filters and regex exist only in the Win32 build.
- Recycle and rename act on one item; multi-item recycle needs sequential jobs in the session.
- The theme lives in WebView2 `localStorage`, not in `Settings.appearance`.
- `package.json` is still named `firewol-m0`.
- Three mockup PNGs exported by the user sit untracked in the project root; leave them out of commits unless the user asks.

## Remaining roadmap

1. **M4.2 finish:** shell file icons. Extract icons in the core (Shell image factory or `SHGetFileInfo`, cached by extension and per path for executables and shortcuts), serve them through a Tauri custom protocol, and re-run the memory gate.
2. **M4.3 parity:** tabs (drag, detach), settings as a tab (appearance editor with color wheel and the seven presets moved into core settings, context-menu editor, admin policy), folder styles (alias, color, icon, tags), search filters, regex and the NTFS search scope chip, drag and drop between panes, grid view, the Transfers popover with speed graph, breadcrumb collapsing, folders-first sorting, multi-item recycle, Nilesoft-style icon row in the context menu.
3. **M4.4 acceptance:** gate and UIA stress on the final interface, interactive transfer, recycle and undo scenarios, long and Unicode paths, then remove the Win32 interface (keep the M0 measurement binaries) and update README.
4. **Design review (required before closing):** once M4.4 passes and nothing else remains, ask the user whether they want design corrections. Apply any requested changes and re-run the affected checks before calling the project finished.
