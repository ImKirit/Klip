// The timeline of the editor: ruler, events, the cut list with trim handles, one
// waveform per audio track and the input track.

import { memo, useEffect, useMemo, useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type { AudioTrack } from "../lib/audio";
import { effectiveGain, segmentAt, trimEnd, trimStart } from "../lib/edit";
import type { Range } from "../lib/edit";
import { formatDuration } from "../lib/format";
import { t } from "../lib/i18n";
import { eventLabel } from "../lib/library";
import type { ClipEvent, EditDoc, TrackEdit } from "../lib/types";

interface Props {
  durationMs: number;
  fps: number;
  timeMs: number;
  zoom: number;
  edit: EditDoc;
  events: ClipEvent[];
  tracks: AudioTrack[];
  /** Spans in which any key or button is held, null when the clip has no input track. */
  presses: Range[] | null;
  onSeek: (ms: number) => void;
  onTrim: (side: "in" | "out", ms: number) => void;
  /** Called once before a drag changes the edit, for undo. */
  onTrimStart: () => void;
  onTrack: (stream: number, patch: Partial<TrackEdit>) => void;
}

const Waveform = memo(function Waveform({ peaks }: { peaks: number[] }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const draw = () => {
      const width = Math.max(1, Math.round(canvas.clientWidth * devicePixelRatio));
      const height = Math.max(1, Math.round(canvas.clientHeight * devicePixelRatio));
      canvas.width = width;
      canvas.height = height;
      const ctx = canvas.getContext("2d");
      if (!ctx || peaks.length === 0) return;
      ctx.fillStyle = getComputedStyle(canvas).color;
      const mid = height / 2;
      for (let x = 0; x < width; x++) {
        // Loudest value of all points that fall on this pixel column.
        const from = Math.floor((x / width) * peaks.length);
        const to = Math.max(from + 1, Math.floor(((x + 1) / width) * peaks.length));
        let peak = 0;
        for (let i = from; i < to && i < peaks.length; i++) peak = Math.max(peak, peaks[i]);
        // Square root: quiet passages stay visible next to loud ones.
        const half = Math.max(0.5, Math.sqrt(peak) * (mid - 3));
        ctx.fillRect(x, mid - half, 1, half * 2);
      }
    };
    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [peaks]);
  return <canvas ref={ref} className="tl-wave" />;
});

function rulerStep(durationMs: number, zoom: number): number {
  const steps = [500, 1000, 2000, 5000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000];
  const wanted = durationMs / (12 * zoom);
  return steps.find((s) => s >= wanted) ?? steps[steps.length - 1];
}

export function Timeline(props: Props) {
  const { durationMs, edit, timeMs, zoom } = props;
  const lanes = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const pct = (ms: number) => `${(ms / durationMs) * 100}%`;
  const gains = useMemo(() => effectiveGain(edit.tracks), [edit.tracks]);

  const msAt = (clientX: number): number => {
    const box = inner.current!.getBoundingClientRect();
    return Math.max(0, Math.min(durationMs, ((clientX - box.left) / box.width) * durationMs));
  };

  // Scrub: press anywhere on the lanes and drag.
  const scrub = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    props.onSeek(msAt(e.clientX));
  };
  const scrubMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.buttons === 1 && (e.currentTarget as HTMLElement).hasPointerCapture(e.pointerId)) props.onSeek(msAt(e.clientX));
  };

  const dragHandle = (side: "in" | "out") => ({
    onPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => {
      e.stopPropagation();
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      props.onTrimStart();
    },
    onPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => {
      if (e.buttons !== 1 || !(e.currentTarget as HTMLElement).hasPointerCapture(e.pointerId)) return;
      // Snap to whole frames.
      const frame = 1000 / (props.fps || 60);
      props.onTrim(side, Math.round(msAt(e.clientX) / frame) * frame);
    },
  });

  // Keep the playhead in view while zoomed in.
  useEffect(() => {
    const view = lanes.current;
    const content = inner.current;
    if (!view || !content || zoom <= 1) return;
    const x = (timeMs / durationMs) * content.clientWidth;
    if (x < view.scrollLeft + 20 || x > view.scrollLeft + view.clientWidth - 20) view.scrollLeft = Math.max(0, x - view.clientWidth / 2);
  }, [timeMs, durationMs, zoom]);

  const step = rulerStep(durationMs, zoom);
  const ticks = useMemo(() => {
    const list: number[] = [];
    for (let t = 0; t < durationMs; t += step) list.push(t);
    return list;
  }, [durationMs, step]);
  const current = segmentAt(edit, timeMs);
  const start = trimStart(edit);
  const end = trimEnd(edit);

  return (
    <div className="timeline">
      <div className="tl-heads">
        <div className="tl-head is-ruler" />
        {props.events.length > 0 ? <div className="tl-head is-thin">{t("tl.events")}</div> : null}
        <div className="tl-head">
          <span className="name">{t("tl.picture")}</span>
        </div>
        {props.tracks.map((track) => {
          const state = edit.tracks.find((t) => t.stream === track.stream);
          if (!state) return null;
          return (
            <div key={track.stream} className="tl-head">
              <span className="name" title={track.name}>
                {track.name}
              </span>
              <button type="button" className={`ms-btn${state.muted ? " is-mute" : ""}`} title={t("tl.mute")} aria-pressed={state.muted} onClick={() => props.onTrack(track.stream, { muted: !state.muted })}>
                M
              </button>
              <button type="button" className={`ms-btn${state.solo ? " is-solo" : ""}`} title={t("tl.solo")} aria-pressed={state.solo} onClick={() => props.onTrack(track.stream, { solo: !state.solo })}>
                S
              </button>
              <input
                type="range"
                min={0}
                max={2}
                step={0.05}
                value={state.volume}
                title={t("tl.volume", Math.round(state.volume * 100))}
                aria-label={t("tl.volumeOf", track.name)}
                onChange={(e) => props.onTrack(track.stream, { volume: Number(e.target.value) })}
                onDoubleClick={() => props.onTrack(track.stream, { volume: 1 })}
              />
            </div>
          );
        })}
        {props.presses ? <div className="tl-head is-thin">{t("tl.inputs")}</div> : null}
      </div>

      <div className="tl-lanes" ref={lanes}>
        <div className="tl-lanes-inner" ref={inner} style={{ width: `${zoom * 100}%` }} onPointerDown={scrub} onPointerMove={scrubMove}>
          <div className="tl-lane is-ruler">
            {ticks.map((t) => (
              <span key={t} className="tick" style={{ left: pct(t) }}>
                {formatDuration(t)}
              </span>
            ))}
          </div>

          {props.events.length > 0 ? (
            <div className="tl-lane is-thin">
              {props.events.map((event, i) => (
                <button
                  key={i}
                  type="button"
                  className={`tl-event${event.kind === "marker" ? " is-marker" : ""}`}
                  style={{ left: pct(event.at_ms) }}
                  title={t("tl.eventAt", event.label ?? eventLabel(event.kind), formatDuration(event.at_ms))}
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={() => props.onSeek(event.at_ms)}
                />
              ))}
            </div>
          ) : null}

          <div className="tl-lane">
            {edit.segments.map((s, i) => (
              <div
                key={`${s.start_ms}-${s.end_ms}`}
                className={`segment${s.enabled ? "" : " is-off"}${i === current ? " is-current" : ""}`}
                style={{ left: pct(s.start_ms), width: pct(s.end_ms - s.start_ms) }}
                title={s.enabled ? t("tl.section") : t("tl.sectionOff")}
              >
                <span className="segment-label">{s.enabled ? formatDuration(s.end_ms - s.start_ms) : t("tl.removed")}</span>
              </div>
            ))}
            <div className="trim-handle is-in" style={{ left: pct(start) }} title={t("tl.in")} {...dragHandle("in")} />
            <div className="trim-handle is-out" style={{ left: pct(end) }} title={t("tl.out")} {...dragHandle("out")} />
          </div>

          {props.tracks.map((track) => (
            <div key={track.stream} className={`tl-lane${(gains.get(track.stream) ?? 1) === 0 ? " is-silent" : ""}`}>
              <Waveform peaks={track.peaks} />
            </div>
          ))}

          {props.presses ? (
            <div className="tl-lane is-thin">
              {props.presses.map((p, i) => (
                <span key={i} className="tl-press" style={{ left: pct(p.start_ms), width: pct(p.end_ms - p.start_ms) }} />
              ))}
            </div>
          ) : null}

          {/* Everything outside the trim and every removed part is dimmed over all lanes. */}
          {start > 0 ? <div className="tl-shade" style={{ left: 0, width: pct(start) }} /> : null}
          {end < durationMs ? <div className="tl-shade" style={{ left: pct(end), right: 0 }} /> : null}
          {edit.segments.map((s) => (s.enabled ? null : <div key={`off-${s.start_ms}`} className="tl-shade" style={{ left: pct(s.start_ms), width: pct(s.end_ms - s.start_ms) }} />))}

          <div className="playhead" style={{ left: pct(timeMs) }} />
        </div>
      </div>
    </div>
  );
}
