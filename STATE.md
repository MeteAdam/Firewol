# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec. PLAN.md holds the execution plan and sources.
- M0 selected native Win32 after the Tauri prototype exceeded the 150,000,000-byte private working set cap.
- M1 provides virtual directory paging, manual pins, cancellable folder search, opt-in NTFS volume search, identity-checked file operations and session undo. The live UAC denial exercise remains open.
- M2 runs from `target/release/firewol-win32.exe`. Tabs retain history when reordered or detached. Split panes navigate independently and accept drag-to-copy or Shift-drag-to-move. Up to four windows share a process, clipboard and undo history.
- M2 provides seven presets, a color wheel, configurable caption buttons and an editable in-app menu. Sidebar preferences and folder aliases, icon styles and tags persist in `%LOCALAPPDATA%\Firewol\explorer.json` alongside existing pins.
- Windows release tests covered split transfers, conflicts, undo, tab detachment, all presets, the color wheel, menu editing and a custom executable action, plus settings migration and persistence.
- A release run on Windows 11 reached 91,344,896 bytes of sampled peak private working set with four windows, eight 100,000-row panes and a concurrent million-record indexing workload. The first list took 84 ms, scrolling reached 90 FPS and ready 100,000-item folder search p95 was 4.0 ms. A 256 MiB pane transfer also completed.
- Keep the combined private working set below 150,000,000 bytes. Moves use a same-volume destination. Copies of nonempty folders form an undo boundary.

## Next Immediate Step
Start M3 from PLAN.md with bounded parallel transfers, progress, pause and resume, then verified cross-volume moves and elevation policy. Exercise live UAC denial on a disposable NTFS volume.
