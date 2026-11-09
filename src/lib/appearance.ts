import type { AppearancePatch, AppearanceView } from "./contract";
import { contrastRatio } from "./color";

export const PRESETS = [
  ["light_morphism", "light", "Light-morphism"], ["glassmorphism", "glass", "Glassmorphism"],
  ["neumorphism", "neu", "Neumorphism"], ["neubrutalism", "brutal", "Neubrutalism"],
  ["claymorphism", "clay", "Claymorphism"], ["skeuomorphism", "skeuo", "Skeuomorphism"],
  ["metal_morphism", "metal", "Metal-morphism"],
] as const;
export const presetClass = (preset: AppearanceView["preset"]) => PRESETS.find(([id]) => id === preset)?.[1] ?? "light";
export const hex = (rgb: number[] | null) => rgb ? `#${rgb.map(c => c.toString(16).padStart(2, "0")).join("")}` : "";
export const COLOR_GROUPS = [
  [
    "Text",
    [
      ["text", "Text", "text"],
      ["muted", "Secondary text", "muted"],
      ["folder_label", "Folder label", "folder-label"],
      ["danger", "Error text", "danger"]
    ]
  ],
  [
    "Surfaces",
    [
      ["background", "Background", "bg"],
      ["surface", "Surface", "surface"],
      ["surface_2", "Popup surface", "surface-2"],
      ["field", "Input surface", "field"],
      ["border", "Border", "border"]
    ]
  ],
  [
    "Tabs and window controls",
    [
      ["titlebar", "Title bar", "titlebar"],
      ["tab_active", "Active tab fill", "tab-active"],
      ["tab_active_text", "Active tab text", "tab-active-text"],
      ["tab_inactive", "Inactive tab fill", "tab-inactive"],
      ["tab_inactive_text", "Inactive tab text", "tab-inactive-text"],
      ["caption_fill", "Button fill", "caption-fill"],
      ["caption_text", "Button icon", "caption-text"],
      ["caption_hover", "Close hover", "caption-hover"]
    ]
  ],
  [
    "Interaction states",
    [
      ["accent", "Accent", "accent"],
      ["on_accent", "Accent text", "on-accent"],
      ["folder", "Folder icon", "folder"],
      ["selection", "Selection fill", "sel"],
      ["selection_text", "Selection text", "sel-text"],
      ["hover", "Hover fill", "hover"]
    ]
  ]
] as const;

export type ColorSlot = (typeof COLOR_GROUPS)[number][1][number][0];
type ColorControl = (typeof COLOR_GROUPS)[number][1][number];
export const COLOR_SLOTS = COLOR_GROUPS.flatMap<ColorControl>(([, slots]) => slots);
export const paletteOf = (appearance?: AppearanceView) => appearance?.palettes[appearance.preset] ?? {};

export function mergeAppearance(base: AppearanceView, patch: AppearancePatch): AppearanceView {
  const { palettes: changes, preset, caption_shape, ...fields } = patch;
  const palettes = { ...base.palettes };
  for (const [theme, change] of Object.entries(changes ?? {})) {
    const palette = { ...palettes[theme] };
    for (const [slot, color] of Object.entries(change ?? {})) {
      if (color) palette[slot] = color;
      else delete palette[slot];
    }
    palettes[theme] = palette;
  }
  return {
    ...base, ...fields, palettes,
    preset: (preset ?? base.preset) as AppearanceView["preset"],
    caption_shape: (caption_shape ?? base.caption_shape) as AppearanceView["caption_shape"],
  };
}

export function colors(node: HTMLElement, appearance?: AppearanceView) {
  const apply = (value?: AppearanceView) => {
    const palette = paletteOf(value);
    if (palette.caption_hover) {
      const light = contrastRatio(palette.caption_hover, [255, 255, 255]);
      const dark = contrastRatio(palette.caption_hover, [0, 0, 0]);
      node.style.setProperty("--caption-hover-text", light >= dark ? "#fff" : "#000");
    } else node.style.removeProperty("--caption-hover-text");
    if (palette.border) node.style.setProperty("--border-w", "max(1px, var(--preset-border-w))");
    else node.style.removeProperty("--border-w");
    for (const [field, , token] of COLOR_SLOTS) {
      const color = palette[field];
      if (color) node.style.setProperty(`--${token}`, hex(color));
      else node.style.removeProperty(`--${token}`);
    }
  };
  apply(appearance);
  return { update: apply };
}
