// The cut list of the editor. A clip is a row of segments that touch each other, from
// the trim start to the trim end. Cutting splits a segment, deleting switches one off.
// Nothing here touches the video file.

import { DEFAULT_OVERLAY } from "./inputs";
import type { OverlayConfig } from "./inputs";
import type { CameraEdit, Corner, EditDoc, Segment, TrackEdit } from "./types";

function between(value: unknown, min: number, max: number, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(min, Math.min(max, value)) : fallback;
}

export function normalizeOverlay(raw: Partial<OverlayConfig>): OverlayConfig {
  const d = DEFAULT_OVERLAY;
  const scopes = ["wasd", "left_hand", "sixty", "arrows", "full", "none"];
  return {
    enabled: raw.enabled === true,
    keyboard: raw.keyboard === "qwerty" ? "qwerty" : "qwertz",
    scope: scopes.includes(raw.scope as string) ? raw.scope! : d.scope,
    mouse: raw.mouse !== false,
    x: between(raw.x, 0, 1, d.x),
    y: between(raw.y, 0, 1, d.y),
    scale: between(raw.scale, 0.05, 1, d.scale),
    opacity: between(raw.opacity, 0.1, 1, d.opacity),
  };
}

const CORNERS: Corner[] = ["top_left", "top_right", "bottom_left", "bottom_right"];

export function normalizeCamera(raw: Partial<CameraEdit>, fallback?: CameraEdit): CameraEdit {
  return {
    enabled: typeof raw.enabled === "boolean" ? raw.enabled : (fallback?.enabled ?? true),
    corner: CORNERS.includes(raw.corner as Corner) ? (raw.corner as Corner) : (fallback?.corner ?? "bottom_right"),
    size_percent: between(raw.size_percent, 10, 60, fallback?.size_percent ?? 25),
  };
}

/** Segments shorter than this are not created, they could not be grabbed anyway. */
const MIN_SEGMENT_MS = 40;

export function defaultTracks(streams: number[]): TrackEdit[] {
  return streams.map((stream) => ({ stream, muted: false, solo: false, volume: 1 }));
}

export function defaultEdit(durationMs: number, streams: number[]): EditDoc {
  return { version: 1, segments: [{ start_ms: 0, end_ms: durationMs, enabled: true }], tracks: defaultTracks(streams) };
}

/** Repairs whatever was read from disk: sorted, inside the clip, touching, known tracks. */
export function normalizeEdit(raw: unknown, durationMs: number, streams: number[]): EditDoc {
  const fallback = defaultEdit(durationMs, streams);
  if (!raw || typeof raw !== "object") return fallback;
  const doc = raw as Partial<EditDoc>;
  const segments: Segment[] = [];
  const input = Array.isArray(doc.segments) ? [...doc.segments] : [];
  input.sort((a, b) => a.start_ms - b.start_ms);
  for (const s of input) {
    const start = Math.max(0, Math.min(durationMs, Math.round(Number(s.start_ms))));
    const end = Math.max(0, Math.min(durationMs, Math.round(Number(s.end_ms))));
    const from = segments.length > 0 ? segments[segments.length - 1].end_ms : start;
    if (!Number.isFinite(start) || !Number.isFinite(end) || end - from < 1) continue;
    segments.push({ start_ms: from, end_ms: end, enabled: s.enabled !== false });
  }
  const tracks = defaultTracks(streams).map((track) => {
    const saved = Array.isArray(doc.tracks) ? doc.tracks.find((t) => t.stream === track.stream) : undefined;
    if (!saved) return track;
    const volume = Number.isFinite(saved.volume) ? Math.max(0, Math.min(2, saved.volume)) : 1;
    return { stream: track.stream, muted: saved.muted === true, solo: saved.solo === true, volume };
  });
  const edit: EditDoc = { version: 1, segments: segments.length > 0 ? segments : fallback.segments, tracks };
  if (doc.input_overlay && typeof doc.input_overlay === "object") edit.input_overlay = normalizeOverlay(doc.input_overlay);
  if (doc.camera && typeof doc.camera === "object") edit.camera = normalizeCamera(doc.camera);
  return edit;
}

export function isUntouched(edit: EditDoc, durationMs: number): boolean {
  const s = edit.segments;
  const whole = s.length === 1 && s[0].enabled && s[0].start_ms <= 0 && s[0].end_ms >= durationMs;
  return whole && !edit.input_overlay && edit.tracks.every((t) => !t.muted && !t.solo && t.volume === 1);
}

export function trimStart(edit: EditDoc): number {
  return edit.segments[0].start_ms;
}

export function trimEnd(edit: EditDoc): number {
  return edit.segments[edit.segments.length - 1].end_ms;
}

function withSegments(edit: EditDoc, segments: Segment[]): EditDoc {
  return { ...edit, segments };
}

export function segmentAt(edit: EditDoc, ms: number): number {
  return edit.segments.findIndex((s, i) => ms >= s.start_ms && (ms < s.end_ms || (i === edit.segments.length - 1 && ms <= s.end_ms)));
}

/** Cuts the segment under `ms` in two. Does nothing too close to an existing cut. */
export function splitAt(edit: EditDoc, ms: number): EditDoc {
  const index = segmentAt(edit, ms);
  if (index < 0) return edit;
  const s = edit.segments[index];
  if (ms - s.start_ms < MIN_SEGMENT_MS || s.end_ms - ms < MIN_SEGMENT_MS) return edit;
  const next = [...edit.segments];
  next.splice(index, 1, { ...s, end_ms: ms }, { ...s, start_ms: ms });
  return withSegments(edit, next);
}

export function setSegmentEnabled(edit: EditDoc, index: number, enabled: boolean): EditDoc {
  if (!edit.segments[index]) return edit;
  return withSegments(edit, edit.segments.map((s, i) => (i === index ? { ...s, enabled } : s)));
}

/** Removes the cut between segment `index` and the one after it. */
export function joinAfter(edit: EditDoc, index: number): EditDoc {
  const a = edit.segments[index];
  const b = edit.segments[index + 1];
  if (!a || !b) return edit;
  const next = [...edit.segments];
  next.splice(index, 2, { start_ms: a.start_ms, end_ms: b.end_ms, enabled: a.enabled || b.enabled });
  return withSegments(edit, next);
}

/** Moves the trim start. Moving it outwards brings trimmed material back. */
export function setIn(edit: EditDoc, ms: number, durationMs: number): EditDoc {
  const target = Math.max(0, Math.min(ms, trimEnd(edit) - MIN_SEGMENT_MS, durationMs));
  const rest = edit.segments.filter((s) => s.end_ms - target >= MIN_SEGMENT_MS);
  if (rest.length === 0) return edit;
  rest[0] = { ...rest[0], start_ms: target };
  return withSegments(edit, rest);
}

export function setOut(edit: EditDoc, ms: number, durationMs: number): EditDoc {
  const target = Math.min(durationMs, Math.max(ms, trimStart(edit) + MIN_SEGMENT_MS));
  const rest = edit.segments.filter((s) => target - s.start_ms >= MIN_SEGMENT_MS);
  if (rest.length === 0) return edit;
  rest[rest.length - 1] = { ...rest[rest.length - 1], end_ms: target };
  return withSegments(edit, rest);
}

export interface Range {
  start_ms: number;
  end_ms: number;
}

/** The parts that end up in the result, neighbours merged. */
export function keptRanges(edit: EditDoc): Range[] {
  const ranges: Range[] = [];
  for (const s of edit.segments) {
    if (!s.enabled) continue;
    const last = ranges[ranges.length - 1];
    if (last && last.end_ms === s.start_ms) last.end_ms = s.end_ms;
    else ranges.push({ start_ms: s.start_ms, end_ms: s.end_ms });
  }
  return ranges;
}

export function keptDuration(edit: EditDoc): number {
  return keptRanges(edit).reduce((sum, r) => sum + r.end_ms - r.start_ms, 0);
}

/** Position in the result for a position in the source, null inside a removed part. */
export function sourceToOutput(ranges: Range[], ms: number): number | null {
  let before = 0;
  for (const r of ranges) {
    if (ms >= r.start_ms && ms <= r.end_ms) return before + ms - r.start_ms;
    before += r.end_ms - r.start_ms;
  }
  return null;
}

/** Where playback continues from `ms`: the same spot, the next kept part, or null at the end. */
export function nextPlayable(ranges: Range[], ms: number): number | null {
  for (const r of ranges) {
    if (ms < r.start_ms) return r.start_ms;
    if (ms < r.end_ms) return ms;
  }
  return null;
}

/** Volume each track is really heard at: solo wins over everything, then mute. */
export function effectiveGain(tracks: TrackEdit[]): Map<number, number> {
  const anySolo = tracks.some((t) => t.solo);
  return new Map(tracks.map((t) => [t.stream, (anySolo ? t.solo : !t.muted) ? t.volume : 0]));
}
