// Builds the ffmpeg command line for an export and picks an encoder that really works
// on this machine. Nothing in here runs ffmpeg except the encoder trial.

import { call } from "./api";
import { t } from "./i18n";
import type { Key } from "./i18n";
import type { Range } from "./edit";

export type Format = "mp4" | "webm" | "mkv" | "gif" | "mp3" | "wav";
export type Quality = "high" | "medium" | "low";
export type AudioMode = "mix" | "separate" | "none";
export type CodecFamily = "h264" | "hevc" | "vp9";

/** `note` is a key of the translation table. */
export const FORMATS: { id: Format; label: string; kind: "video" | "audio"; note: Key }[] = [
  { id: "mp4", label: "MP4", kind: "video", note: "fmt.mp4" },
  { id: "webm", label: "WebM", kind: "video", note: "fmt.webm" },
  { id: "mkv", label: "MKV", kind: "video", note: "fmt.mkv" },
  { id: "gif", label: "GIF", kind: "video", note: "fmt.gif" },
  { id: "mp3", label: "MP3", kind: "audio", note: "fmt.mp3" },
  { id: "wav", label: "WAV", kind: "audio", note: "fmt.wav" },
];

export interface SourceAudio {
  /** Index among the audio streams of the file. */
  stream: number;
  codec: string;
  name: string;
}

export interface Source {
  path: string;
  durationMs: number;
  width: number;
  height: number;
  fps: number;
  /** Codec name as ffprobe reports it, "" for a file without video. */
  videoCodec: string;
  audio: SourceAudio[];
}

export interface ExportTrack {
  stream: number;
  name: string;
  volume: number;
}

export interface ExportOptions {
  format: Format;
  /** null keeps the source height. */
  height: number | null;
  /** null keeps the source rate. */
  fps: number | null;
  quality: Quality;
  codec: "h264" | "hevc";
  audioMode: AudioMode;
  tracks: ExportTrack[];
  /** Upper bound for the file in MiB, null for quality based encoding. */
  targetMb: number | null;
  /** Copy the streams instead of encoding when the edit allows it. */
  allowCopy: boolean;
}

/** A picture laid over the video: always (spans null) or only during the given spans. */
export interface OverlayInput {
  path: string;
  x: number;
  y: number;
  /** Times in ms of the result. */
  spans: Range[] | null;
}

/** The webcam file of a clip, laid over the picture in a corner. */
export interface CameraInput {
  path: string;
  /** Time of the first camera frame on the timeline of the clip. */
  offsetMs: number;
  corner: "top_left" | "top_right" | "bottom_left" | "bottom_right";
  /** Width in percent of the output width. */
  sizePercent: number;
}

export interface Plan {
  /** Everything up to and including the inputs. */
  inputArgs: string[];
  /** The filter graph, "" if none is needed. */
  filter: string;
  /** Mapping, codecs and the output files. */
  outputArgs: string[];
  /** Every file ffmpeg writes, the first one is the main result. */
  outputs: string[];
  totalMs: number;
  video: "copy" | "encode" | "none";
  audio: "copy" | "encode" | "none";
  encoder: string | null;
  videoKbps: number | null;
  audioKbps: number | null;
  warnings: string[];
}

const CONTAINER_VIDEO: Record<string, string[]> = {
  mp4: ["h264", "hevc", "av1"],
  mkv: ["h264", "hevc", "av1", "vp9", "vp8"],
  webm: ["vp9", "vp8", "av1"],
};
const CONTAINER_AUDIO: Record<string, string[]> = {
  mp4: ["aac", "mp3"],
  mkv: ["aac", "mp3", "opus", "flac", "vorbis"],
  webm: ["opus", "vorbis"],
};

export function isVideoFormat(format: Format): boolean {
  return format === "mp4" || format === "webm" || format === "mkv" || format === "gif";
}

export function familyFor(format: Format, codec: "h264" | "hevc"): CodecFamily | null {
  if (format === "webm") return "vp9";
  if (format === "mp4" || format === "mkv") return codec;
  return null;
}

/** Why the video cannot simply be copied, or null if it can. */
export function copyBlocker(source: Source, ranges: Range[], o: ExportOptions, withOverlay = false): string | null {
  if (!isVideoFormat(o.format) || o.format === "gif") return t("ex.bFormat");
  if (withOverlay) return t("ex.bOverlay");
  if (source.videoCodec === "") return t("ex.noPicture");
  if (ranges.length !== 1) return t("ex.bSections");
  if (!CONTAINER_VIDEO[o.format].includes(source.videoCodec)) {
    return t("ex.bContainer", source.videoCodec.toUpperCase(), o.format.toUpperCase());
  }
  if (o.height !== null && o.height !== source.height) return t("ex.bHeight");
  if (o.fps !== null && o.fps !== source.fps) return t("ex.bFps");
  if (o.targetMb !== null) return t("ex.bSize");
  return null;
}

/** Frame size of the result. The width follows the height and is always even. */
export function outputSize(source: Source, o: ExportOptions): { width: number; height: number } {
  const wanted = o.format === "gif" ? (o.height ?? 360) : o.height;
  const height = wanted !== null && wanted < source.height ? wanted : source.height;
  const width = height === source.height ? source.width : Math.round((source.width * height) / source.height / 2) * 2;
  return { width, height };
}

/**
 * The final argument list. A long filter graph (the input overlay brings one span per
 * key press) goes into a file, the Windows command line is limited to 32767 characters.
 */
export function planArgs(plan: Plan, filterScript: string | null): string[] {
  if (plan.filter === "") return [...plan.inputArgs, ...plan.outputArgs];
  const graph = filterScript ? ["-/filter_complex", filterScript] : ["-filter_complex", plan.filter];
  return [...plan.inputArgs, ...graph, ...plan.outputArgs];
}

export const FILTER_SCRIPT_FROM = 4000;

function seconds(ms: number): string {
  return (ms / 1000).toFixed(3);
}

function videoBaseKbps(height: number, fps: number, quality: Quality): number {
  // Rough bitrate ladder for encoders without a usable quality mode.
  const pixels = (height * height * 16) / 9;
  const perPixel = { high: 0.11, medium: 0.065, low: 0.035 }[quality];
  return Math.max(600, Math.round((pixels * Math.min(fps, 60) * perPixel) / 1000));
}

function qualityArgs(encoder: string, quality: Quality, height: number, fps: number): string[] {
  const pick = (high: number, medium: number, low: number) => String({ high, medium, low }[quality]);
  if (encoder === "libx264") return ["-preset", "veryfast", "-crf", pick(18, 23, 28)];
  if (encoder === "libx265") return ["-preset", "fast", "-crf", pick(21, 26, 31), "-x265-params", "log-level=error"];
  if (encoder === "libvpx-vp9") return ["-crf", pick(24, 32, 40), "-b:v", "0", "-deadline", "good", "-cpu-used", "4", "-row-mt", "1"];
  if (encoder.endsWith("_nvenc")) return ["-preset", "p5", "-rc", "vbr", "-cq", pick(19, 24, 30), "-b:v", "0"];
  if (encoder.endsWith("_qsv")) return ["-preset", "medium", "-global_quality", pick(20, 25, 31)];
  if (encoder.endsWith("_amf")) return ["-rc", "cqp", "-qp_i", pick(20, 25, 31), "-qp_p", pick(20, 25, 31)];
  // Media Foundation: plain bitrate, its quality mode depends on the driver.
  return ["-b:v", `${videoBaseKbps(height, fps, quality)}k`];
}

function bitrateArgs(encoder: string, kbps: number): string[] {
  const rate = [`-b:v`, `${kbps}k`, "-maxrate", `${kbps}k`, "-bufsize", `${kbps * 2}k`];
  if (encoder === "libx264") return ["-preset", "medium", ...rate];
  if (encoder === "libx265") return ["-preset", "fast", "-x265-params", "log-level=error", ...rate];
  if (encoder === "libvpx-vp9") return ["-deadline", "good", "-cpu-used", "4", "-row-mt", "1", ...rate];
  if (encoder.endsWith("_nvenc")) return ["-preset", "p5", "-rc", "vbr", ...rate];
  if (encoder.endsWith("_mf")) return ["-b:v", `${kbps}k`];
  return rate;
}

function safeName(text: string): string {
  return text.replace(/[\\/:*?"<>|\s]+/g, "_").replace(/^_+|_+$/g, "") || "track";
}

/** `C:\a\clip.mp3` and "Game" give `C:\a\clip_Game.mp3`. */
function withSuffix(path: string, suffix: string): string {
  const dot = path.lastIndexOf(".");
  return `${path.slice(0, dot)}_${safeName(suffix)}${path.slice(dot)}`;
}

/**
 * The full ffmpeg call. `encoder` is the tested encoder for the codec family of the
 * format, it is ignored where the video is copied or there is none.
 */
export function buildPlan(source: Source, ranges: Range[], o: ExportOptions, encoder: string | null, outPath: string, overlay: OverlayInput[] = [], camera: CameraInput | null = null): Plan {
  if (ranges.length === 0) throw new Error(t("ex.nothingLeft"));
  const totalMs = ranges.reduce((sum, r) => sum + r.end_ms - r.start_ms, 0);
  const warnings: string[] = [];
  const audioOnly = !isVideoFormat(o.format);
  const hasVideo = !audioOnly && source.videoCodec !== "";
  if (!audioOnly && !hasVideo) throw new Error(t("ex.noPicture"));
  const tracks = o.format === "gif" || o.audioMode === "none" ? [] : o.tracks.filter((t) => source.audio.some((a) => a.stream === t.stream));
  if (audioOnly && tracks.length === 0) throw new Error(t("ex.noTrackChosen"));

  const copyVideo = hasVideo && o.allowCopy && copyBlocker(source, ranges, o, overlay.length > 0 || camera !== null) === null;
  const audioCodecOf = (stream: number) => source.audio.find((a) => a.stream === stream)?.codec ?? "";
  const copyAudio =
    copyVideo &&
    o.audioMode === "separate" &&
    tracks.length > 0 &&
    tracks.every((t) => t.volume === 1 && (CONTAINER_AUDIO[o.format] ?? []).includes(audioCodecOf(t.stream)));

  // One input per kept part. Seeking happens on the input, that is fast and, when
  // encoding, exact to the frame.
  let args: string[] = [];
  for (const r of ranges) args.push("-ss", seconds(r.start_ms), "-t", seconds(r.end_ms - r.start_ms), "-i", source.path);
  // The camera is cut the same way, shifted by its offset. Where it starts after a kept
  // part begins, the gap is filled with black.
  const useCamera = camera !== null && hasVideo && !audioOnly;
  const camPads: number[] = [];
  if (useCamera) {
    for (const r of ranges) {
      const at = r.start_ms - camera.offsetMs;
      const pad = Math.max(0, -at);
      camPads.push(pad);
      args.push("-ss", seconds(Math.max(0, at)), "-t", seconds(Math.max(40, r.end_ms - r.start_ms - pad)), "-i", camera.path);
    }
  }
  const pictureBase = ranges.length * (useCamera ? 2 : 1);

  const filters: string[] = [];
  const n = ranges.length;
  const joined = (kind: "v" | "a", spec: string, label: string): string => {
    if (n === 1) return `[0:${spec}]`;
    filters.push(`${ranges.map((_, i) => `[${i}:${spec}]`).join("")}concat=n=${n}:v=${kind === "v" ? 1 : 0}:a=${kind === "a" ? 1 : 0}[${label}]`);
    return `[${label}]`;
  };

  const outHeight = outputSize(source, o).height;
  const outFps = o.fps !== null && o.fps < source.fps ? o.fps : source.fps;
  let videoMap: string | null = null;
  if (hasVideo && !copyVideo) {
    // Size and rate first, then the overlay pictures, then the pixel format or palette.
    const steps: string[] = [];
    if (o.format === "gif") {
      steps.push(`fps=${Math.min(o.fps ?? 15, 30)}`, `scale=-2:${outHeight}:flags=lanczos`);
    } else {
      if (outFps !== source.fps) steps.push(`fps=${outFps}`);
      if (outHeight !== source.height) steps.push(`scale=-2:${outHeight}`);
    }
    let head = joined("v", "v:0", "vc");
    if (overlay.length > 0 || useCamera) {
      filters.push(`${head}${steps.length > 0 ? steps.join(",") : "null"}[ov0]`);
      steps.length = 0;
      head = "[ov0]";
    }
    if (useCamera) {
      const width = outputSize(source, o).width;
      const camW = Math.max(32, Math.round((width * camera.sizePercent) / 100 / 2) * 2);
      const margin = Math.round(width * 0.02);
      ranges.forEach((_, i) => {
        const pad = camPads[i] > 0 ? `tpad=start_mode=add:start_duration=${seconds(camPads[i])}:color=black,` : "";
        filters.push(`[${n + i}:v]${pad}scale=${camW}:-2,setsar=1[cam${i}]`);
      });
      const cam = n === 1 ? "[cam0]" : "[camj]";
      if (n > 1) filters.push(`${ranges.map((_, i) => `[cam${i}]`).join("")}concat=n=${n}:v=1:a=0[camj]`);
      const x = camera.corner.endsWith("left") ? `${margin}` : `main_w-overlay_w-${margin}`;
      const y = camera.corner.startsWith("top") ? `${margin}` : `main_h-overlay_h-${margin}`;
      filters.push(`${head}${cam}overlay=x=${x}:y=${y}:eof_action=pass[ovcam]`);
      head = "[ovcam]";
    }
    if (overlay.length > 0) {
      let at = head;
      overlay.forEach((picture, i) => {
        args.push("-i", picture.path);
        const lit = picture.spans === null ? "" : `:enable='${picture.spans.map((s) => `between(t,${seconds(s.start_ms)},${seconds(s.end_ms)})`).join("+")}'`;
        filters.push(`${at}[${pictureBase + i}:v]overlay=x=${picture.x}:y=${picture.y}${lit}[ov${i + 1}]`);
        at = `[ov${i + 1}]`;
      });
      head = at;
    }
    if (o.format === "gif") {
      filters.push(`${head}${[...steps, "split[g1][g2]"].join(",")}`, "[g1]palettegen=stats_mode=diff[pal]", "[g2][pal]paletteuse=dither=bayer:bayer_scale=4[v]");
    } else {
      filters.push(`${head}${[...steps, "format=yuv420p"].join(",")}[v]`);
    }
    videoMap = "[v]";
  }

  // Audio: every chosen track is cut the same way, then mixed or kept apart.
  const audioMaps: { label: string; name: string }[] = [];
  if (tracks.length > 0 && !copyAudio) {
    const shaped = tracks.map((t, i) => {
      const from = joined("a", `a:${t.stream}`, `ac${i}`);
      filters.push(`${from}volume=${t.volume.toFixed(3)}[a${i}]`);
      return { label: `[a${i}]`, name: t.name };
    });
    if (o.audioMode === "mix" && shaped.length > 1) {
      filters.push(`${shaped.map((s) => s.label).join("")}amix=inputs=${shaped.length}:normalize=0:dropout_transition=0,alimiter=limit=0.95[amix]`);
      audioMaps.push({ label: "[amix]", name: "Mix" });
    } else {
      audioMaps.push(...shaped);
    }
  }

  // Bitrates for a target size.
  const audioStreams = copyAudio ? tracks.length : audioMaps.length;
  let videoKbps: number | null = null;
  let audioKbps: number | null = audioStreams > 0 ? (o.format === "webm" ? 128 : o.format === "mp3" ? 192 : 160) : null;
  if (o.format === "wav") audioKbps = 1536;
  if (o.targetMb !== null && o.format !== "gif" && o.format !== "wav") {
    // 6 percent stay free for the container and for encoders that overshoot a little.
    const totalKbps = (o.targetMb * 8388.608 * 0.94) / (totalMs / 1000);
    if (audioOnly) {
      audioKbps = Math.max(32, Math.min(320, Math.floor(totalKbps / audioStreams)));
    } else {
      audioKbps = audioStreams > 0 ? 96 : null;
      videoKbps = Math.floor(totalKbps - (audioKbps ?? 0) * audioStreams);
      if (videoKbps < 150) {
        warnings.push(t("ex.wTight"));
        videoKbps = Math.max(videoKbps, 50);
      }
    }
  } else if (o.targetMb !== null) {
    warnings.push(t("ex.wNoSize"));
  }

  const inputArgs = args;
  const filter = filters.join(";");
  // From here on `args` collects what follows the filter graph.
  args = [];

  const audioCodec = (): string[] => {
    if (o.format === "webm") return ["-c:a", "libopus", "-b:a", `${audioKbps}k`];
    if (o.format === "mp3") return ["-c:a", "libmp3lame", "-b:a", `${audioKbps}k`];
    if (o.format === "wav") return ["-c:a", "pcm_s16le"];
    return ["-c:a", "aac", "-b:a", `${audioKbps}k`];
  };

  const outputs: string[] = [];
  if (audioOnly) {
    // Separate tracks become separate files.
    audioMaps.forEach((a) => {
      const path = audioMaps.length > 1 ? withSuffix(outPath, a.name) : outPath;
      args.push("-map", a.label, "-vn", ...audioCodec(), path);
      outputs.push(path);
    });
    return { inputArgs, filter, outputArgs: args, outputs, totalMs, video: "none", audio: "encode", encoder: null, videoKbps: null, audioKbps, warnings };
  }

  if (copyVideo) {
    args.push("-map", "0:v:0", "-c:v", "copy");
    warnings.push(t("ex.wKeyframe"));
  } else if (o.format === "gif") {
    args.push("-map", "[v]", "-loop", "0");
  } else {
    if (!encoder) throw new Error(t("ex.noEncoder"));
    args.push("-map", videoMap!, "-c:v", encoder);
    args.push(...(videoKbps !== null ? bitrateArgs(encoder, videoKbps) : qualityArgs(encoder, o.quality, outHeight, outFps)));
    if (o.format === "mp4" && (encoder.includes("hevc") || encoder === "libx265")) args.push("-tag:v", "hvc1");
  }
  if (copyAudio) {
    tracks.forEach((t, i) => args.push("-map", `0:a:${t.stream}`, `-metadata:s:a:${i}`, `title=${t.name}`));
    args.push("-c:a", "copy");
  } else if (audioMaps.length > 0) {
    audioMaps.forEach((a, i) => args.push("-map", a.label, `-metadata:s:a:${i}`, `title=${a.name}`));
    args.push(...audioCodec());
  } else {
    args.push("-an");
  }
  if (o.format === "mp4") args.push("-movflags", "+faststart");
  args.push(outPath);
  outputs.push(outPath);
  return {
    inputArgs,
    filter,
    outputArgs: args,
    outputs,
    totalMs,
    video: copyVideo ? "copy" : "encode",
    audio: copyAudio ? "copy" : audioMaps.length > 0 ? "encode" : "none",
    encoder: copyVideo || o.format === "gif" ? null : encoder,
    videoKbps,
    audioKbps: copyAudio ? null : audioKbps,
    warnings,
  };
}

// Encoders, best first. The last one of each family is software and always the fallback.
const CANDIDATES: Record<CodecFamily, string[]> = {
  h264: ["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf", "libx264"],
  hevc: ["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_mf", "libx265"],
  vp9: ["vp9_qsv", "libvpx-vp9"],
};

export function isSoftware(encoder: string): boolean {
  return encoder.startsWith("lib");
}

export interface EncoderTrial {
  encoder: string;
  ok: boolean;
  ms: number;
  message: string;
}

const trials = new Map<string, Promise<EncoderTrial>>();

/**
 * Encodes ten frames of a test picture. An encoder that is compiled in can still fail
 * at runtime (driver too old, no such GPU), only a real run tells.
 */
export function tryEncoder(encoder: string): Promise<EncoderTrial> {
  let trial = trials.get(encoder);
  if (!trial) {
    trial = call<{ code: number | null; log: string; ms: number }>("ffmpeg_run", {
      args: ["-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30", "-frames:v", "10", "-c:v", encoder, "-f", "null", "-"],
      timeout_ms: 15000,
    }).then(
      (r) => ({ encoder, ok: r.code === 0, ms: r.ms, message: r.code === 0 ? "" : r.log.trim().split("\n").slice(-2).join(" ") || t("ex.timeout") }),
      (e: unknown) => ({ encoder, ok: false, ms: 0, message: e instanceof Error ? e.message : String(e) }),
    );
    trials.set(encoder, trial);
  }
  return trial;
}

export function forgetEncoderTrials(): void {
  trials.clear();
}

/** The first encoder of the family that passes the trial. */
export async function pickEncoder(family: CodecFamily): Promise<string> {
  for (const encoder of CANDIDATES[family]) {
    if ((await tryEncoder(encoder)).ok) return encoder;
  }
  throw new Error(t("ex.noEncoderWorks", family.toUpperCase()));
}

export async function tryAllEncoders(): Promise<EncoderTrial[]> {
  const all = [...CANDIDATES.h264, ...CANDIDATES.hevc, ...CANDIDATES.vp9];
  const results: EncoderTrial[] = [];
  for (const encoder of all) results.push(await tryEncoder(encoder));
  return results;
}
