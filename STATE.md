# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec. PLAN.md holds the execution plan and sources.
- M0 is complete on Windows 11 x64 with MSVC 19.44 and Windows SDK 10.0.26100.0.
- Win32 owner-data ListView and Direct2D are the default over the shared Rust core. With 100,000 NTFS files and 1 million index records, Tauri peaked at 163.4 MB and Win32 at 30.8 MB.
- The fixture prototype uses paged SQLite FTS5 search. Win32 returned the first list page in 224–268 ms and search p95 in 12.4–13.1 ms across three release runs.
- Windows verified exact-identity Recycle Bin restore, destination conflict handling, and a read-only elevated MFT probe for a synthetic file reference.
- The first usable release includes folder search and opt-in, read-only NTFS volume indexing.
- Keep the total private working set below 150,000,000 bytes. README.md describes the reproducible M0 workload.
- Nilesoft screenshots guide the default appearance and in-app menu. Store customization in local app data.

## Next Immediate Step
Build M1 on the Win32 adapter. Start with real folder navigation and cancellable paging, then connect item-identity operations and opt-in NTFS indexing.
