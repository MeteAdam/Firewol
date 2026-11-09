import type { Entry } from "./contract";

const UNITS = ["B", "KB", "MB", "GB", "TB"];
let date: Intl.DateTimeFormat;
let decimal: Intl.NumberFormat;
let whole: Intl.NumberFormat;
useLocale(undefined);

/** Dates and numbers follow the Windows regional format, which the core reports at startup. */
export function useLocale(locale: string | undefined) {
  date = new Intl.DateTimeFormat(locale, {
    day: "numeric",
    month: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
  decimal = new Intl.NumberFormat(locale, { maximumFractionDigits: 1 });
  whole = new Intl.NumberFormat(locale);
}

export function formatDate(seconds: number | null): string {
  return seconds === null ? "" : date.format(seconds * 1000);
}

export function formatSize(bytes: number | null): string {
  if (bytes === null) return "";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${unit === 0 ? whole.format(value) : decimal.format(value)} ${UNITS[unit]}`;
}

export function formatCount(count: number): string {
  return whole.format(count);
}

/** Text-direction controls can make `invoice_\u202Efdp.exe` read as a PDF, so they are shown. */
export function visibleName(name: string): string {
  return name.replace(/[\u202A-\u202E\u2066-\u2069]/g, (mark) => `[U+${mark.charCodeAt(0).toString(16).toUpperCase()}]`);
}

const RUNNABLE = new Set(["bat", "cmd", "com", "exe", "hta", "jar", "js", "jse", "lnk", "msi", "pif", "ps1", "reg", "scr", "url", "vbe", "vbs", "wsf"]);

/** Hides a name's extension, unless that would make a program look like a document. */
export function withoutExtension(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 && !RUNNABLE.has(name.slice(dot + 1).toLowerCase()) ? name.slice(0, dot) : name;
}

export function typeName(entry: Entry): string {
  if (entry.directory) return "File folder";
  const dot = entry.name.lastIndexOf(".");
  return dot > 0 ? `${entry.name.slice(dot + 1).toUpperCase()} file` : "File";
}

export function location(entry: Entry): string {
  const slash = entry.path.lastIndexOf("\\");
  return slash > 0 ? entry.path.slice(0, slash) : entry.path;
}
