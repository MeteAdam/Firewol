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
- **File operations.** Use the context menu or Ctrl+C, Ctrl+X and Ctrl+V to copy or move an item through the Firewol clipboard. F2 renames. Delete sends the selected item to the Recycle Bin. Ctrl+Z reverses verified operations in the current session when the original path is available. Copies of nonempty folders form an undo boundary. Same-volume moves are supported in M1.

Firewol uses Windows Shell operations for ordinary transfers and extended Win32 paths for long copies and moves. Recycle requires the Shell to resolve the exact item. The M1 toolbar and window background use native system drawing to keep the first paint fast. [Windows Shell file operations](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation) · [Extended Windows paths](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation)

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
