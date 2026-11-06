# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec. PLAN.md holds the execution plan and sources.
- M0 selected Win32 after the Tauri prototype exceeded the 150,000,000-byte private working set cap.
- M1 runs from `target/release/firewol-win32.exe` on Windows 11 x64. It restores the last folder, persists manual pins, and provides virtual directory paging and cancellable folder and NTFS search.
- The NTFS helper is read-only and opt-in. A disposable volume verified UAC approval, ACL filtering, live USN updates and journal-loss recovery.
- File operations use verified identities. Windows tests covered Recycle Bin restoration, undo conflicts, interrupted copies, case-only rename and long Unicode paths. Undo is scoped to the current session and verified items.
- Three release runs with 100,000 rows and a million-file index showed first list in 94–232 ms, search p95 in 85–97 ms, scroll at 79–95 FPS and a folder-workload private peak of 9.93 MB.
- Copies of nonempty folders form an undo boundary. Extended paths use native copy and move. Recycling requires an exact Windows Shell item.
- Keep settings under `%LOCALAPPDATA%\Firewol` and the total private working set below 150,000,000 bytes.

## Next Immediate Step
Exercise the UAC-denial path on a disposable NTFS volume, then start M2 with tabs and split-pane navigation.
