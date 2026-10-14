// Player and editor. The video is never changed: cuts, track settings and the overlay
// position go to `<name>.edit.json` next to it.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react";
import { ArrowLeft, ChevronLeft, ChevronRight, CopyPlus, Pause, Play, RotateCcw, Scissors, Share, SkipBack, SkipForward, Trash2, Undo2 } from "lucide-react";
import { call, errorText, fileUrl, runJob } from "../lib/api";
import { editableStreams, prepareTracks, TrackMixer } from "../lib/audio";
import type { AudioTrack } from "../lib/audio";
import {
  defaultEdit, isUntouched, joinAfter, keptDuration, keptRanges, nextPlayable, normalizeEdit, segmentAt, setIn, setOut, setSegmentEnabled, splitAt,
  trimEnd, trimStart,
} from "../lib/edit";
import type { Range } from "../lib/edit";
import { pickEncoder } from "../lib/export";
import type { Source } from "../lib/export";
import { formatBytes, formatDateTime, formatDuration, formatNumber, timecode } from "../lib/format";
import { t } from "../lib/i18n";
import { activeAt, DEFAULT_OVERLAY, drawElement, drawOverlay, geometry, overlayDefault, parseTrack, pressIntervals, scopeLabel, SCOPES } from "../lib/inputs";
import type { InputTrack, KeyboardKind, OverlayConfig, OverlayScope } from "../lib/inputs";
import { eventLabel, kindLabel } from "../lib/library";
import { cleanAccent, onAccent } from "../lib/settings";
import type { CameraEdit, Corner, EditDoc, JobStatus, ProbeResult, TrackEdit } from "../lib/types";
import { useApp } from "../state";
import { Button, ProgressBar, Select, Toggle, useToast } from "../ui";
import { ExportDialog } from "./ExportDialog";
import { Shell } from "./Shell";
import { Timeline } from "./Timeline";

const TEMPOS = [0.25, 0.5, 1, 1.5, 2, 4];

function frameRate(text: string | undefined): number {
  if (!text) return 0;
  const [a, b] = text.split("/").map(Number);
  return b ? Math.round(a / b) : Math.round(a || 0);
}

interface Loaded {
  source: Source;
  probe: ProbeResult;
  inputs: InputTrack | null;
  inputsError: string | null;
}

/** The webcam of the clip in its corner, kept in step with the main video. */
function CameraLayer(props: { src: string; offsetMs: number; timeMs: number; playing: boolean; tempo: number; edit: CameraEdit; frameW: number }) {
  const ref = useRef<HTMLVideoElement>(null);
  const { offsetMs, timeMs, playing, tempo } = props;
  useEffect(() => {
    const cam = ref.current;
    if (!cam) return;
    const target = (timeMs - offsetMs) / 1000;
    if (target < 0) {
      // The camera starts a little after this point of the clip.
      cam.pause();
      cam.currentTime = 0;
      return;
    }
    cam.playbackRate = tempo;
    // Small drift is fine, a jump only when it shows.
    if (Math.abs(cam.currentTime - target) > (playing ? 0.25 : 0.02)) cam.currentTime = target;
    if (playing && cam.paused) void cam.play().catch(() => {});
    if (!playing && !cam.paused) cam.pause();
  }, [timeMs, offsetMs, playing, tempo]);
  const width = Math.round((props.frameW * props.edit.size_percent) / 100);
  const margin = Math.round(props.frameW * 0.02);
  const corner = props.edit.corner;
  const style: CSSProperties = { width, position: "absolute", pointerEvents: "none" };
  style[corner.endsWith("left") ? "left" : "right"] = margin;
  style[corner.startsWith("top") ? "top" : "bottom"] = margin;
  return <video ref={ref} className="camera-layer" src={props.src} muted playsInline preload="auto" style={style} />;
}

/** The overlay canvas on top of the video. Drag moves it. */
function OverlayLayer(props: { config: OverlayConfig; spans: Map<string, Range[]>; timeMs: number; frameW: number; accent: string; onMove: (x: number, y: number) => void; onMoveStart: () => void }) {
  const { config, frameW } = props;
  const ref = useRef<HTMLCanvasElement>(null);
  const geo = useMemo(() => geometry(config), [config]);
  const widthPx = frameW * config.scale;
  const heightPx = geo.width > 0 ? (widthPx / geo.width) * geo.height : 0;
  const active = activeAt(props.spans, props.timeMs);
  const activeKey = [...active].sort().join(",");
  const grab = useRef<{ dx: number; dy: number } | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || geo.elements.length === 0) return;
    const ratio = devicePixelRatio;
    canvas.width = Math.max(1, Math.round(widthPx * ratio));
    canvas.height = Math.max(1, Math.round(heightPx * ratio));
    const ctx = canvas.getContext("2d")!;
    const colors = { accent: props.accent, onAccent: onAccent(props.accent) };
    // Same order as the export: everything at rest, then the lit elements on top.
    ctx.globalAlpha = config.opacity;
    drawOverlay(ctx, geo, canvas.width, new Set(), colors);
    const unit = canvas.width / geo.width;
    for (const el of geo.elements) if (active.has(el.id)) drawElement(ctx, el, unit, true, colors);
    // `activeKey` stands for `active`, which is a new set on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [geo, widthPx, heightPx, activeKey, config.opacity, props.accent]);

  if (geo.elements.length === 0) return null;
  const down = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    grab.current = { dx: e.clientX - box.left, dy: e.clientY - box.top };
    e.currentTarget.setPointerCapture(e.pointerId);
    props.onMoveStart();
  };
  const move = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const parent = e.currentTarget.parentElement;
    if (!grab.current || e.buttons !== 1 || !parent) return;
    const frame = parent.getBoundingClientRect();
    const x = (e.clientX - grab.current.dx - frame.left) / frame.width;
    const y = (e.clientY - grab.current.dy - frame.top) / frame.height;
    props.onMove(Math.max(0, Math.min(1 - widthPx / frame.width, x)), Math.max(0, Math.min(1 - heightPx / frame.height, y)));
  };
  return (
    <canvas
      ref={ref}
      className="overlay-canvas"
      title={t("ed.dragToMove")}
      style={{ left: `${config.x * 100}%`, top: `${config.y * 100}%`, width: widthPx, height: heightPx }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={() => (grab.current = null)}
    />
  );
}

export function Editor({ path }: { path: string }) {
  const { clips, go, config, scanning, rescan } = useApp();
  const toast = useToast();
  const clip = useMemo(() => clips.find((c) => c.path === path) ?? null, [clips, path]);
  const videoRef = useRef<HTMLVideoElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const mixer = useRef<TrackMixer | null>(null);

  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [edit, setEditState] = useState<EditDoc | null>(null);
  const [tracks, setTracks] = useState<AudioTrack[]>([]);
  const [audioJob, setAudioJob] = useState<JobStatus | "starting" | null>(null);
  const [audioError, setAudioError] = useState<string | null>(null);
  const [videoSrc, setVideoSrc] = useState(() => fileUrl(path));
  const [videoProblem, setVideoProblem] = useState<"none" | "unplayable" | "proxying" | "proxy">("none");
  const [proxyProgress, setProxyProgress] = useState(0);
  const [timeMs, setTimeMs] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [tempo, setTempo] = useState(1);
  const [reverse, setReverse] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [stage, setStage] = useState({ w: 0, h: 0 });
  const [dialog, setDialog] = useState<"export" | "newclip" | null>(null);
  const history = useRef<EditDoc[]>([]);
  const editRef = useRef<EditDoc | null>(null);
  editRef.current = edit;

  const durationMs = loaded?.source.durationMs ?? 0;
  const fps = loaded?.source.fps || 60;

  // Probe the file, read the cuts and the input track. Waits for the library so that
  // the names of the audio tracks from the sidecar are there.
  const libraryReady = clip !== null || !scanning;
  useEffect(() => {
    if (!libraryReady) return;
    let alive = true;
    (async () => {
      const probe = await call<ProbeResult>("probe", { path });
      const video = probe.streams.find((s) => s.codec_type === "video");
      const meta = clip?.item.meta ?? null;
      const duration = Math.round(Number(probe.format.duration ?? 0) * 1000) || meta?.duration_ms || 0;
      const audio = probe.streams.filter((s) => s.codec_type === "audio");
      const source: Source = {
        path,
        durationMs: duration,
        width: video?.width ?? 0,
        height: video?.height ?? 0,
        fps: frameRate(video?.avg_frame_rate ?? video?.r_frame_rate) || meta?.fps || 60,
        videoCodec: video?.codec_name ?? "",
        audio: audio.map((s, stream) => ({ stream, codec: s.codec_name ?? "", name: meta?.audio_tracks[stream] ?? s.tags?.title ?? t("track.n", stream + 1) })),
      };
      let inputs: InputTrack | null = null;
      let inputsError: string | null = null;
      try {
        const raw = await call<unknown>("inputs_read", { path });
        if (raw !== null) inputs = parseTrack(raw);
      } catch (error) {
        inputsError = errorText(error);
      }
      const streams = editableStreams(probe, meta);
      const saved = await call<unknown>("edit_read", { path }).catch(() => null);
      if (!alive) return;
      setLoaded({ source, probe, inputs, inputsError });
      setEditState(saved ? normalizeEdit(saved, duration, streams.map((s) => s.stream)) : defaultEdit(duration, streams.map((s) => s.stream)));

      // Sound: pull every track out once, then mix them live.
      if (streams.length > 0) {
        setAudioJob("starting");
        try {
          const ready = await prepareTracks(path, streams, duration, (status) => alive && setAudioJob(status));
          if (!alive) return;
          setTracks(ready);
        } catch (error) {
          if (alive) setAudioError(errorText(error));
        } finally {
          if (alive) setAudioJob(null);
        }
      }
    })().catch((error) => alive && setLoadError(errorText(error)));
    return () => {
      alive = false;
    };
    // The clip is looked up once on purpose, a rescan must not restart the editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, libraryReady]);

  // The mixer lives as long as the tracks do.
  useEffect(() => {
    const video = videoRef.current;
    if (tracks.length === 0 || !video) return;
    const m = new TrackMixer(tracks);
    m.attach(video);
    if (editRef.current) m.apply(editRef.current.tracks);
    mixer.current = m;
    // Handle for checks from the browser console, development builds only.
    if (import.meta.env.DEV) (window as unknown as { __klipMixer?: TrackMixer }).__klipMixer = m;
    return () => {
      m.dispose();
      mixer.current = null;
    };
  }, [tracks]);

  useEffect(() => {
    if (edit) mixer.current?.apply(edit.tracks);
  }, [edit]);

  // Save cuts shortly after the last change. An untouched clip has no edit file.
  const dirty = useRef(false);
  const save = useCallback(() => {
    const current = editRef.current;
    if (!dirty.current || !current || durationMs === 0) return;
    dirty.current = false;
    const job = isUntouched(current, durationMs) ? call("edit_delete", { path }) : call("edit_write", { path, edit: current });
    job.catch((error) => toast(t("ed.cutNotSaved", errorText(error)), "error"));
  }, [durationMs, path, toast]);
  useEffect(() => {
    if (!dirty.current) return;
    const timer = window.setTimeout(save, 500);
    return () => window.clearTimeout(timer);
  }, [edit, save]);
  useEffect(() => save, [save]);

  const remember = useCallback(() => {
    if (editRef.current) history.current = [...history.current.slice(-49), editRef.current];
  }, []);
  /** Changes the edit. `record` false is for continuous drags that saved a state up front. */
  const change = useCallback(
    (update: (edit: EditDoc) => EditDoc, record = true) => {
      const current = editRef.current;
      if (!current) return;
      const next = update(current);
      if (next === current) return;
      if (record) remember();
      dirty.current = true;
      // Right away, so that a second change in the same tick builds on this one.
      editRef.current = next;
      setEditState(next);
    },
    [remember],
  );
  const undo = useCallback(() => {
    const previous = history.current.pop();
    if (!previous) return;
    dirty.current = true;
    setEditState(previous);
  }, []);

  const ranges = useMemo(() => (edit ? keptRanges(edit) : []), [edit]);
  const rangesRef = useRef(ranges);
  rangesRef.current = ranges;

  const seek = useCallback(
    (ms: number) => {
      const video = videoRef.current;
      if (!video || durationMs === 0) return;
      const clamped = Math.max(0, Math.min(durationMs, ms));
      video.currentTime = clamped / 1000;
      setTimeMs(clamped);
    },
    [durationMs],
  );

  const stepFrames = useCallback(
    (frames: number) => {
      const video = videoRef.current;
      if (!video) return;
      video.pause();
      setReverse(0);
      // Half a frame further in, so the seek lands inside the wanted frame.
      const index = Math.round(video.currentTime * fps) + frames;
      seek(((index + 0.5) / fps) * 1000);
    },
    [fps, seek],
  );

  const play = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    setReverse(0);
    const from = nextPlayable(rangesRef.current, video.currentTime * 1000 + 1);
    if (from === null) video.currentTime = (rangesRef.current[0]?.start_ms ?? 0) / 1000;
    void video.play().catch((error) => toast(t("ed.playFailed", errorText(error)), "error"));
  }, [toast]);

  const toggle = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) play();
    else video.pause();
  }, [play]);

  // Follow the video clock, skip removed parts, stop at the end of the cut.
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    let raf = 0;
    const tick = () => {
      const ms = video.currentTime * 1000;
      if (!video.paused) {
        const next = nextPlayable(rangesRef.current, ms);
        if (next === null) {
          video.pause();
        } else if (next > ms + 1) {
          video.currentTime = next / 1000;
        }
      }
      setTimeMs(ms);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    const onPlay = () => setPlaying(true);
    const onPause = () => setPlaying(false);
    video.addEventListener("play", onPlay);
    video.addEventListener("pause", onPause);
    return () => {
      cancelAnimationFrame(raf);
      video.removeEventListener("play", onPlay);
      video.removeEventListener("pause", onPause);
    };
  }, [videoSrc]);

  useEffect(() => {
    if (videoRef.current) videoRef.current.playbackRate = tempo;
  }, [tempo, videoSrc]);

  // Browsers cannot play backwards, J steps back in small jumps instead.
  useEffect(() => {
    if (reverse === 0) return;
    const video = videoRef.current;
    if (!video) return;
    video.pause();
    const timer = window.setInterval(() => {
      const next = video.currentTime - 0.1 * reverse;
      if (next <= 0) setReverse(0);
      video.currentTime = Math.max(0, next);
    }, 100);
    return () => window.clearInterval(timer);
  }, [reverse]);

  // Fit the picture into the stage, the overlay needs its exact pixel box.
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const measure = () => setStage({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const aspect = loaded && loaded.source.height > 0 ? loaded.source.width / loaded.source.height : 16 / 9;
  const pictureW = Math.floor(Math.min(stage.w, stage.h * aspect));
  const pictureH = Math.floor(pictureW / aspect);

  // The playhead straight from the video, the `timeMs` state lags a frame behind.
  const nowMs = useCallback(() => Math.round((videoRef.current?.currentTime ?? 0) * 1000), []);
  const splitHere = useCallback(() => change((e) => splitAt(e, nowMs())), [change, nowMs]);
  const removeHere = useCallback(
    () =>
      change((e) => {
        const index = segmentAt(e, nowMs());
        if (index < 0) return e;
        return setSegmentEnabled(e, index, !e.segments[index].enabled);
      }),
    [change, nowMs],
  );
  const joinHere = useCallback(
    () =>
      change((e) => {
        const at = nowMs();
        const index = segmentAt(e, at);
        // Join with the neighbour whose cut is closer to the playhead.
        if (index < 0) return e;
        const s = e.segments[index];
        const toLeft = index > 0 && (index === e.segments.length - 1 || at - s.start_ms < s.end_ms - at);
        return joinAfter(e, toLeft ? index - 1 : index);
      }),
    [change, nowMs],
  );
  // Keyboard: space, J K L, I and O, arrows, S, Delete, Ctrl+Z.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (dialog || ["INPUT", "SELECT", "TEXTAREA"].includes(target.tagName)) {
        if (!(target instanceof HTMLInputElement && target.type === "range")) return;
      }
      const video = videoRef.current;
      if (!video || !editRef.current) return;
      const key = e.key.toLowerCase();
      if ((e.ctrlKey || e.metaKey) && key === "z") {
        undo();
      } else if (e.ctrlKey || e.metaKey || e.altKey) {
        return;
      } else if (e.code === "Space") {
        toggle();
      } else if (key === "k") {
        video.pause();
        setReverse(0);
        video.playbackRate = tempo;
      } else if (key === "l") {
        if (video.paused) play();
        else video.playbackRate = Math.min(4, video.playbackRate * 2);
      } else if (key === "j") {
        setReverse((r) => (r === 0 ? 1 : Math.min(8, r * 2)));
      } else if (key === "i") {
        change((ed) => setIn(ed, Math.round(video.currentTime * 1000), durationMs));
      } else if (key === "o") {
        change((ed) => setOut(ed, Math.round(video.currentTime * 1000), durationMs));
      } else if (key === "s") {
        splitHere();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        removeHere();
      } else if (e.key === "ArrowLeft") {
        if (e.shiftKey) seek(video.currentTime * 1000 - 1000);
        else stepFrames(-1);
      } else if (e.key === "ArrowRight") {
        if (e.shiftKey) seek(video.currentTime * 1000 + 1000);
        else stepFrames(1);
      } else if (e.key === "Home") {
        seek(trimStart(editRef.current));
      } else if (e.key === "End") {
        seek(trimEnd(editRef.current));
      } else if (e.key === "Escape") {
        go({ view: "library" });
      } else {
        return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [change, dialog, durationMs, go, play, removeHere, seek, splitHere, stepFrames, tempo, toggle, undo]);

  // A codec the webview cannot decode: play a small H.264 copy from the cache instead.
  const onVideoError = useCallback(async () => {
    if (videoProblem !== "none") return;
    const proxy = await call<{ path: string; exists: boolean }>("cache_file", { source: path, name: "proxy.mp4" }).catch(() => null);
    if (proxy?.exists) {
      setVideoSrc(fileUrl(proxy.path));
      setVideoProblem("proxy");
    } else {
      setVideoProblem("unplayable");
    }
  }, [path, videoProblem]);

  const makeProxy = useCallback(async () => {
    setVideoProblem("proxying");
    try {
      const proxy = await call<{ path: string }>("cache_file", { source: path, name: "proxy.mp4" });
      const part = proxy.path.replace(/\.mp4$/, ".part.mp4");
      const encoder = await pickEncoder("h264");
      await runJob(
        {
          args: ["-loglevel", "error", "-i", path, "-map", "0:v:0", "-an", "-vf", "scale=-2:'min(720,ih)',format=yuv420p", "-c:v", encoder, "-b:v", "4M", "-g", "30", part],
          total_ms: durationMs,
          outputs: [part],
          renames: [[part, proxy.path]],
        },
        (status) => setProxyProgress(status.progress),
      );
      setVideoSrc(fileUrl(proxy.path));
      setVideoProblem("proxy");
    } catch (error) {
      toast(t("ed.proxyFailed", errorText(error)), "error");
      setVideoProblem("unplayable");
    }
  }, [durationMs, path, toast]);

  const spans = useMemo(() => (loaded?.inputs ? pressIntervals(loaded.inputs) : null), [loaded]);
  const presses = useMemo(() => {
    if (!spans) return null;
    // One lane for everything: merge the spans of all keys and buttons.
    const all = [...spans.values()].flat().sort((a, b) => a.start_ms - b.start_ms);
    const merged: Range[] = [];
    for (const s of all) {
      const last = merged[merged.length - 1];
      if (last && s.start_ms <= last.end_ms) last.end_ms = Math.max(last.end_ms, s.end_ms);
      else merged.push({ ...s });
    }
    return merged;
  }, [spans]);

  if (loadError || (!clip && !scanning && !loaded)) {
    return (
      <div className="boot">
        <p className="boot-error">{loadError ?? t("ed.clipGone")}</p>
        <Button icon={<ArrowLeft size={15} />} onClick={() => go({ view: "library" })}>{t("ed.toLibrary")}</Button>
      </div>
    );
  }

  // A clip without overlay settings of its own follows the settings page: shown or not,
  // and in the chosen corner. Once the clip has its own, those win.
  const overlayBase = loaded?.inputs ? overlayDefault(config.input, aspect) : DEFAULT_OVERLAY;
  const overlay = edit?.input_overlay ?? overlayBase;
  const setOverlay = (patch: Partial<OverlayConfig>, record = true) => change((e) => ({ ...e, input_overlay: { ...(e.input_overlay ?? overlayBase), ...patch } }), record);
  // The webcam: shown by default as the settings say, until the clip has its own choice.
  const cameraSrc = clip?.item.camera ? fileUrl(clip.item.camera) : null;
  const cameraOffset = clip?.item.meta?.camera?.offset_ms ?? 0;
  const cameraBase: CameraEdit = { enabled: true, corner: config.camera?.corner ?? "bottom_right", size_percent: config.camera?.size_percent ?? 25 };
  const camera = edit?.camera ?? cameraBase;
  const setCamera = (patch: Partial<CameraEdit>, record = true) => change((e) => ({ ...e, camera: { ...(e.camera ?? cameraBase), ...patch } }), record);
  const setTrack = (stream: number, patch: Partial<TrackEdit>) => change((e) => ({ ...e, tracks: e.tracks.map((tr) => (tr.stream === stream ? { ...tr, ...patch } : tr)) }));
  const kept = edit ? keptDuration(edit) : 0;
  const currentSegment = edit ? segmentAt(edit, timeMs) : -1;
  const segmentOff = edit && currentSegment >= 0 ? !edit.segments[currentSegment].enabled : false;
  const meta = clip?.item.meta ?? null;
  const events = meta?.events ?? [];
  const untouched = edit ? isUntouched(edit, durationMs) : true;

  return (
    <Shell
      busy
      title={clip?.title ?? path}
      sub={clip ? `${kindLabel(clip.kind)} / ${clip.game ?? t("lib.noGame")} / ${formatDateTime(clip.created)}` : undefined}
      hud={
        <>
        <Button kind="ghost" icon={<ArrowLeft size={16} />} onClick={() => go({ view: "library" })}>{t("nav.library")}</Button>
        <span className="spacer" />
        <Button kind="ghost" icon={<Undo2 size={15} />} title={t("ed.undo")} onClick={undo} disabled={history.current.length === 0} />
        <Button kind="ghost" icon={<RotateCcw size={15} />} disabled={untouched} onClick={() => loaded && edit && change(() => defaultEdit(durationMs, edit.tracks.map((tr) => tr.stream)))}>
          {t("ed.reset")}
        </Button>
        <Button icon={<CopyPlus size={15} />} disabled={!loaded || !edit || ranges.length === 0} onClick={() => setDialog("newclip")}>{t("ex.saveAsNew")}</Button>
        <Button kind="primary" icon={<Share size={15} />} disabled={!loaded || !edit || ranges.length === 0} onClick={() => setDialog("export")}>{t("ex.export")}</Button>
        </>
      }
    >
    <div className="editor">
      <div className="editor-body">
        <div className="stage" ref={stageRef}>
          <div className="picture" style={{ width: pictureW, height: pictureH }}>
            <video
              ref={videoRef}
              src={videoSrc}
              preload="auto"
              playsInline
              // With extracted tracks the mixer makes the sound, the video itself stays silent.
              muted={tracks.length > 0 || videoProblem === "proxy"}
              onClick={toggle}
              onError={() => void onVideoError()}
            />
            {cameraSrc && camera.enabled && videoProblem !== "unplayable" && videoProblem !== "proxying" ? (
              <CameraLayer src={cameraSrc} offsetMs={cameraOffset} timeMs={timeMs} playing={playing} tempo={tempo} edit={camera} frameW={pictureW} />
            ) : null}
            {edit && loaded && spans && overlay.enabled && videoProblem !== "unplayable" && videoProblem !== "proxying" ? (
              <OverlayLayer config={overlay} spans={spans} timeMs={timeMs} frameW={pictureW} accent={cleanAccent(config.accent)} onMoveStart={remember} onMove={(x, y) => setOverlay({ x, y }, false)} />
            ) : null}
          </div>
          {videoProblem === "unplayable" || videoProblem === "proxying" ? (
            <div className="stage-message">
              <strong>{t("ed.cannotPlay", loaded?.source.videoCodec.toUpperCase() || t("ed.thisVideo"))}</strong>
              <p>{t("ed.cannotPlayText")}</p>
              {videoProblem === "proxying" ? <ProgressBar value={proxyProgress} style={{ width: 260 }} /> : <Button kind="primary" onClick={() => void makeProxy()}>{t("ed.makeProxy")}</Button>}
            </div>
          ) : null}
          {audioJob ? (
            <div className="stage-busy">
              {t("ed.preparingAudio")}
              <ProgressBar value={audioJob === "starting" ? 0 : audioJob.progress} />
            </div>
          ) : null}
          {videoProblem === "proxy" ? <div className="stage-busy">{t("ed.proxyNote")}</div> : null}
        </div>

        <aside className="inspector">
          <h3>{t("kind.clip")}</h3>
          <dl className="facts">
            <dt>{t("ed.length")}</dt>
            <dd className="mono">{formatDuration(durationMs)}</dd>
            <dt>{t("ed.afterCut")}</dt>
            <dd className="mono">{formatDuration(kept)}</dd>
            <dt>{t("tl.picture")}</dt>
            <dd className="mono">{loaded ? `${loaded.source.width}x${loaded.source.height} / ${loaded.source.fps} fps` : ""}</dd>
            <dt>Codec</dt>
            <dd className="mono">{loaded?.source.videoCodec.toUpperCase()}</dd>
            <dt>{t("ed.size")}</dt>
            <dd className="mono">{clip ? formatBytes(clip.item.size) : ""}</dd>
            <dt>{t("ed.audioTracks")}</dt>
            <dd className="mono">{loaded?.source.audio.length ?? 0}</dd>
          </dl>
          {loaded && loaded.source.audio.length === 0 ? <p className="muted small insp-note">{t("ed.noSound")}</p> : null}
          {audioError ? <p className="error-text small insp-note">{t("ed.audioUnreadable", audioError)}</p> : null}

          <h3>{t("set.input")}</h3>
          {loaded?.inputs ? (
            <>
              <div className="insp-row">
                <span>{t("ed.showInPicture")}</span>
                <Toggle checked={overlay.enabled} label={t("set.input")} onChange={(enabled) => setOverlay({ enabled })} />
              </div>
              {overlay.enabled ? (
                <>
                  <div className="insp-row">
                    <span>{t("ed.keyboard")}</span>
                    <Select<KeyboardKind> value={overlay.keyboard} onChange={(keyboard) => setOverlay({ keyboard })} options={[{ value: "qwertz", label: "QWERTZ" }, { value: "qwerty", label: "QWERTY" }]} />
                  </div>
                  <div className="insp-row">
                    <span>{t("ed.scope")}</span>
                    <Select<OverlayScope> value={overlay.scope} onChange={(scope) => setOverlay({ scope })} options={SCOPES.map((value) => ({ value, label: scopeLabel(value) }))} />
                  </div>
                  <div className="insp-row">
                    <span>{t("ed.mouse")}</span>
                    <Toggle checked={overlay.mouse} label={t("ed.showMouse")} onChange={(mouse) => setOverlay({ mouse })} />
                  </div>
                  <div className="insp-row">
                    <span>{t("ed.size")}</span>
                    <input type="range" min={0.08} max={0.9} step={0.01} value={overlay.scale} aria-label={t("ed.overlaySize")} onPointerDown={remember} onChange={(e) => setOverlay({ scale: Number(e.target.value) }, false)} />
                  </div>
                  <div className="insp-row">
                    <span>{t("ed.opacity")}</span>
                    <input type="range" min={0.2} max={1} step={0.05} value={overlay.opacity} aria-label={t("ed.overlayOpacity")} onPointerDown={remember} onChange={(e) => setOverlay({ opacity: Number(e.target.value) }, false)} />
                  </div>
                  <p className="muted small">{t("ed.overlayHelp")}</p>
                </>
              ) : null}
            </>
          ) : (
            <p className="muted small">{loaded?.inputsError ?? t("ed.noInputs")}</p>
          )}

          <h3>{t("ed.camera")}</h3>
          {cameraSrc ? (
            <>
              <div className="insp-row">
                <span>{t("ed.cameraShow")}</span>
                <Toggle checked={camera.enabled} label={t("ed.camera")} onChange={(enabled) => setCamera({ enabled })} />
              </div>
              {camera.enabled ? (
                <>
                  <div className="insp-row">
                    <span>{t("cam.corner")}</span>
                    <Select<Corner>
                      value={camera.corner}
                      onChange={(corner) => setCamera({ corner })}
                      options={[
                        { value: "top_left", label: t("in.cornerTopLeft") },
                        { value: "top_right", label: t("in.cornerTopRight") },
                        { value: "bottom_left", label: t("in.cornerBottomLeft") },
                        { value: "bottom_right", label: t("in.cornerBottomRight") },
                      ]}
                    />
                  </div>
                  <div className="insp-row">
                    <span>{t("ed.size")}</span>
                    <input type="range" min={10} max={60} step={1} value={camera.size_percent} aria-label={t("cam.size")} onPointerDown={remember} onChange={(e) => setCamera({ size_percent: Number(e.target.value) }, false)} />
                  </div>
                </>
              ) : null}
            </>
          ) : (
            <p className="muted small">{t("ed.cameraNone")}</p>
          )}

          {events.length > 0 ? (
            <>
              <h3>{t("tl.events")}</h3>
              <div className="event-list">
                {events.map((event, i) => (
                  <button key={i} type="button" className="event-item" onClick={() => seek(event.at_ms)}>
                    <span className={`event-mark${event.kind === "marker" ? " is-marker" : ""}`} />
                    <span className="event-name">{event.label ?? eventLabel(event.kind)}</span>
                    <span className="mono">{formatDuration(event.at_ms)}</span>
                  </button>
                ))}
              </div>
            </>
          ) : null}
        </aside>
      </div>

      <div className="transport">
        <Button kind="ghost" icon={<SkipBack size={16} />} title={t("ed.toStart")} onClick={() => edit && seek(trimStart(edit))} />
        <Button kind="ghost" icon={<ChevronLeft size={16} />} title={t("ed.frameBack")} onClick={() => stepFrames(-1)} />
        <Button kind="primary" icon={playing ? <Pause size={16} /> : <Play size={16} />} title={t("ed.play")} onClick={toggle} />
        <Button kind="ghost" icon={<ChevronRight size={16} />} title={t("ed.frameForward")} onClick={() => stepFrames(1)} />
        <Button kind="ghost" icon={<SkipForward size={16} />} title={t("ed.toEnd")} onClick={() => edit && seek(trimEnd(edit))} />
        <span className="timecode mono">
          {timecode(timeMs, fps)} <span className="total">/ {timecode(durationMs, fps)}</span>
        </span>
        {reverse > 0 ? <span className="mini">{t("ed.reverse", reverse)}</span> : null}
        <span className="vr" />
        <Button small title={t("ed.setIn")} onClick={() => change((e) => setIn(e, nowMs(), durationMs))}>
          {t("ed.in")}<span className="hint">I</span>
        </Button>
        <Button small title={t("ed.setOut")} onClick={() => change((e) => setOut(e, nowMs(), durationMs))}>
          {t("ed.out")}<span className="hint">O</span>
        </Button>
        <Button small icon={<Scissors size={14} />} title={t("ed.splitHere")} onClick={splitHere}>
          {t("ed.split")}<span className="hint">S</span>
        </Button>
        <Button small icon={<Trash2 size={14} />} title={segmentOff ? t("ed.restoreSection") : t("ed.removeSectionHelp")} onClick={removeHere}>
          {segmentOff ? t("ed.restore") : t("ed.removeSection")}
          <span className="hint">{t("key.DELETE")}</span>
        </Button>
        <Button small title={t("ed.joinHelp")} disabled={!edit || edit.segments.length < 2} onClick={joinHere}>
          {t("ed.join")}
        </Button>
        <span className="spacer" />
        <span className="lbl">{t("ed.speed")}</span>
        <Select<number> label={t("ed.speed")} value={tempo} onChange={setTempo} options={TEMPOS.map((rate) => ({ value: rate, label: `${formatNumber(rate)}x` }))} />
        <span className="lbl">Zoom</span>
        <input type="range" min={1} max={12} step={0.5} value={zoom} aria-label={t("ed.zoom")} onChange={(e) => setZoom(Number(e.target.value))} />
      </div>

      {edit && loaded && durationMs > 0 ? (
        <Timeline
          durationMs={durationMs}
          fps={fps}
          timeMs={timeMs}
          zoom={zoom}
          edit={edit}
          events={events}
          tracks={tracks}
          presses={presses}
          onSeek={seek}
          onTrimStart={remember}
          onTrim={(side, ms) => change((e) => (side === "in" ? setIn(e, ms, durationMs) : setOut(e, ms, durationMs)), false)}
          onTrack={setTrack}
        />
      ) : (
        <div className="timeline" />
      )}

      {dialog && loaded && edit && clip ? (
        <ExportDialog
          mode={dialog}
          clip={clip}
          source={loaded.source}
          ranges={ranges}
          // The export works with the overlay as it is shown here, defaults included.
          edit={{ ...(loaded.inputs ? { ...edit, input_overlay: overlay } : edit), camera }}
          camera={clip.item.camera ? { path: clip.item.camera, offsetMs: cameraOffset } : null}
          tracks={tracks}
          inputs={loaded.inputs}
          onClose={(created) => {
            setDialog(null);
            if (created) void rescan();
          }}
        />
      ) : null}
    </div>
    </Shell>
  );
}
