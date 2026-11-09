// HSV colour math for the settings colour wheel, ported from firewol-win32 explorer/color.rs.
export type Rgb = [number, number, number];
export type Hsv = { hue: number; saturation: number; value: number };

export const hexOf = ([r, g, b]: Rgb) => `#${[r, g, b].map((c) => c.toString(16).padStart(2, "0")).join("").toUpperCase()}`;

/** Parses `#RRGGBB` (the `#` is optional). Returns null for an empty value and undefined for invalid text. */
export function parseHex(text: string): Rgb | null | undefined {
  const value = text.trim();
  if (!value) return null;
  const digits = value.startsWith("#") ? value.slice(1) : value;
  if (!/^[0-9a-fA-F]{6}$/.test(digits)) return undefined;
  const number = Number.parseInt(digits, 16);
  return [(number >> 16) & 255, (number >> 8) & 255, number & 255];
}

export function rgbOf({ hue, saturation, value }: Hsv): Rgb {
  const h = hue * 6;
  const channel = (offset: number) => {
    const k = (((offset + h) % 6) + 6) % 6;
    return Math.round(255 * value * (1 - saturation * Math.min(Math.max(Math.min(k, 4 - k), 0), 1)));
  };
  return [channel(5), channel(3), channel(1)];
}

export function hsvOf([r, g, b]: Rgb): Hsv {
  const [red, green, blue] = [r / 255, g / 255, b / 255];
  const value = Math.max(red, green, blue);
  const delta = value - Math.min(red, green, blue);
  let hue = 0;
  if (delta !== 0) {
    if (value === red) hue = (green - blue) / delta;
    else if (value === green) hue = (blue - red) / delta + 2;
    else hue = (red - green) / delta + 4;
  }
  return { hue: (((hue % 6) + 6) % 6) / 6, saturation: value === 0 ? 0 : delta / value, value };
}

/** The wheel colour under (x, y), measured from the wheel centre. */
export function hsvAt(x: number, y: number, radius: number, value: number): Hsv {
  const angle = Math.atan2(y, x) / (2 * Math.PI);
  return { hue: ((angle % 1) + 1) % 1, saturation: Math.min(Math.hypot(x, y) / radius, 1), value };
}

/** Where a colour sits on the wheel, relative to the centre. */
export function pointOf({ hue, saturation }: Hsv, radius: number) {
  const angle = hue * 2 * Math.PI;
  return { x: Math.cos(angle) * saturation * radius, y: Math.sin(angle) * saturation * radius };
}

/** Reads a CSS colour token (`#rgb`, `#rrggbb` or `rgb()`) into RGB, or null for gradients and blends. */
export function cssRgb(value: string): Rgb | null {
  const text = value.trim();
  const short = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(text);
  if (short) return short.slice(1).map((digit) => Number.parseInt(digit + digit, 16)) as Rgb;
  const long = parseHex(text);
  if (long && text.startsWith("#")) return long;
  const rgb = /^rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*(1|1\.0+))?\)$/i.exec(text);
  return rgb ? [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])] : null;
}

/** Relative luminance contrast for two opaque sRGB colours. */
export function contrastRatio(first: Rgb, second: Rgb): number {
  const luminance = (rgb: Rgb) => {
    const [r, g, b] = rgb.map((byte) => {
      const channel = byte / 255;
      return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const a = luminance(first);
  const b = luminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}
