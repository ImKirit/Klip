// Turns the raw file list into clips and answers every question of the library view:
// what matches the filters, in which order, in which day group, what the cleanup takes.

import { dayKey } from "./format";
import { getLang, t, tOpt } from "./i18n";
import type { Key } from "./i18n";
import type { AppSettings, ClipEvent, ClipKind, LibraryItem, SortKey } from "./types";

/** A file of the library with everything resolved, sidecar or not. */
export interface Clip {
  item: LibraryItem;
  path: string;
  title: string;
  kind: ClipKind;
  created: Date;
  game: string | null;
  /** Why an automatic clip was saved, such as "ace". Null for everything saved by hand. */
  reason: string | null;
  durationMs: number;
  events: ClipEvent[];
  favorite: boolean;
  tags: string[];
  hasSidecar: boolean;
}

export function toClip(item: LibraryItem): Clip {
  const meta = item.meta;
  const created = meta ? new Date(meta.created) : new Date(item.modified_ms);
  // Without a sidecar the first folder below the clip folder stands in for the game.
  const folderGame = item.folder.split(/[\\/]/)[0] || null;
  return {
    item,
    path: item.path,
    title: item.name,
    kind: meta?.kind ?? (item.media === "image" ? "screenshot" : "clip"),
    created: Number.isNaN(created.getTime()) ? new Date(item.modified_ms) : created,
    game: meta ? (meta.game ?? null) : folderGame,
    reason: autoReason(meta),
    durationMs: meta?.duration_ms ?? 0,
    events: meta?.events ?? [],
    favorite: meta?.favorite ?? false,
    tags: meta?.tags ?? [],
    hasSidecar: meta !== null,
  };
}

/** Events worth a badge on the card, best first. */
const HIGHLIGHT_ORDER = ["ace", "clutch", "quadra_kill", "triple_kill", "double_kill", "match_win", "match_won", "kill", "death"];

/** Older automatic clips carry no reason of their own, their best event stands in. */
function autoReason(meta: LibraryItem["meta"]): string | null {
  if (!meta || meta.kind !== "auto_clip") return null;
  if (meta.reason) return meta.reason;
  const kinds = new Set((meta.events ?? []).map((e) => e.kind));
  return HIGHLIGHT_ORDER.find((k) => kinds.has(k)) ?? null;
}

/** The name shown for the game of a clip. Clips without a game were made on the desktop. */
export function gameLabel(game: string | null): string {
  return game || t("lib.desktop");
}

const KIND_KEY: Record<ClipKind, Key> = {
  clip: "kind.clip",
  recording: "kind.recording",
  screenshot: "kind.screenshot",
  auto_clip: "kind.auto_clip",
};

export function kindLabel(kind: ClipKind): string {
  return t(KIND_KEY[kind]);
}


export function eventLabel(kind: string): string {
  // Known kinds have an entry "event.<kind>" in the translation table.
  return tOpt(`event.${kind}`) ?? kind.replace(/_/g, " ");
}

export function highlightEvent(clip: Clip): string | null {
  const kinds = new Set(clip.events.map((e) => e.kind));
  return HIGHLIGHT_ORDER.find((k) => kinds.has(k)) ?? null;
}

export type DateRange = "all" | "today" | "week" | "month";
export type LengthRange = "all" | "short" | "medium" | "long";

export interface Filters {
  /** "all", "favorites" or a clip kind. */
  view: "all" | "favorites" | ClipKind;
  /** null means every game, "" means clips without a game. */
  game: string | null;
  date: DateRange;
  length: LengthRange;
  event: string | null;
  tag: string | null;
  search: string;
}

export const NO_FILTERS: Filters = { view: "all", game: null, date: "all", length: "all", event: null, tag: null, search: "" };

export function matches(clip: Clip, f: Filters, now = new Date()): boolean {
  if (f.view === "favorites" ? !clip.favorite : f.view !== "all" && clip.kind !== f.view) return false;
  if (f.game !== null && (clip.game ?? "") !== f.game) return false;
  if (f.date !== "all") {
    const start = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
    const days = f.date === "today" ? 0 : f.date === "week" ? 6 : 29;
    if (clip.created.getTime() < start - days * 86_400_000) return false;
  }
  if (f.length !== "all") {
    if (clip.kind === "screenshot") return false;
    const s = clip.durationMs / 1000;
    if (f.length === "short" && s >= 30) return false;
    if (f.length === "medium" && (s < 30 || s > 120)) return false;
    if (f.length === "long" && s <= 120) return false;
  }
  if (f.event !== null && !clip.events.some((e) => e.kind === f.event)) return false;
  if (f.tag !== null && !clip.tags.includes(f.tag)) return false;
  const words = f.search.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length > 0) {
    const haystack = [
      clip.title,
      clip.game ?? "",
      kindLabel(clip.kind),
      ...clip.tags,
      ...clip.events.flatMap((e) => [eventLabel(e.kind), e.label ?? ""]),
    ]
      .join(" ")
      .toLowerCase();
    if (!words.every((w) => haystack.includes(w))) return false;
  }
  return true;
}

export function sortClips(clips: Clip[], key: SortKey): Clip[] {
  const by: Record<SortKey, (a: Clip, b: Clip) => number> = {
    date_desc: (a, b) => b.created.getTime() - a.created.getTime(),
    date_asc: (a, b) => a.created.getTime() - b.created.getTime(),
    length_desc: (a, b) => b.durationMs - a.durationMs,
    length_asc: (a, b) => a.durationMs - b.durationMs,
    name: (a, b) => a.title.localeCompare(b.title, getLang()),
    size_desc: (a, b) => b.item.size - a.item.size,
  };
  return [...clips].sort(by[key]);
}

export interface DayGroup {
  key: string;
  date: Date;
  clips: Clip[];
}

/** Groups by calendar day and keeps the order the clips came in. */
export function groupByDay(clips: Clip[]): DayGroup[] {
  const groups = new Map<string, DayGroup>();
  for (const clip of clips) {
    const key = dayKey(clip.created);
    let group = groups.get(key);
    if (!group) {
      group = { key, date: clip.created, clips: [] };
      groups.set(key, group);
    }
    group.clips.push(clip);
  }
  return [...groups.values()];
}

export function countBy<T>(clips: Clip[], pick: (clip: Clip) => T[]): Map<T, number> {
  const counts = new Map<T, number>();
  for (const clip of clips) for (const value of pick(clip)) counts.set(value, (counts.get(value) ?? 0) + 1);
  return counts;
}

export interface CleanupPlan {
  remove: Clip[];
  bytes: number;
  /** Why each clip is on the list, by path. */
  reason: Map<string, "age" | "size">;
}

const GB = 1024 ** 3;

/** What the cleanup would move to the recycle bin. Favorites are never touched. */
export function planCleanup(clips: Clip[], rules: AppSettings["cleanup"], now = new Date()): CleanupPlan {
  const reason = new Map<string, "age" | "size">();
  const candidates = clips.filter((c) => !c.favorite).sort((a, b) => a.created.getTime() - b.created.getTime());
  if (rules.max_age_days > 0) {
    const limit = now.getTime() - rules.max_age_days * 86_400_000;
    for (const clip of candidates) if (clip.created.getTime() < limit) reason.set(clip.path, "age");
  }
  if (rules.max_size_gb > 0) {
    let total = clips.reduce((sum, c) => sum + (reason.has(c.path) ? 0 : c.item.size), 0);
    // Oldest first until the folder fits again.
    for (const clip of candidates) {
      if (total <= rules.max_size_gb * GB) break;
      if (reason.has(clip.path)) continue;
      reason.set(clip.path, "size");
      total -= clip.item.size;
    }
  }
  const remove = candidates.filter((c) => reason.has(c.path));
  return { remove, bytes: remove.reduce((sum, c) => sum + c.item.size, 0), reason };
}
