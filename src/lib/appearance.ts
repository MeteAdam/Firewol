import type { AppearanceView } from "./contract";

export const PRESETS = [
  ["light_morphism", "light", "Light-morphism"], ["glassmorphism", "glass", "Glassmorphism"],
  ["neumorphism", "neu", "Neumorphism"], ["neubrutalism", "brutal", "Neubrutalism"],
  ["claymorphism", "clay", "Claymorphism"], ["skeuomorphism", "skeuo", "Skeuomorphism"],
  ["metal_morphism", "metal", "Metal-morphism"],
] as const;
export const presetClass = (preset: AppearanceView["preset"]) => PRESETS.find(([id]) => id === preset)?.[1] ?? "light";
export const hex = (rgb: number[] | null) => rgb ? `#${rgb.map(c => c.toString(16).padStart(2, "0")).join("")}` : "";
export function colors(node: HTMLElement, appearance?: AppearanceView) {
  const apply = (value?: AppearanceView) => {
    const slots = { background: "bg", surface: "surface", text: "text", accent: "accent", folder: "folder", folder_label: "folder-label", caption_fill: "caption-fill", caption_hover: "caption-hover" } as const;
    for (const [field, token] of Object.entries(slots)) {
      const color = value?.[field as keyof typeof slots];
      if (color) node.style.setProperty(`--${token}`, hex(color));
      else node.style.removeProperty(`--${token}`);
    }
  };
  apply(appearance);
  return { update: apply };
}
