# Firewol

Firewol is a Windows 11 x64 file manager with a native Win32 virtual list and disk-backed SQLite FTS5 search. The code is available under the MIT license.

## Run Firewol

Build with Rust for `x86_64-pc-windows-msvc`, the MSVC x64 C++ tools and the Windows SDK. Run the executable from a standard-user session.

```powershell
cargo build --release --locked
.\target\release\firewol-win32.exe
```

Firewol opens your user folder on the first launch and restores the last accessible folder on later launches. Use `--open PATH` to choose a starting folder. The sidebar shows folders you pin manually. Select a pin and press Delete to remove it. Settings are stored under `%LOCALAPPDATA%\Firewol`.

- **Navigation.** Use the address bar, Back, Forward, Up or the folder rows. Ctrl+L focuses the address bar. F5 refreshes the listing.
- **Search.** Ctrl+F searches the current folder and its subfolders. One- and two-character terms match prefixes. Longer terms match substrings. Use Regex and Filters for extension, size and UTC modification dates. Previous and Next navigate result pages. Folder search refreshes its index after Windows change notifications.
- **NTFS search.** Choose **NTFS search…** to index the current drive for the current search session. The read-only helper requests UAC and requires an active USN journal. It sends opaque file references through a private local pipe. The regular Firewol process verifies paths and access before indexing and showing results. The index tracks one accessible path per reference and rebuilds after lost journal history.
- **File operations.** Use the context menu or Ctrl+C, Ctrl+X and Ctrl+V to copy or move selected items through the Firewol clipboard. F2 renames. Delete sends the selected item to the Recycle Bin. Ctrl+Z reverses one verified item at a time in the current session when the original path is available. Copies of nonempty folders form an undo boundary.

Firewol uses Windows Shell operations for same-volume moves, folder copies and other transfers outside the regular fixed-drive file path. Recycle requires the Shell to resolve the exact item. [Windows Shell file operations](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation) · [Extended Windows paths](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)

## M3 transfers and administrator actions

- Select up to 32 items with Ctrl or Shift. Paste into a folder or transfer directly to the other pane. Regular files on fixed local drives copy in up to two lanes. Open **Transfers…** to see copy speed, the recent speed graph and the last result. Pause, resume or cancel an in-flight copy in the current session. Shell operations on folders and other fallback items finish the current item before taking a pause or cancel request.
- Firewol verifies SHA-256 for the main data and every named data stream before publishing a staged copy. A verified cross-volume move of a regular local file publishes without overwriting, then removes the verified original. If source removal fails, Firewol rolls back the new copy when possible and reports its location if rollback fails. Windows applies destination permissions. Undo uses the same verified transfer in reverse. [CopyFile2](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-copyfile2) · [Windows pause and resume](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ne-winbase-copyfile2_message_action)
- **Admin…** defaults to standard permissions. **Allowlisted executables** authorizes administrator requests for the listed absolute `.exe` paths and verified file identities. Separate paths with `|`. **Any executable (higher risk)** prompts for confirmation when enabled. Choose **Run as administrator…** on an eligible executable or set **Run with** to **Administrator consent** on a custom menu action. Windows handles UAC consent. The read-only NTFS search helper retains its separate opt-in. [ShellExecuteExW](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecuteexw)

The Windows release tests cover native pause, resume and cancellation, batches with a conflict, long Unicode paths, settings migration, executable replacement, cross-volume moves with named streams, and undo conflicts. To run both extended M3 tests, set `FIREWOL_M3_OTHER_VOLUME` to a writable folder on a second fixed NTFS volume and run `cargo test --release --locked -p firewol-core --test windows_transfer -- --include-ignored`.

## Current verification and release blocker

On 2026-11-07, the existing MSVC x64 toolchain and Windows SDK were verified. Rust formatting, release Clippy, nine standard Windows release tests, the extended native pause/resume/cancel test and Svelte checks passed. The cross-volume test was not rerun in this session.

The release UI copied two 4 GiB files in parallel, completed verification, and reported success. A second transfer was paused at 5,385 MiB with no published files, resumed beyond 7,232 MiB, and cancelled before publication. Cancellation removed both staging directories and preserved both sources. The exact-identity Recycle Bin restore and destination-conflict probe also passed. All fixtures and settings were isolated under `target/verify-m3-be5d47eaa6cc4d62b64e597855550377` on the local NTFS drive.

**Release remains blocked.** A 100,000-row list initially presented in 103 ms, scrolled at 88 FPS and verified its last row, but later became unresponsive with desktop UI Automation active. A fresh single-window process reached **555,610,112 bytes of private working set**. Samples of its busy thread ran through Windows accessibility routines. This suggests an accessibility-related enumeration problem; its root cause has not been isolated. The attempted four-window M3 run was stopped, and these results supersede any inference that the earlier memory measurements cover this scenario. Explicit user-driven UAC denial is also pending.

The fixture's `list.json`, `memory.json`, `large-list-blocker.json` and `large-list-stack.txt` retain the observations locally. Large transfer payloads were removed after verification; the directory fixture and reports remain. `STATE.md` identifies the next investigation.

For subsequent interactive measurements, `scripts/Measure-Session.ps1` reuses the M0 private-working-set sampler. Supply the app PID, a control JSON path and an output JSON path. Control JSON has `phase` (a nonempty label), `process_ids` (additional helper roots) and `stop` (boolean). Start with `stop: false`, change phase or helper roots as workloads change, and set `stop: true` to finish. Replace the control file atomically from a temporary file in the same directory. Detached Firewol windows share one process; process trees and repeated roots are deduplicated. The report includes phase peaks, sample counts, the largest sampling gap, timeout status and the 150,000,000-byte check. A run with no samples fails.

```powershell
.\scripts\Measure-Session.ps1 -AppProcessId 12345 -ControlPath .\target\session-control.json -OutputPath .\target\session-memory.json
```

## M2 navigation and appearance

- **Tabs.** Press Ctrl+T to open a tab in the current folder, Ctrl+Tab to switch, and Ctrl+W to close. Drag a tab along the strip to reorder it or outside the window to detach it. Ctrl+Shift+N also detaches the active tab. A session supports four windows with shared clipboard and undo history. A tab restores its folder history and rebuilds any resumed folder search.
- **Split view.** Open a second pane with its own folder, tabs and search. Drop selected items on the other pane to copy them into the displayed folder. Hold Shift to move them. The context menu also offers both actions. Returning to a single pane keeps the open tabs.
- **Sidebar.** Toggle the sidebar from the toolbar. Pin and unpin folders from either pane. The sidebar opens a selected pin in the active pane.
- **Appearance.** Select a preset, pick colors on the wheel or enter hex values, and set the caption buttons' side, shape, fill and hover color. Glassmorphism uses the Windows acrylic backdrop in the title area where supported. [Windows system backdrop types](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type)
- **Folder appearance.** Right-click a folder, or the blank area inside it, and choose **Customize folder…** to set a display name, icon glyph, color and tags. Firewol stores these by folder identity in `explorer.json` and reads them independently of the search index.
- **In-app menu.** Use **Edit menu…** to reorder or hide actions and change labels, glyphs and icon colors. Add a custom action with an absolute executable path. Firewol passes the verified selected item or current folder as one argument. [Nilesoft menu reference](https://nilesoft.org/images/screenshots/light/goto.png)

### M2 verification

The Windows 11 release run opened four windows and eight panes with 100,000 rows each. It measured 84 ms to the first list, 90 FPS during single-pane scrolling and a 4.0 ms p95 across 30 ready-index queries against the 100,000-item folder. Peak sampled private working set was 91,344,896 bytes across Firewol and a concurrent million-record index build. A 256 MiB copy completed through the split-pane UI.

The release tests cover legacy settings and concurrent writes. Windows UI exercises covered tab reordering and detachment, split-pane transfers and undo, cross-window clipboard, all seven presets, the color wheel, caption controls, folder styles, and custom menu actions.

## M1 verification

`cargo test --release --locked` exercises Windows item identity, copy and undo conflicts, long Unicode paths, paged search and external folder changes. The release executable was also tested with a disposable NTFS volume for UAC-approved volume search, ACL visibility, live journal updates and journal loss recovery.

Three release runs on Windows 11 used a 100,000-row NTFS directory and an index of one million synthetic files. First list presentation took 94–232 ms, ready-index search p95 was 85–97 ms, and native scrolling reached 79–95 FPS. The last virtual row was verified in all three runs. Initial folder indexing took 7.8–10.6 seconds. The folder workload peaked at 9,932,800 bytes of sampled private working set. The explicit UAC-denial exercise remains open.

A 256 MiB transfer through the release Rust core also completed while the native window was open. The window and the separate transfer probe peaked at a combined 6,180,864 bytes of sampled private working set.

To record native startup and scrolling, set `FIREWOL_M1_LIST_REPORT` to a new JSON path and start `firewol-win32.exe --open PATH` with a large synthetic folder. Firewol records first presentation, native scroll redraws and the last virtual row.

## Reproduce the M0 measurement

1. In an elevated PowerShell with the Hyper-V PowerShell module available, run `.\scripts\New-M0Volume.ps1 -ImagePath "$env:TEMP\firewol-m0.vhdx"` to create and mount a fresh NTFS VHDX.
2. In a standard-user PowerShell, set `FIREWOL_M0_ROOT` to a new folder on the drive printed by the script. Run the following commands from the project root.

```powershell
.\target\release\firewol-m0.exe prepare 100000 1000000
.\scripts\Measure-M0.ps1 -App .\target\release\firewol-win32.exe -Fixture $env:FIREWOL_M0_ROOT
.\target\release\firewol-m0.exe recycle-probe
.\target\release\firewol-m0.exe ntfs-probe
```

The NTFS probe opens a UAC prompt for a one-shot read-only helper. It checks the MFT for the reference of a disposable file opened under the caller's regular permissions. The Recycle Bin probe checks the original file identity and bytes after restore, and verifies that a destination conflict preserves the replacement file. [MFT enumeration](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data) · [RecycleItem](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-itransfersource-recycleitem)

Build and measure the Tauri comparison with Node, npm and the WebView2 Runtime.

```powershell
npm ci
npm run build
cargo build --release --locked -p firewol-tauri --features custom-protocol
.\scripts\Measure-M0.ps1 -App .\target\release\firewol-tauri.exe -Fixture $env:FIREWOL_M0_ROOT
```

The sampler sums `PROCESS_MEMORY_COUNTERS_EX2.PrivateWorkingSetSize` across the app process tree and the active indexing or copy workload. It records the sampled peak during startup, search and scroll, idle, rebuilding a second index, and copying a 256 MiB fixture file. The elevated MFT probe runs separately. Reports are written under `target/measure-*`. [Windows memory counters](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex2) · [WebView2 process model](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model)

Dismount the test image from elevated PowerShell when finished with `Dismount-VHD -Path "$env:TEMP\firewol-m0.vhdx"`.

## M0 results on Windows 11

Release measurements on 2026-11-06 used 100,000 NTFS files and a disk index of 1,000,000 synthetic names.

| Interface | Highest sampled private working set | Search p95 | Scroll rate |
| --- | ---: | ---: | ---: |
| Tauri v2 and one WebView2 | 163,418,112 bytes | 15.4 ms | 165 FPS |
| Win32 owner-data ListView and Direct2D | 30,793,728 bytes | 12.4–13.1 ms | 137–162 FPS |

Win32 presented the first page in 224–268 ms across three release runs and displayed the last virtual row. Rebuilding the million-name index under the Win32 workload took 2.5–2.9 seconds. The 150,000,000-byte gate selects Win32 for M1.
