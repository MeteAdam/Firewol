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

export function typeName(entry: Entry): string {
  if (entry.directory) return "File folder";
  const dot = entry.name.lastIndexOf(".");
  return dot > 0 ? `${entry.name.slice(dot + 1).toUpperCase()} file` : "File";
}

export function location(entry: Entry): string {
  const slash = entry.path.lastIndexOf("\\");
  return slash > 0 ? entry.path.slice(0, slash) : entry.path;
}
