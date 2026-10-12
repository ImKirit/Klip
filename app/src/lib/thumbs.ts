// Thumbnails are made by ffmpeg on demand and kept in the cache. At most three are in
// the making at once, so scrolling through a big library does not start fifty ffmpegs.

import { call } from "./api";
import type { Clip } from "./library";

const known = new Map<string, string>();
const pending = new Map<string, Promise<string>>();
const waiting: (() => void)[] = [];
let running = 0;
const LIMIT = 3;

function slot(): Promise<void> {
  if (running < LIMIT) {
    running++;
    return Promise.resolve();
  }
  return new Promise((resolve) => waiting.push(resolve));
}

function release(): void {
  const next = waiting.shift();
  if (next) next();
  else running--;
}

function key(clip: Clip): string {
  return `${clip.path}|${clip.item.size}|${clip.item.modified_ms}`;
}

export function knownThumb(clip: Clip): string | null {
  return clip.item.thumb ?? known.get(key(clip)) ?? null;
}

export function requestThumb(clip: Clip): Promise<string> {
  const id = key(clip);
  const ready = knownThumb(clip);
  if (ready) return Promise.resolve(ready);
  let job = pending.get(id);
  if (!job) {
    job = (async () => {
      await slot();
      try {
        // A second or a tenth into the clip, the very first frame is often a fade or a menu.
        const at_ms = clip.item.media === "image" ? 0 : Math.round(Math.min(1000, clip.durationMs / 10));
        const result = await call<{ path: string }>("thumb", { path: clip.path, at_ms });
        known.set(id, result.path);
        return result.path;
      } finally {
        release();
        pending.delete(id);
      }
    })();
    pending.set(id, job);
  }
  return job;
}
