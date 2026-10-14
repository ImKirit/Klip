// The input overlay: reads `<name>.inputs.json`, knows which keys a layout shows and
// draws them. The same drawing code feeds the live preview in the editor and the
// pictures ffmpeg lays over the video on export, so both look alike.
//
// Key geometry comes straight from crates/klip-input/layouts, the format of the track
// is described in Klip-Vault/Recorder/Eingabe-Erfassung.md.

import qwertz from "../../../crates/klip-input/layouts/qwertz.json";
import qwerty from "../../../crates/klip-input/layouts/qwerty.json";
import type { Range } from "./edit";
import { getLang, t } from "./i18n";
import type { Key } from "./i18n";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface LayoutKey {
  id: number;
  code: string;
  label: string;
  block: string;
  rect: Rect;
  extra?: Rect;
}

interface KeyboardLayout {
  width: number;
  height: number;
  keys: LayoutKey[];
}

export type KeyboardKind = "qwertz" | "qwerty";
export type OverlayScope = "wasd" | "left_hand" | "sixty" | "arrows" | "full" | "none";

/** How the overlay sits in the picture. Stored in `<name>.edit.json`. */
export interface OverlayConfig {
  enabled: boolean;
  keyboard: KeyboardKind;
  scope: OverlayScope;
  mouse: boolean;
  /** Left and top edge as a fraction of the picture, 0 to 1. */
  x: number;
  y: number;
  /** Width as a fraction of the picture width. */
  scale: number;
  opacity: number;
}

export const DEFAULT_OVERLAY: OverlayConfig = { enabled: false, keyboard: "qwertz", scope: "left_hand", mouse: true, x: 0.03, y: 0.62, scale: 0.3, opacity: 0.9 };

/** Gap between the overlay and the edge of the picture, as a fraction of its width. */
const CORNER_MARGIN = 0.02;

/**
 * The overlay of a clip that has no settings of its own yet: shown or hidden as the
 * settings say, in the corner chosen there. `aspect` is width by height of the picture.
 */
export function overlayDefault(input: { show_in_clips?: boolean; corner?: string }, aspect: number): OverlayConfig {
  const base = DEFAULT_OVERLAY;
  const geo = geometry(base);
  const heightShare = geo.width > 0 ? (base.scale * geo.height * aspect) / geo.width : 0;
  const corner = input.corner ?? "top_right";
  const marginY = CORNER_MARGIN * aspect;
  return {
    ...base,
    enabled: input.show_in_clips !== false,
    x: corner.endsWith("left") ? CORNER_MARGIN : Math.max(0, 1 - base.scale - CORNER_MARGIN),
    y: corner.startsWith("top") ? marginY : Math.max(0, 1 - heightShare - marginY),
  };
}

export const SCOPES: OverlayScope[] = ["wasd", "left_hand", "sixty", "arrows", "full", "none"];

const SCOPE_KEY: Record<OverlayScope, Key> = {
  wasd: "scope.wasd",
  left_hand: "scope.left_hand",
  sixty: "scope.sixty",
  arrows: "scope.arrows",
  full: "scope.full",
  none: "scope.none",
};

export function scopeLabel(scope: OverlayScope): string {
  return t(SCOPE_KEY[scope]);
}

// Same sets as Preset::Wasd and Preset::LeftHandShooter in klip-input.
const WASD = [0x11, 0x1e, 0x1f, 0x20];
const LEFT_HAND = [
  0x02, 0x03, 0x04, 0x05, 0x06, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x3a, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x2a, 0x56, 0x2c, 0x2d, 0x2e, 0x2f, 0x1d, 0x38, 0x39,
];

const LAYOUTS: Record<KeyboardKind, KeyboardLayout> = { qwertz: qwertz as KeyboardLayout, qwerty: qwerty as KeyboardLayout };

function visibleKeys(kind: KeyboardKind, scope: OverlayScope): LayoutKey[] {
  const keys = LAYOUTS[kind].keys;
  switch (scope) {
    case "wasd":
      return keys.filter((k) => WASD.includes(k.id));
    case "left_hand":
      return keys.filter((k) => LEFT_HAND.includes(k.id));
    case "sixty":
      return keys.filter((k) => k.block === "main");
    case "arrows":
      return keys.filter((k) => k.block === "arrows");
    case "full":
      return keys;
    case "none":
      return [];
  }
}

/** One thing that lights up: a key, a mouse button or a wheel direction. */
export interface Element {
  /** "k17" for a key, "b0" to "b4" for mouse buttons, "wu" and "wd" for the wheel. */
  id: string;
  label: string;
  rects: Rect[];
}

export interface Geometry {
  /** Size in key units. */
  width: number;
  height: number;
  elements: Element[];
  /** Outline of the mouse, drawn behind its buttons. */
  mouseBody: Rect | null;
}

/**
 * Keys whose name is a word, not a character: [English, German]. They follow the
 * language of the interface. Every other label stays as the keyboard layout prints it.
 */
const WORD_KEYS: Record<string, [string, string]> = {
  ArrowDown: ["Down", "Runter"],
  ArrowLeft: ["Left", "Links"],
  ArrowRight: ["Right", "Rechts"],
  Backspace: ["Back", "Rück"],
  CapsLock: ["Caps", "Feststell"],
  ContextMenu: ["Menu", "Menü"],
  ControlLeft: ["Ctrl", "Strg"],
  ControlRight: ["Ctrl", "Strg"],
  Delete: ["Del", "Entf"],
  End: ["End", "Ende"],
  Home: ["Home", "Pos1"],
  Insert: ["Ins", "Einfg"],
  PageDown: ["PgDn", "Bild ab"],
  PageUp: ["PgUp", "Bild auf"],
  PrintScreen: ["PrtSc", "Druck"],
  ScrollLock: ["ScrLk", "Rollen"],
  ShiftLeft: ["Shift", "Umschalt"],
  ShiftRight: ["Shift", "Umschalt"],
  Space: ["Space", "Leer"],
};

export function keyLabel(code: string, label: string, lang = getLang()): string {
  const words = WORD_KEYS[code];
  return words ? words[lang === "de" ? 1 : 0] : label;
}

const MOUSE_W = 2.6;
const MOUSE_H = 3.6;

/** Everything the overlay shows, laid out from 0,0. */
export function geometry(config: OverlayConfig): Geometry {
  const keys = visibleKeys(config.keyboard, config.scope);
  const rects = keys.flatMap((k) => (k.extra ? [k.rect, k.extra] : [k.rect]));
  const x0 = rects.length > 0 ? Math.min(...rects.map((r) => r.x)) : 0;
  const y0 = rects.length > 0 ? Math.min(...rects.map((r) => r.y)) : 0;
  const keyW = rects.length > 0 ? Math.max(...rects.map((r) => r.x + r.w)) - x0 : 0;
  const keyH = rects.length > 0 ? Math.max(...rects.map((r) => r.y + r.h)) - y0 : 0;
  const elements: Element[] = keys.map((k) => ({
    id: `k${k.id}`,
    label: keyLabel(k.code, k.label),
    rects: (k.extra ? [k.rect, k.extra] : [k.rect]).map((r) => ({ x: r.x - x0, y: r.y - y0, w: r.w, h: r.h })),
  }));
  let width = keyW;
  let height = keyH;
  let mouseBody: Rect | null = null;
  if (config.mouse) {
    const mx = keyW > 0 ? keyW + 0.5 : 0;
    const my = Math.max(0, (keyH - MOUSE_H) / 2);
    mouseBody = { x: mx, y: my, w: MOUSE_W, h: MOUSE_H };
    const half = (MOUSE_W - 0.6) / 2;
    elements.push(
      { id: "b0", label: "L", rects: [{ x: mx, y: my, w: half, h: 1.5 }] },
      { id: "b1", label: "R", rects: [{ x: mx + half + 0.6, y: my, w: half, h: 1.5 }] },
      { id: "wu", label: "", rects: [{ x: mx + half, y: my, w: 0.6, h: 0.5 }] },
      { id: "b2", label: "", rects: [{ x: mx + half, y: my + 0.5, w: 0.6, h: 0.5 }] },
      { id: "wd", label: "", rects: [{ x: mx + half, y: my + 1.0, w: 0.6, h: 0.5 }] },
      { id: "b4", label: "", rects: [{ x: mx, y: my + 1.7, w: 0.45, h: 0.7 }] },
      { id: "b3", label: "", rects: [{ x: mx, y: my + 2.4, w: 0.45, h: 0.7 }] },
    );
    width = mx + MOUSE_W;
    height = Math.max(keyH, MOUSE_H);
  }
  return { width, height, elements, mouseBody };
}

type RawEvent = [number, string, number, number];

export interface InputTrack {
  version: number;
  duration_ms: number;
  move_window_ms: number;
  held: { keys: number[]; buttons: string[] };
  events: RawEvent[];
}

const BUTTON_INDEX: Record<string, number> = { left: 0, right: 1, middle: 2, side1: 3, side2: 4 };
/** How long one wheel notch stays lit. */
const WHEEL_FLASH_MS = 130;

/** Checks what was read from disk. Throws with a clear message for a newer format. */
export function parseTrack(raw: unknown): InputTrack {
  const track = raw as Partial<InputTrack> | null;
  if (!track || typeof track !== "object" || !Array.isArray(track.events)) throw new Error(t("in.trackUnreadable"));
  if (typeof track.version !== "number" || track.version > 1) throw new Error(t("in.trackVersion", String(track.version)));
  return {
    version: track.version,
    duration_ms: Number(track.duration_ms) || 0,
    move_window_ms: Number(track.move_window_ms) || 50,
    held: { keys: track.held?.keys ?? [], buttons: track.held?.buttons ?? [] },
    events: track.events.filter((e) => Array.isArray(e) && e.length >= 4),
  };
}

/** For every element the spans in which it is lit, in clip time (ms). */
export function pressIntervals(track: InputTrack): Map<string, Range[]> {
  const spans = new Map<string, Range[]>();
  const open = new Map<string, number>();
  const add = (id: string, start: number, end: number) => {
    if (end <= start) return;
    const list = spans.get(id) ?? [];
    const last = list[list.length - 1];
    if (last && start <= last.end_ms) last.end_ms = Math.max(last.end_ms, end);
    else list.push({ start_ms: start, end_ms: end });
    spans.set(id, list);
  };
  for (const key of track.held.keys) open.set(`k${key}`, 0);
  for (const button of track.held.buttons) if (button in BUTTON_INDEX) open.set(`b${BUTTON_INDEX[button]}`, 0);
  for (const [t, kind, a, b] of track.events) {
    if (kind === "k" || kind === "b") {
      const id = `${kind}${a}`;
      if (b === 1) {
        if (!open.has(id)) open.set(id, t);
      } else if (open.has(id)) {
        add(id, open.get(id)!, t);
        open.delete(id);
      }
    } else if (kind === "w" && b === 0) {
      add(a > 0 ? "wu" : "wd", t, t + WHEEL_FLASH_MS);
    }
  }
  // Still held when the clip ends.
  for (const [id, start] of open) add(id, start, track.duration_ms);
  return spans;
}

export function activeAt(spans: Map<string, Range[]>, ms: number): Set<string> {
  const active = new Set<string>();
  for (const [id, list] of spans) {
    if (list.some((r) => ms >= r.start_ms && ms < r.end_ms)) active.add(id);
  }
  return active;
}

/** Moves spans from clip time to the time of the cut result and drops removed parts. */
export function mapToOutput(spans: Range[], kept: Range[]): Range[] {
  const out: Range[] = [];
  let before = 0;
  for (const part of kept) {
    for (const span of spans) {
      const start = Math.max(span.start_ms, part.start_ms);
      const end = Math.min(span.end_ms, part.end_ms);
      if (end > start) out.push({ start_ms: before + start - part.start_ms, end_ms: before + end - part.start_ms });
    }
    before += part.end_ms - part.start_ms;
  }
  return out;
}

export interface OverlayColors {
  accent: string;
  onAccent: string;
}

const CAP = "#191924";
const CAP_LINE = "#2A2A3C";
const CAP_TEXT = "#EDEDF7";

/** The shape of the design: a rectangle with the top right corner cut off, never rounded. */
function cutRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, cut: number) {
  const c = Math.min(cut, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + w - c, y);
  ctx.lineTo(x + w, y + c);
  ctx.lineTo(x + w, y + h);
  ctx.lineTo(x, y + h);
  ctx.closePath();
}

function capPath(ctx: CanvasRenderingContext2D, r: Rect, unit: number) {
  const gap = unit * 0.05;
  cutRect(ctx, r.x * unit + gap, r.y * unit + gap, r.w * unit - gap * 2, r.h * unit - gap * 2, unit * 0.18);
}

export function drawElement(ctx: CanvasRenderingContext2D, el: Element, unit: number, pressed: boolean, colors: OverlayColors) {
  for (const r of el.rects) {
    capPath(ctx, r, unit);
    ctx.fillStyle = pressed ? colors.accent : CAP;
    ctx.fill();
    if (!pressed) {
      ctx.lineWidth = Math.max(1, unit * 0.03);
      ctx.strokeStyle = CAP_LINE;
      ctx.stroke();
    }
  }
  if (!el.label) return;
  const r = el.rects[0];
  // Long labels such as "Umschalt" shrink until they fit their cap.
  let size = unit * 0.36;
  ctx.font = `600 ${size}px "Barlow", sans-serif`;
  const room = r.w * unit - unit * 0.24;
  const wide = ctx.measureText(el.label).width;
  if (wide > room) {
    size *= room / wide;
    ctx.font = `600 ${size}px "Barlow", sans-serif`;
  }
  ctx.fillStyle = pressed ? colors.onAccent : CAP_TEXT;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(el.label, (r.x + r.w / 2) * unit, (r.y + r.h / 2) * unit + size * 0.04);
}

function drawMouseBody(ctx: CanvasRenderingContext2D, body: Rect, unit: number) {
  cutRect(ctx, body.x * unit + unit * 0.05, body.y * unit + unit * 0.05, body.w * unit - unit * 0.1, body.h * unit - unit * 0.1, unit * 0.5);
  ctx.fillStyle = "#111119";
  ctx.fill();
  ctx.lineWidth = Math.max(1, unit * 0.03);
  ctx.strokeStyle = CAP_LINE;
  ctx.stroke();
}

/** Draws the whole overlay with the given elements lit. `widthPx` is its full width. */
export function drawOverlay(ctx: CanvasRenderingContext2D, geo: Geometry, widthPx: number, active: Set<string>, colors: OverlayColors) {
  const unit = widthPx / geo.width;
  if (geo.mouseBody) drawMouseBody(ctx, geo.mouseBody, unit);
  for (const el of geo.elements) drawElement(ctx, el, unit, active.has(el.id), colors);
}

export interface OverlayPicture {
  /** PNG, base64 without the data URL prefix. */
  png: string;
  /** Position inside the video frame in pixels. */
  x: number;
  y: number;
  /** Lit spans in seconds of the result, null for the picture that is always shown. */
  spans: Range[] | null;
  name: string;
}

function toPng(canvas: HTMLCanvasElement): string {
  return canvas.toDataURL("image/png").split(",")[1];
}

/**
 * The pictures ffmpeg needs: one of the whole overlay at rest, and one small picture per
 * element that is ever lit, cut to that element. Sizes are for a frame of the given size.
 */
export async function renderForExport(
  config: OverlayConfig,
  track: InputTrack,
  kept: Range[],
  frameW: number,
  frameH: number,
  colors: OverlayColors,
): Promise<OverlayPicture[]> {
  await document.fonts.load('600 20px "Barlow"');
  const geo = geometry(config);
  if (geo.elements.length === 0) return [];
  const widthPx = Math.max(16, Math.round(frameW * config.scale));
  const unit = widthPx / geo.width;
  const heightPx = Math.ceil(geo.height * unit);
  const left = Math.round(config.x * frameW);
  const top = Math.round(config.y * frameH);

  const base = document.createElement("canvas");
  base.width = widthPx;
  base.height = heightPx;
  const baseCtx = base.getContext("2d")!;
  baseCtx.globalAlpha = config.opacity;
  drawOverlay(baseCtx, geo, widthPx, new Set(), colors);
  const pictures: OverlayPicture[] = [{ png: toPng(base), x: left, y: top, spans: null, name: "overlay_base.png" }];

  const spans = pressIntervals(track);
  for (const el of geo.elements) {
    const lit = mapToOutput(spans.get(el.id) ?? [], kept);
    if (lit.length === 0) continue;
    const x0 = Math.floor(Math.min(...el.rects.map((r) => r.x)) * unit);
    const y0 = Math.floor(Math.min(...el.rects.map((r) => r.y)) * unit);
    const x1 = Math.ceil(Math.max(...el.rects.map((r) => r.x + r.w)) * unit);
    const y1 = Math.ceil(Math.max(...el.rects.map((r) => r.y + r.h)) * unit);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, x1 - x0);
    canvas.height = Math.max(1, y1 - y0);
    const ctx = canvas.getContext("2d")!;
    ctx.globalAlpha = config.opacity;
    ctx.translate(-x0, -y0);
    drawElement(ctx, el, unit, true, colors);
    pictures.push({ png: toPng(canvas), x: left + x0, y: top + y0, spans: lit, name: `overlay_${el.id}.png` });
  }
  return pictures;
}
