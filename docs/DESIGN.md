# Firewol M4 Interface Design

This specification defines the M4 interface independently of the rendering technology. The Tauri v2 + Svelte build and the Direct2D fallback implement the same anatomy, tokens and behaviour. Dolphin is a benchmark for calm, uncluttered layout only; Firewol does not copy its structure, icons or visuals.

## Principles

1. **Calm by default.** One toolbar row. Commands that are not used every minute live in the `⋯` menu, the context menu or Settings, never as permanent buttons.
2. **Content first.** The file list owns the window. Chrome sits on the window background; the list sits on one raised content surface.
3. **Keyboard first.** Every command has a shortcut or is reachable with Tab and arrow keys. Focus is always visible.
4. **No imposed places.** No Home page, no automatic Quick Access. The sidebar contains only folders the user pinned.
5. **Themes change materials, not layout.** All seven presets share one anatomy and one token contract.

## Window anatomy

| Region | Height / width | Contents |
| --- | --- | --- |
| Title bar | 44 px | Tabs (drag to reorder or detach), `+` new tab, caption buttons. Caption side, shape, fill and hover come from `Appearance`. |
| Toolbar | 52 px | Sidebar toggle · Back · Forward · Up · breadcrumb address · search field · view switch (details / grid) · split view · `⋯` |
| Sidebar | 232 px (resizable 180–360) | "Pinned" header with `+` (pin current folder); pinned folders with alias, colour and icon. Hidden in zen mode. |
| Content | flexible | One raised surface (radius from token). Details view: Name · Date modified · Type · Size · Tags. Search adds Location. Grid view: 96/128/160 px tiles. |
| Status bar | 32 px | Item and selection counts · transfer pill · free space of the current drive. |

Split view places two content surfaces side by side with a 12 px gutter; each pane has its own breadcrumb, search and tabs. The active pane has an accent outline.

## Components

- **Tab.** Folder glyph, label (ellipsis), close button on hover and on the active tab. Active tab uses `--tab-active`. Middle-click closes.
- **Icon button.** 32 × 32, 16 px glyph, radius `--radius-s`. Hover `--hover`, pressed `--sel`, disabled 40 % opacity. Every icon button has a tooltip with its shortcut.
- **Breadcrumb.** Drive glyph, then segments separated by chevrons. Clicking a segment navigates; clicking a chevron lists sibling folders; clicking empty space or Ctrl+L turns the field into an editable path. Overflowing segments collapse into `…` at the start.
- **Search field.** 280 px, magnifier glyph, placeholder "Search *folder name*", `Ctrl F` hint. While active: scope chip ("This folder" or "C: · NTFS"), Regex toggle and Filters popover (extension, size, modified date). NTFS search is offered from the scope chip, never as a toolbar button.
- **File list.** 32 px rows (28 compact, 36 comfortable), 16 px icons, sticky 32 px header with sort arrow. Selection uses `--sel` with `--sel-text`; hover uses `--hover`. Inline rename (F2) edits the name cell in place. Folder colours, aliases and tags come from folder styles.
- **Context menu.** Nilesoft-inspired: neutral surface, a compact icon row (Cut, Copy, Paste, Rename, Recycle), then 28 px rows with 16 px glyph, label and right-aligned shortcut, fine separators between groups, and an "Edit menu…" footer. Only applicable actions are shown. Custom actions carry their own glyph colour. M4 adds two built-in actions: **Open in new tab** (Ctrl+Enter) and **Pin to sidebar**.
- **Status bar transfer pill.** Shows the active transfer (items, percentage, speed, thin progress bar). Clicking it opens the Transfers popover with the speed graph, Pause / Resume / Cancel and recent results. No separate Transfers window.
- **Undo toast.** After recycle, move or rename: "Moved 3 items to the Recycle Bin · Undo (Ctrl+Z)" for 6 s, bottom centre.
- **Settings.** Opens as a tab, not a dialog. Sections: Appearance, Context menu, Search, Transfers, Administrator, About. Changes apply live; there is no Apply button. Destructive or risky choices (Administrator "Any executable") ask for confirmation inline.
- **Confirmations.** In-window sheets anchored to the content surface; OS dialogs only where Windows requires them (UAC).

## Typography and spacing

- Font: Segoe UI Variable (Text optical size for ≤ 14 px, Display for ≥ 20 px), falling back to Segoe UI.
- Scale: caption 12/16, list 13/20, body 14/20, subtitle 20/28 semibold, title 28/36 semibold.
- Spacing: 4 px grid; standard gaps 4, 8, 12, 16, 24, 32.
- Dates and numbers follow the Windows regional format (`GetUserDefaultLocaleName`), not the WebView2 display language: `11/7/2026, 12:46 PM` and `14.6 KB` for en-US, `7.11.2026 12:46` and `14,6 KB` for tr-TR.

## Iconography

One bundled outline icon set (16 px, 1.5–1.75 px stroke, round caps), shipped as an SVG sprite so it renders identically on every Windows build. File-type icons come from the Windows Shell icon cache through the core, so they match the user's associations.

## Theme token contract

Every surface reads these tokens. The core's `Appearance` overrides map onto them: `background → --bg`, `surface → --surface`, `text → --text`, `accent → --accent`, `folder → --folder`, `folder_label → --folder-label`; caption fields style the caption buttons only.

| Token | Meaning |
| --- | --- |
| `--bg` | Window background (colour, gradient or texture). |
| `--surface`, `--surface-2` | Content surface; raised popovers and menus. |
| `--field`, `--field-shadow` | Inputs (breadcrumb, search). |
| `--text`, `--muted` | Primary and secondary text (≥ 4.5:1 on their surfaces). |
| `--border`, `--border-w`, `--sep` | Outlines and their width; separators inside surfaces. |
| `--accent`, `--on-accent` | Accent and text on accent. |
| `--sel`, `--sel-text`, `--sel-shadow`, `--hover` | Selection and hover. |
| `--folder`, `--folder-label` | Default folder glyph and name colour. |
| `--radius`, `--radius-s` | Surface and control radii. |
| `--shadow`, `--shadow-s`, `--shadow-pop` | Surface, small control and popover elevation. |
| `--tab-active` | Active tab background. |
| `--blur` | Backdrop filter for translucent surfaces. |
| `--stitch`, `--emboss` | Skeuomorphic stitching and text emboss. |

### Presets

| Preset | Material | Key values |
| --- | --- | --- |
| Light-morphism (default) | Quiet light surfaces on a soft grey base, hairline borders, low shadows. Dark variant. | radius 12, accent `#2468CA` |
| Glassmorphism | Translucent panels over a colourful backdrop; Windows Acrylic in the title area. | blur 24 px, radius 16, accent `#98D1FF` |
| Neumorphism | Surfaces share the base colour; paired light and dark shadows; inset fields and selection. Dark variant. | radius 18 |
| Neubrutalism | Pale yellow base, 2 px black outlines, hard offset shadows, high-contrast accent. | radius 6, shadow `5px 5px 0` |
| Claymorphism | Pastel, very round, puffy outer shadows plus inner highlights. | radius 24 |
| Skeuomorphism | Dark leather texture, stitched panel edges, embossed text, amber selection. | radius 10 |
| Metal-morphism | Brushed aluminium gradients, chrome highlights, steel-blue selection. | radius 8 |

Exact values live in one token file shared by the interface and the core's preset defaults. Until that file exists, the visual reference is the private mockup canvas [Firewol M4 UI Mockup](https://claude.ai/artifact/T2CUmhLXg3w2S4een7q3wo) (main window, Settings › Appearance, all presets), whose CSS custom properties are the draft token values.

## Motion

120–180 ms ease-out for hover, popovers and tab moves; no motion for list scrolling or selection. `prefers-reduced-motion` disables transitions.

## Accessibility and acceptance

- Contrast ≥ 4.5:1 for text, ≥ 3:1 for icons and focus rings in every preset.
- 2 px focus ring in `--accent`, 2 px offset.
- The virtual list exposes a grid with `aria-rowcount` and `aria-rowindex` so assistive technology never enumerates unrealised rows. A 100,000-row folder with Narrator or Inspect running must stay responsive (M4 acceptance).
- The grid is its own scroll container with a sticky header, so UI Automation exposes a scrollable table. Rows are keyed by index: scrolling only adds and removes edge rows, which keeps accessibility events proportional to what enters the view.
- Per-monitor DPI v2; all sizes above are at 100 % scale.

## Non-goals

No ribbon, no Home page, no automatic Quick Access, no permanent buttons for rarely used commands, no Windows Explorer shell-extension hosting in M4.
