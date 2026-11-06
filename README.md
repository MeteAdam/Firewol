# Firewol

Firewol is a Windows 11 x64 file manager in development. The M0 prototype displays a virtual list from a synthetic NTFS folder and searches a disk-backed SQLite FTS5 index. The Win32 interface and the Tauri comparison build share the same Rust core. The code is available under the MIT license.

## Run the Win32 prototype

Build with Rust for `x86_64-pc-windows-msvc`, the MSVC x64 C++ tools and the Windows SDK. Run the executable from a standard-user session. Run the fixture command once to create the dataset at `%LOCALAPPDATA%\Firewol\M0`. Set `FIREWOL_M0_ROOT` to use another dataset.

```powershell
cargo build --release --locked
.\target\release\firewol-m0.exe prepare 1200 20000
.\target\release\firewol-win32.exe
```

One- and two-character searches use a prefix. Longer queries match a substring. The native M0 view displays the first 128 matches.

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
