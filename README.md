# Firewol

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%2011%20x64-0078D4.svg)](https://microsoft.com/windows)
[![Rust](https://img.shields.io/badge/Rust-2024%20Edition-DEA584.svg)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-v2-FFC131.svg)](https://v2.tauri.app/)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00.svg)](https://svelte.dev/)

**Firewol** is a blazing-fast, lightweight, and calm file manager built specifically for Windows 11. Engineered from the ground up in Rust and Tauri, Firewol eliminates the sluggishness, unresponsive searches, and forced clutter of the standard Windows Explorer while providing instant search, verified file transfers, and uncompromising visual and workflow customization.

---

## Why Firewol?

Windows Explorer has increasingly grown cluttered, slow, and rigid. Deep directory listings often freeze, searches can take minutes to scan a drive, and unwanted items repeatedly clutter the sidebar. 

Firewol was created with a clear philosophy:
* **Calm by Default:** No unremovable ribbon buttons, no forced "Home" page, and no automatic "Quick Access" clutter. The sidebar only contains folders you choose to pin.
* **Instantaneous Search:** Search millions of files across an entire volume in milliseconds through direct NTFS USN journal indexing and SQLite FTS5.
* **Speed & Efficiency:** Strictly budgeted memory usage and virtualized rendering ensure buttery-smooth 120+ FPS scrolling, even in folders with 100,000+ files.
* **Verified Integrity:** Built-in multi-lane file transfers with SHA-256 pre-publish integrity checks, pause/resume, and safe rollbacks.
* **Deep Personalization:** Seven distinct aesthetic presets (from Light-morphism to Neumorphism and Skeuomorphism), an interactive HSV color wheel, per-folder visual styles, and an editable context menu.
* **Privacy & Offline First:** Zero telemetry, no cloud dependencies, and no background phone-home services.

---

## Key Features

### ⚡ Instant Search & NTFS Indexing
* **Disk-Backed SQLite FTS5:** Blazing-fast substring, prefix, and regular-expression searches across active folders.
* **Granular Filtering:** Filter results on the fly by file extension, file size boundaries, and modification dates.
* **Whole-Volume NTFS Search:** Opt-in to search entire physical NTFS drives via an isolated, read-only UAC helper reading the USN journal (`FSCTL_ENUM_USN_DATA`). Millions of entries are queried with instant results.

### 🪟 Fluid, Virtualized Dual-Pane Interface
* **Split View:** Browse and transfer between two independent directory panes side by side with a single click or shortcut.
* **Flexible Tabs:** Drag to reorder, detach tabs into independent windows, or close with middle-click. Built-in window resource management keeps memory usage predictable.
* **Virtualized Details & Grid Views:** Seamlessly switch between a dense tabular details view and scalable tile grids (96 px, 128 px, or 160 px). Virtualized rendering ensures large directories never degrade performance.
* **Asynchronous Shell Icons:** System file icons and large thumbnails load asynchronously from the Windows Shell cache without blocking UI interaction.
* **Accessible & Keyboard-First:** Built-in UI Automation support and complete keyboard navigation across all views, breadcrumbs, and sheets.

### 🛡️ Verified Transfers & Safe Operations
* **Multi-Lane Engine:** Local regular files transfer in parallel lanes utilizing Windows `CopyFile2`.
* **Live Transfers Popover:** Click the status bar transfer pill to view real-time throughput graphs, progress events, and time-remaining estimates, with instant Pause, Resume, or Cancel controls.
* **SHA-256 Verification:** Staged destinations are checksum-verified before publication. Cross-volume moves verify target data before safely removing source files.
* **Atomic Undo & Exact Recycle:** Multi-step `Ctrl+Z` undo support with result toasts and accurate Recycle Bin restoration for deleted items.
* **In-Window Drag & Drop:** Intuitive pointer drag-and-drop between panes, tabs, pinned folders, and subdirectories.

### 🎨 Design Presets & Folder Customization
* **7 Design Themes:**
  * **Light-morphism** (Refined, quiet light surfaces with subtle shadows)
  * **Glassmorphism** (Translucent acrylic backdrop blur)
  * **Neumorphism** (Soft extruded surfaces and inset selection fields)
  * **Neubrutalism** (Bold high-contrast borders and solid offset shadows)
  * **Claymorphism** (Tactile, puffy 3D curves and pastel highlights)
  * **Skeuomorphism** (Rich leather textures and embossed details)
  * **Metal-morphism** (Brushed aluminium gradients and chrome accents)
* **Live Color Wheel:** Fine-tune background, surface, text, and accent colors with real-time previews.
* **Folder Styles Sheet:** Customize any folder with custom display aliases, icon color tints, icon glyphs, and tags saved by persistent file identity.
* **Context Menu Editor:** Nilesoft-inspired clean context menu layout. Reorder entries, hide unused options, or register custom command actions.

### 🔒 Granular Security & Elevation Policy
* Runs securely under standard user privileges by default.
* Configure administrative elevation policies: allowlist specific binaries or require explicit consent, keeping elevated permissions strictly scoped.

---

## Project Architecture

Firewol is organized as a modular Rust workspace:

```
Firewol/
├── crates/
│   └── firewol-core/      # Pure Rust engine: NTFS USN reader, SQLite FTS5 search,
│                          # directory enumeration, transfers, session state & elevation
├── src-tauri/             # Tauri v2 backend, IPC contract & TypeScript type generation (ts-rs)
├── src/                   # Frontend UI built with Svelte 5, TypeScript & Vite
└── Cargo.toml             # Rust workspace configuration
```

---

## Getting Started

### Prerequisites

* **Operating System:** Windows 11 x64 (or Windows 10 with WebView2 Runtime installed)
* **Rust Toolchain:** Stable Rust with the `x86_64-pc-windows-msvc` target and C++ Build Tools
* **Node.js:** Node.js 18+ and npm

### Installation & Development

1. **Clone the repository:**
   ```powershell
   git clone https://github.com/MeteAdam/Firewol.git
   cd Firewol
   ```

2. **Install frontend dependencies:**
   ```powershell
   npm ci
   ```

3. **Run in development mode (with Hot Module Replacement):**
   ```powershell
   npm run tauri dev
   ```

### Building for Release

To produce an optimized, standalone Windows binary:

```powershell
# Build the Svelte frontend
npm run build

# Build the release executable (custom-protocol embeds dist/ instead of the dev server)
cargo build --release --locked -p firewol-tauri --features custom-protocol
```

The compiled executable will be located at:
```
.\target\release\firewol-tauri.exe
```

The native Win32 interface that served as the reference until the Tauri interface reached parity was removed in M4.4. It remains available at the Git tag `m3-win32-final`.

---

## Keyboard Shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl + T` | Open new tab |
| `Ctrl + W` | Close active tab |
| `Ctrl + Tab` / `Ctrl + Shift + Tab` | Switch between tabs |
| `Ctrl + Shift + N` | Detach active tab into a new window |
| `Ctrl + L` / `Alt + D` | Focus and edit address / breadcrumb bar |
| `Ctrl + F` | Focus search bar |
| `Alt + Left` / `Alt + Right` | Navigate Back / Forward |
| `Alt + Up` | Navigate to parent directory |
| `F2` | Inline rename selected item |
| `Delete` | Send selected item to Recycle Bin |
| `Ctrl + C` / `Ctrl + X` / `Ctrl + V` | Copy / Cut / Paste items |
| `Ctrl + Z` | Undo last file operation |
| `F5` | Refresh current folder listing |
| `Ctrl + Shift + 2 / 3 / 4 / 6` | Switch grid tile sizes (or Details view) |
| `Ctrl + Wheel` | Zoom grid tile sizes |

---

## Configuration & Storage

Firewol stores its user preferences, folder styling metadata, and search caches locally in your user profile:
```
%LOCALAPPDATA%\Firewol\
```
Settings are stored in standard JSON format, allowing straightforward backup or inspection.

---

## License

This project is licensed under the [MIT License](LICENSE).
