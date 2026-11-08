// URLs of Shell icons. A version per key makes every window reload an icon that changed.
import { SvelteMap } from "svelte/reactivity";

const versions = new SvelteMap<string, number>();

export function iconChanged(key: string) {
  versions.set(key, (versions.get(key) ?? 0) + 1);
}

/** The `icon://` URL for `key`, sized for `scale` device pixels; "" when there is no key. */
export function iconSource(key: string | null | undefined, size: number, scale: number) {
  if (!key) return "";
  const pixels = Math.min(256, Math.max(16, Math.ceil(size * scale)));
  const version = versions.get(key);
  return `http://icon.localhost/?key=${encodeURIComponent(key)}&s=${pixels}${version ? `&v=${version}` : ""}`;
}
