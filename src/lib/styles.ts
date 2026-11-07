// Folder styles (alias, icon colour, icon glyph, tags), keyed by folder identity as in the core.
import type { FolderStyleView, ItemRef, SettingsView } from "./contract";
import { hexOf, type Rgb } from "./color";

type Identity = Pick<ItemRef, "volume" | "index">;

/** Mirrors `FolderStyle::normalized` in firewol-core settings.rs, which has the final say. */
export const LIMITS = { alias: 100, icon: 8, tag: 32, tags: 8 } as const;

export const SWATCHES: [string, Rgb][] = [
  ["Red", [229, 72, 77]], ["Orange", [242, 140, 40]], ["Yellow", [230, 184, 0]],
  ["Green", [46, 160, 90]], ["Teal", [20, 160, 160]], ["Blue", [47, 127, 224]],
  ["Violet", [139, 92, 246]], ["Pink", [224, 90, 156]], ["Slate", [100, 116, 139]],
];
export const GLYPHS = ["★", "◆", "●", "▲", "♥", "⚑", "✓", "☁"];

export const styleKey = (id: Identity) => `${id.volume}:${id.index}`;
export const styleMap = (settings?: SettingsView) =>
  new Map((settings?.folders ?? []).map((folder) => [styleKey(folder), folder.style]));
export const hasTags = (style?: FolderStyleView | null) => !!style?.tags.length;
/** The icon tint for `FolderIcon`; undefined keeps the theme's folder colour. */
export const tint = (style?: FolderStyleView | null) => (style?.color ? hexOf(style.color) : undefined);
export const splitTags = (text: string) => text.split(",").map((tag) => tag.trim()).filter(Boolean);
