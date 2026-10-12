// Text for numbers, times and dates, in the language of the app.

import { getLang, t } from "./i18n";
import type { Lang } from "./i18n";

interface Formats {
  number1: Intl.NumberFormat;
  number0: Intl.NumberFormat;
  day: Intl.DateTimeFormat;
  dayYear: Intl.DateTimeFormat;
  time: Intl.DateTimeFormat;
  dateTime: Intl.DateTimeFormat;
}

// British English keeps the 24 hour clock, which is what every label of the app uses.
const LOCALE: Record<Lang, string> = { en: "en-GB", de: "de-DE" };
const cache = new Map<Lang, Formats>();

function formats(): Formats {
  const lang = getLang();
  let f = cache.get(lang);
  if (!f) {
    const locale = LOCALE[lang];
    f = {
      number1: new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }),
      number0: new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }),
      day: new Intl.DateTimeFormat(locale, { weekday: "long", day: "numeric", month: "long" }),
      dayYear: new Intl.DateTimeFormat(locale, { weekday: "long", day: "numeric", month: "long", year: "numeric" }),
      time: new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit", hour12: false }),
      dateTime: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }),
    };
    cache.set(lang, f);
  }
  return f;
}

/** A number with at most one decimal, with the decimal sign of the language. */
export function formatNumber(value: number): string {
  return formats().number1.format(value);
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  const f = formats();
  return `${value >= 100 ? f.number0.format(value) : f.number1.format(value)} ${units[unit]}`;
}

/** 75000 becomes "1:15", an hour and more "1:02:03". */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Minutes, seconds and the frame inside the second: "01:15:42". */
export function timecode(ms: number, fps: number): string {
  const rate = fps > 0 ? fps : 60;
  const frames = Math.max(0, Math.round((ms / 1000) * rate));
  const f = frames % rate;
  const totalSeconds = Math.floor(frames / rate);
  const s = totalSeconds % 60;
  const m = Math.floor(totalSeconds / 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(m)}:${pad(s)}:${pad(Math.floor(f))}`;
}

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function dayLabel(date: Date, now = new Date()): string {
  const days = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000);
  if (days === 0) return t("day.today");
  if (days === 1) return t("day.yesterday");
  const f = formats();
  return date.getFullYear() === now.getFullYear() ? f.day.format(date) : f.dayYear.format(date);
}

export function formatTime(date: Date): string {
  return formats().time.format(date);
}

export function formatDateTime(date: Date): string {
  return formats().dateTime.format(date);
}

export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

export function dirName(path: string): string {
  const cut = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return cut > 0 ? path.slice(0, cut) : path;
}

export function stem(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}
