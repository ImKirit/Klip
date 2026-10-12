import { describe, expect, it } from "vitest";
import { defaultEdit, effectiveGain, joinAfter, keptDuration, keptRanges, nextPlayable, normalizeEdit, setIn, setOut, setSegmentEnabled, sourceToOutput, splitAt } from "./edit";
import { buildPlan, copyBlocker, outputSize, planArgs } from "./export";
import type { ExportOptions, Source } from "./export";
import { actionLabel, checkHotkeys, comboId, parseCombo, withAllActions } from "./hotkeys";
import { geometry, mapToOutput, overlayDefault, parseTrack, pressIntervals, DEFAULT_OVERLAY } from "./inputs";
import { allEntries } from "./i18n";
import { matches, NO_FILTERS, planCleanup, toClip } from "./library";
import type { LibraryItem } from "./types";

const source: Source = {
  path: "C:\\clips\\a.mp4",
  durationMs: 30_000,
  width: 1920,
  height: 1080,
  fps: 60,
  videoCodec: "h264",
  audio: [
    { stream: 0, codec: "aac", name: "Mix" },
    { stream: 1, codec: "aac", name: "Spiel" },
    { stream: 2, codec: "aac", name: "Mikrofon" },
  ],
};

const base: ExportOptions = {
  format: "mp4",
  height: null,
  fps: null,
  quality: "high",
  codec: "h264",
  audioMode: "mix",
  tracks: [
    { stream: 1, name: "Spiel", volume: 1 },
    { stream: 2, name: "Mikrofon", volume: 0.5 },
  ],
  targetMb: null,
  allowCopy: true,
};

describe("edit", () => {
  it("splits, removes and joins", () => {
    let edit = defaultEdit(10_000, [1, 2]);
    edit = splitAt(edit, 4000);
    edit = splitAt(edit, 6000);
    expect(edit.segments).toHaveLength(3);
    edit = setSegmentEnabled(edit, 1, false);
    expect(keptRanges(edit)).toEqual([
      { start_ms: 0, end_ms: 4000 },
      { start_ms: 6000, end_ms: 10_000 },
    ]);
    expect(keptDuration(edit)).toBe(8000);
    expect(sourceToOutput(keptRanges(edit), 7000)).toBe(5000);
    expect(sourceToOutput(keptRanges(edit), 5000)).toBeNull();
    expect(nextPlayable(keptRanges(edit), 4500)).toBe(6000);
    expect(nextPlayable(keptRanges(edit), 10_000)).toBeNull();
    expect(joinAfter(edit, 0).segments).toHaveLength(2);
  });

  it("trims and brings material back", () => {
    let edit = defaultEdit(10_000, []);
    edit = setIn(edit, 2000, 10_000);
    edit = setOut(edit, 9000, 10_000);
    expect(keptRanges(edit)).toEqual([{ start_ms: 2000, end_ms: 9000 }]);
    edit = setIn(edit, 500, 10_000);
    expect(edit.segments[0].start_ms).toBe(500);
  });

  it("repairs a broken edit file", () => {
    const edit = normalizeEdit({ segments: [{ start_ms: 5000, end_ms: 99_999, enabled: true }, { start_ms: -5, end_ms: 5000 }], tracks: [{ stream: 2, muted: true, volume: 9 }] }, 10_000, [1, 2]);
    expect(edit.segments).toEqual([
      { start_ms: 0, end_ms: 5000, enabled: true },
      { start_ms: 5000, end_ms: 10_000, enabled: true },
    ]);
    expect(edit.tracks[1]).toEqual({ stream: 2, muted: true, solo: false, volume: 2 });
  });

  it("solo wins over mute", () => {
    const gains = effectiveGain([
      { stream: 1, muted: false, solo: true, volume: 0.8 },
      { stream: 2, muted: false, solo: false, volume: 1 },
    ]);
    expect(gains.get(1)).toBe(0.8);
    expect(gains.get(2)).toBe(0);
  });
});

describe("export", () => {
  const whole = [{ start_ms: 0, end_ms: 30_000 }];
  const trimmed = [{ start_ms: 5000, end_ms: 15_000 }];

  it("copies when only trimmed and the format fits", () => {
    const plan = buildPlan(source, trimmed, { ...base, audioMode: "separate", tracks: [{ stream: 1, name: "Spiel", volume: 1 }] }, null, "C:\\out\\a.mp4");
    expect(plan.video).toBe("copy");
    expect(plan.audio).toBe("copy");
    expect(plan.filter).toBe("");
    expect(planArgs(plan, null).join(" ")).toContain("-ss 5.000 -t 10.000 -i C:\\clips\\a.mp4 -map 0:v:0 -c:v copy");
  });

  it("copies the picture and still mixes the sound", () => {
    const plan = buildPlan(source, trimmed, base, null, "C:\\out\\a.mp4");
    expect(plan.video).toBe("copy");
    expect(plan.audio).toBe("encode");
    expect(plan.filter).toContain("amix=inputs=2");
    expect(plan.filter).toContain("volume=0.500");
  });

  it("says why it cannot copy", () => {
    expect(copyBlocker(source, trimmed, { ...base, format: "webm" })).toContain("WEBM");
    expect(copyBlocker(source, [...trimmed, { start_ms: 20_000, end_ms: 25_000 }], base)).toContain("sections");
    expect(copyBlocker(source, trimmed, { ...base, targetMb: 10 })).toContain("target size");
    expect(copyBlocker(source, trimmed, base, true)).toContain("overlay");
    expect(copyBlocker(source, trimmed, base)).toBeNull();
  });

  it("joins several parts with concat", () => {
    const plan = buildPlan(source, [...trimmed, { start_ms: 20_000, end_ms: 25_000 }], base, "libx264", "C:\\out\\a.mp4");
    expect(plan.video).toBe("encode");
    expect(plan.totalMs).toBe(15_000);
    expect(plan.filter).toContain("[0:v:0][1:v:0]concat=n=2:v=1:a=0[vc]");
    expect(plan.filter).toContain("[0:a:1][1:a:1]concat=n=2:v=0:a=1[ac0]");
  });

  it("computes the bitrate for a target size", () => {
    const plan = buildPlan(source, whole, { ...base, targetMb: 10 }, "libx264", "C:\\out\\a.mp4");
    // 10 MiB over 30 s is about 2796 kbit/s, minus the reserve and 96 kbit/s of sound.
    expect(plan.videoKbps).toBe(Math.floor((10 * 8388.608 * 0.94) / 30 - 96));
    expect(plan.outputArgs).toContain(`${plan.videoKbps}k`);
  });

  it("writes one file per track for separate audio", () => {
    const plan = buildPlan(source, whole, { ...base, format: "mp3", audioMode: "separate" }, null, "C:\\out\\a.mp3");
    expect(plan.outputs).toEqual(["C:\\out\\a_Spiel.mp3", "C:\\out\\a_Mikrofon.mp3"]);
  });

  it("lays overlay pictures over the scaled picture", () => {
    const options = { ...base, height: 720 };
    expect(outputSize(source, options)).toEqual({ width: 1280, height: 720 });
    const plan = buildPlan(source, whole, options, "libx264", "C:\\out\\a.mp4", [
      { path: "base.png", x: 10, y: 20, spans: null },
      { path: "k17.png", x: 30, y: 40, spans: [{ start_ms: 1000, end_ms: 1500 }] },
    ]);
    expect(plan.video).toBe("encode");
    expect(plan.filter).toContain("[0:v:0]scale=-2:720[ov0]");
    expect(plan.filter).toContain("[ov0][1:v]overlay=x=10:y=20[ov1]");
    expect(plan.filter).toContain("[ov1][2:v]overlay=x=30:y=40:enable='between(t,1.000,1.500)'[ov2]");
    expect(planArgs(plan, "f.txt")).toContain("-/filter_complex");
  });
});

describe("hotkeys", () => {
  it("treats spellings as the same combination", () => {
    expect(comboId("Strg+Umschalt+F10")).toBe(comboId("shift+ctrl+f10"));
    expect("error" in parseCombo("Alt+Ä")).toBe(true);
  });

  it("finds conflicts", () => {
    const problems = checkHotkeys([
      { keys: "Alt+F10", action: { type: "clip", seconds: 60 } },
      { keys: "alt+f10", action: { type: "marker" } },
      { keys: "Ctrl+Shift+F9", action: { type: "record_toggle" } },
    ]);
    expect(problems[0][0].level).toBe("error");
    expect(problems[1][0].level).toBe("error");
    expect(problems[2]).toHaveLength(0);
  });

  it("offers actions an older config lacks", () => {
    const old = [
      { keys: "Alt+F10", action: { type: "clip", seconds: 60 } as const },
      { keys: "Ctrl+Shift+F9", action: { type: "record_toggle" } as const },
    ];
    const all = withAllActions(old);
    expect(all.slice(0, 2)).toEqual(old);
    expect(all.slice(2).map((b) => b.action.type)).toEqual(["record_pause", "marker", "screenshot", "overlay", "input_pause"]);
    expect(all.slice(2).every((b) => b.keys === "")).toBe(true);
    expect(withAllActions(all)).toBe(all);
    expect(actionLabel({ type: "overlay" })).toContain("overlay");
    expect(actionLabel({ type: "input_pause" })).toContain("input");
  });
});

describe("inputs", () => {
  const track = parseTrack({
    version: 1,
    duration_ms: 4000,
    move_window_ms: 50,
    held: { keys: [17], buttons: [] },
    events: [[1000, "k", 30, 1], [1100, "k", 30, 0], [2000, "b", 0, 1], [2050, "b", 0, 0], [3000, "k", 17, 0], [3500, "w", -120, 0], [3500, "m", 7, 312]],
  });

  it("turns events into lit spans", () => {
    const spans = pressIntervals(track);
    expect(spans.get("k17")).toEqual([{ start_ms: 0, end_ms: 3000 }]);
    expect(spans.get("k30")).toEqual([{ start_ms: 1000, end_ms: 1100 }]);
    expect(spans.get("b0")).toEqual([{ start_ms: 2000, end_ms: 2050 }]);
    expect(spans.get("wd")?.[0].start_ms).toBe(3500);
  });

  it("moves spans onto the cut timeline", () => {
    const kept = [{ start_ms: 500, end_ms: 1050 }, { start_ms: 2000, end_ms: 4000 }];
    expect(mapToOutput([{ start_ms: 1000, end_ms: 1100 }], kept)).toEqual([{ start_ms: 500, end_ms: 550 }]);
    expect(mapToOutput([{ start_ms: 0, end_ms: 3000 }], kept)).toEqual([{ start_ms: 0, end_ms: 550 }, { start_ms: 550, end_ms: 1550 }]);
  });

  it("refuses a newer format", () => {
    expect(() => parseTrack({ version: 2, events: [] })).toThrow();
  });

  it("has the 26 keys of the left hand preset on ISO", () => {
    const geo = geometry({ ...DEFAULT_OVERLAY, mouse: false, scope: "left_hand" });
    expect(geo.elements).toHaveLength(26);
    expect(geometry({ ...DEFAULT_OVERLAY, mouse: false, scope: "wasd" }).elements).toHaveLength(4);
  });
});

describe("library", () => {
  const item = (name: string, daysAgo: number, size: number, favorite: boolean): LibraryItem => ({
    path: `C:\\clips\\${name}.mp4`, name, ext: "mp4", folder: "Valorant", media: "video", size, modified_ms: 0, meta_error: null, has_edit: false, has_inputs: false, thumb: null,
    meta: {
      version: 1, kind: "auto_clip", created: new Date(Date.now() - daysAgo * 86_400_000).toISOString(), game: "Valorant", duration_ms: 20_000, width: 1920, height: 1080, fps: 60,
      codec: "h264", encoder: "x", audio_tracks: [], events: [{ at_ms: 1, kind: "ace" }], favorite, tags: ["ranked"],
    },
  });
  const GB = 1024 ** 3;
  const clips = [item("old", 50, GB, false), item("oldfav", 60, GB, true), item("mid", 10, GB, false), item("new", 1, GB, false)].map(toClip);

  it("filters by search, event and length", () => {
    expect(matches(clips[0], { ...NO_FILTERS, search: "valo ace" })).toBe(true);
    expect(matches(clips[0], { ...NO_FILTERS, search: "minecraft" })).toBe(false);
    expect(matches(clips[0], { ...NO_FILTERS, event: "ace", tag: "ranked", length: "short" })).toBe(true);
    expect(matches(clips[0], { ...NO_FILTERS, date: "week" })).toBe(false);
    expect(matches(clips[1], { ...NO_FILTERS, view: "favorites" })).toBe(true);
  });

  it("cleans up by age and size and never touches favorites", () => {
    const byAge = planCleanup(clips, { enabled: true, max_age_days: 30, max_size_gb: 0 });
    expect(byAge.remove.map((c) => c.title)).toEqual(["old"]);
    const bySize = planCleanup(clips, { enabled: true, max_age_days: 0, max_size_gb: 2.5 });
    expect(bySize.remove.map((c) => c.title)).toEqual(["old", "mid"]);
    expect(bySize.reason.get(clips[2].path)).toBe("size");
  });
});

describe("overlay defaults", () => {
  it("follows the settings: shown or not, and the corner", () => {
    const aspect = 16 / 9;
    const topRight = overlayDefault({ show_in_clips: true, corner: "top_right" }, aspect);
    expect(topRight.enabled).toBe(true);
    expect(topRight.x).toBeCloseTo(1 - topRight.scale - 0.02);
    expect(topRight.y).toBeCloseTo(0.02 * aspect);
    const bottomLeft = overlayDefault({ show_in_clips: false, corner: "bottom_left" }, aspect);
    expect(bottomLeft.enabled).toBe(false);
    expect(bottomLeft.x).toBeCloseTo(0.02);
    expect(bottomLeft.y).toBeGreaterThan(0.5);
    expect(bottomLeft.y).toBeLessThan(1);
    // An older config without the fields behaves like the defaults of klip-core.
    expect(overlayDefault({}, aspect)).toEqual(topRight);
  });
});

describe("translations", () => {
  it("has both languages for every entry, with the same placeholders and no dashes", () => {
    for (const [key, [en, de]] of allEntries()) {
      expect(en, key).not.toBe("");
      expect(de, key).not.toBe("");
      const holes = (text: string) => [...text.matchAll(/\{\d+\}/g)].map((m) => m[0]).sort().join();
      expect(holes(de), key).toBe(holes(en));
      expect(/[\u2013\u2014]/.test(en + de), key).toBe(false);
    }
  });
});
