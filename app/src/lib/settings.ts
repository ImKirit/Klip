// App settings (`app.json`) with their defaults, and the theme. The accent colour is
// not in here: it lives in `config.json`, because the tray and the overlay share it.

import type { AppSettings } from "./types";

/** Same value as `DEFAULT_ACCENT` in klip-core. */
export const DEFAULT_ACCENT = "#8B5CFF";

export const DEFAULT_SETTINGS: AppSettings = {
  version: 1,
  theme: "dark",
  ffmpeg_path: "",
  storage_limit_gb: 0,
  cleanup: { enabled: false, max_age_days: 0, max_size_gb: 0 },
  export_dir: "",
  sort: "date_desc",
};

/** The two presets of the design. Any other colour can be picked freely. */
export const ACCENT_PRESETS: { color: string; name: "violet" | "blue" }[] = [
  { color: "#8B5CFF", name: "violet" },
  { color: "#3B82FF", name: "blue" },
];

function num(value: unknown, fallback: number, min: number, max: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(min, Math.min(max, value)) : fallback;
}

/** Fills gaps and throws out nonsense, whatever the file contained. An accent colour
 *  left in the file by an older version is dropped here. */
export function normalizeSettings(raw: unknown): AppSettings {
  const d = DEFAULT_SETTINGS;
  const r = (raw && typeof raw === "object" ? raw : {}) as Partial<AppSettings>;
  const cleanup = (r.cleanup && typeof r.cleanup === "object" ? r.cleanup : {}) as Partial<AppSettings["cleanup"]>;
  const sorts = ["date_desc", "date_asc", "length_desc", "length_asc", "name", "size_desc"];
  return {
    version: 1,
    theme: r.theme === "light" ? "light" : "dark",
    ffmpeg_path: typeof r.ffmpeg_path === "string" ? r.ffmpeg_path : "",
    storage_limit_gb: num(r.storage_limit_gb, 0, 0, 100_000),
    cleanup: {
      enabled: cleanup.enabled === true,
      max_age_days: num(cleanup.max_age_days, 0, 0, 3650),
      max_size_gb: num(cleanup.max_size_gb, 0, 0, 100_000),
    },
    export_dir: typeof r.export_dir === "string" ? r.export_dir : "",
    sort: sorts.includes(r.sort as string) ? (r.sort as AppSettings["sort"]) : d.sort,
  };
}

/** The accent as "#RRGGBB" in capitals, the default for anything else. */
export function cleanAccent(value: unknown): string {
  return typeof value === "string" && /^#[0-9a-fA-F]{6}$/.test(value) ? value.toUpperCase() : DEFAULT_ACCENT;
}

/** Perceived brightness of a hex colour, 0 to 1. */
export function luminance(hex: string): number {
  const channel = (i: number) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

/** Text colour that stays readable on the accent: white, or near black on a light accent. */
export function onAccent(accent: string): string {
  return luminance(accent) > 0.4 ? "#0A0A0F" : "#FFFFFF";
}

/** Everything that is accent hangs on one variable, the text on it follows its brightness. */
export function applyTheme(theme: AppSettings["theme"], accent: string): void {
  const root = document.documentElement;
  const color = cleanAccent(accent);
  root.dataset.theme = theme;
  root.style.setProperty("--ac", color);
  root.style.setProperty("--ac-tx", onAccent(color));
}
