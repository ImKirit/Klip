// Sound in the editor. A browser plays only the first audio stream of a video, but a
// Klip clip carries one stream per source. So every stream is pulled out once with
// ffmpeg into its own small file in the cache, and here those files play in step with
// the muted video, each through its own gain. That is what makes mute, solo and volume
// per track real.

import { call, fileUrl, runJob } from "./api";
import { effectiveGain } from "./edit";
import { t } from "./i18n";
import type { ClipMeta, JobStatus, ProbeResult, TrackEdit } from "./types";

export interface AudioTrack {
  /** Index among the audio streams of the file. */
  stream: number;
  name: string;
  codec: string;
  /** Extracted audio file in the cache. */
  file: string;
  /** Loudest value per bucket, 0 to 1. */
  peaks: number[];
  peaksPerSecond: number;
}

const PEAK_RATE = 4000;

/**
 * The streams the editor works with. With several streams the first one is the ready
 * made mix of the recorder, the editor mixes the single sources itself and leaves it out.
 */
export function editableStreams(probe: ProbeResult, meta: ClipMeta | null): { stream: number; name: string; codec: string }[] {
  const audio = probe.streams.filter((s) => s.codec_type === "audio");
  const all = audio.map((s, stream) => ({
    stream,
    codec: s.codec_name ?? "",
    name: meta?.audio_tracks[stream] ?? s.tags?.title ?? (audio.length === 1 ? t("track.sound") : t("track.n", stream + 1)),
  }));
  return all.length > 1 ? all.slice(1) : all;
}

/** Extracts the given streams into the cache (once) and reads their waveforms. */
export async function prepareTracks(
  source: string,
  streams: { stream: number; name: string; codec: string }[],
  durationMs: number,
  onProgress?: (status: JobStatus) => void,
): Promise<AudioTrack[]> {
  const files = await Promise.all(
    streams.map(async (s) => ({
      ...s,
      audio: await call<{ path: string; exists: boolean }>("cache_file", { source, name: `a${s.stream}.m4a` }),
      pcm: await call<{ path: string; exists: boolean }>("cache_file", { source, name: `a${s.stream}.pcm` }),
    })),
  );
  const missing = files.filter((f) => !f.audio.exists || !f.pcm.exists);
  if (missing.length > 0) {
    const args = ["-loglevel", "error", "-i", source];
    const outputs: string[] = [];
    const renames: [string, string][] = [];
    for (const f of missing) {
      // The .part names keep a half written file from ever looking finished.
      const audioPart = f.audio.path.replace(/\.m4a$/, ".part.m4a");
      const pcmPart = f.pcm.path.replace(/\.pcm$/, ".part.pcm");
      const codec = f.codec === "aac" ? ["-c:a", "copy"] : ["-c:a", "aac", "-b:a", "192k"];
      args.push("-map", `0:a:${f.stream}`, "-vn", ...codec, "-f", "mp4", audioPart);
      args.push("-map", `0:a:${f.stream}`, "-vn", "-ac", "1", "-ar", String(PEAK_RATE), "-f", "s16le", pcmPart);
      outputs.push(audioPart, pcmPart);
      renames.push([audioPart, f.audio.path], [pcmPart, f.pcm.path]);
    }
    await runJob({ args, total_ms: durationMs, outputs, renames }, onProgress);
  }
  // About 6000 points are plenty for any zoom the timeline offers.
  const perSecond = Math.max(4, Math.min(200, Math.round(6000 / Math.max(1, durationMs / 1000))));
  return Promise.all(
    files.map(async (f) => {
      const wave = await call<{ peaks: number[]; per_second: number }>("peaks", { path: f.pcm.path, rate: PEAK_RATE, per_second: perSecond });
      return { stream: f.stream, name: f.name, codec: f.codec, file: f.audio.path, peaks: wave.peaks, peaksPerSecond: wave.per_second };
    }),
  );
}

interface Voice {
  stream: number;
  element: HTMLAudioElement;
  gain: GainNode;
  analyser: AnalyserNode;
}

/** Plays the extracted tracks in step with a video element. */
export class TrackMixer {
  private context: AudioContext;
  private master: GainNode;
  private voices: Voice[] = [];
  private video: HTMLVideoElement | null = null;
  private timer = 0;
  private detach: (() => void) | null = null;

  constructor(tracks: AudioTrack[]) {
    this.context = new AudioContext();
    this.master = this.context.createGain();
    this.master.connect(this.context.destination);
    for (const track of tracks) {
      const element = new Audio();
      // Needed in the app: the asset protocol is another origin than the page.
      element.crossOrigin = "anonymous";
      element.preload = "auto";
      element.src = fileUrl(track.file);
      const source = this.context.createMediaElementSource(element);
      const gain = this.context.createGain();
      const analyser = this.context.createAnalyser();
      analyser.fftSize = 256;
      source.connect(gain);
      gain.connect(analyser);
      analyser.connect(this.master);
      this.voices.push({ stream: track.stream, element, gain, analyser });
    }
  }

  get state(): AudioContextState {
    return this.context.state;
  }

  /** Follows play, pause, seek and speed of the video from now on. */
  attach(video: HTMLVideoElement): void {
    this.detach?.();
    this.video = video;
    const play = () => {
      void this.context.resume();
      this.align(true);
      for (const v of this.voices) void v.element.play().catch(() => undefined);
    };
    const pause = () => {
      for (const v of this.voices) v.element.pause();
      this.align(true);
    };
    const seek = () => this.align(true);
    const rate = () => {
      for (const v of this.voices) v.element.playbackRate = video.playbackRate;
    };
    video.addEventListener("play", play);
    video.addEventListener("pause", pause);
    video.addEventListener("seeked", seek);
    video.addEventListener("ratechange", rate);
    video.addEventListener("waiting", pause);
    video.addEventListener("playing", play);
    // Two clocks drift apart over time, pull the sound back when it is audible.
    this.timer = window.setInterval(() => this.align(false), 400);
    this.detach = () => {
      video.removeEventListener("play", play);
      video.removeEventListener("pause", pause);
      video.removeEventListener("seeked", seek);
      video.removeEventListener("ratechange", rate);
      video.removeEventListener("waiting", pause);
      video.removeEventListener("playing", play);
      window.clearInterval(this.timer);
    };
  }

  private align(force: boolean): void {
    const video = this.video;
    if (!video) return;
    for (const v of this.voices) {
      const drift = Math.abs(v.element.currentTime - video.currentTime);
      if (force ? drift > 0.01 : drift > 0.07) v.element.currentTime = video.currentTime;
      if (!video.paused && v.element.paused && !video.ended) void v.element.play().catch(() => undefined);
    }
  }

  /** Applies mute, solo and volume. A short ramp avoids clicks. */
  apply(tracks: TrackEdit[]): void {
    const gains = effectiveGain(tracks);
    for (const v of this.voices) {
      v.gain.gain.setTargetAtTime(gains.get(v.stream) ?? 1, this.context.currentTime, 0.015);
    }
  }

  /** Volume of everything together. The levels of level() are measured before it. */
  setMaster(volume: number): void {
    this.master.gain.value = volume;
  }

  /** Current loudness of one track after its gain, 0 to 1. For meters and the self test. */
  level(stream: number): number {
    const voice = this.voices.find((v) => v.stream === stream);
    if (!voice) return 0;
    const data = new Uint8Array(voice.analyser.fftSize);
    voice.analyser.getByteTimeDomainData(data);
    let peak = 0;
    for (const value of data) peak = Math.max(peak, Math.abs(value - 128) / 128);
    return peak;
  }

  dispose(): void {
    this.detach?.();
    for (const v of this.voices) {
      v.element.pause();
      v.element.removeAttribute("src");
      v.element.load();
    }
    void this.context.close();
  }
}
