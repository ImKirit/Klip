// Self test of the real app, started with KLIP_SELFTEST set. It answers what cannot be
// checked in a normal browser: which codecs WebView2 decodes through the asset protocol,
// and whether the extracted audio tracks really reach the mixer there. The result goes
// to `selftest.json` in the data folder, then the app quits.

import { call, errorText, fileUrl, quitApp } from "./lib/api";
import { editableStreams, prepareTracks, TrackMixer } from "./lib/audio";
import type { Clip } from "./lib/library";
import type { ProbeResult } from "./lib/types";

let started = false;

function loadVideo(path: string): Promise<{ ok: boolean; width: number; height: number; duration: number; error: string | null }> {
  return new Promise((resolve) => {
    const video = document.createElement("video");
    video.muted = true;
    video.preload = "auto";
    const finish = (ok: boolean, error: string | null) => {
      window.clearTimeout(timer);
      resolve({ ok, width: video.videoWidth, height: video.videoHeight, duration: video.duration || 0, error });
      video.removeAttribute("src");
      video.load();
    };
    const timer = window.setTimeout(() => finish(false, "timeout after 8 s"), 8000);
    // `loadeddata` means the first frame was really decoded.
    video.addEventListener("loadeddata", () => finish(video.videoWidth > 0, video.videoWidth > 0 ? null : "no picture"));
    video.addEventListener("error", () => finish(false, `media error ${video.error?.code ?? "?"}: ${video.error?.message ?? ""}`));
    video.src = fileUrl(path);
  });
}

function loadImage(path: string): Promise<boolean> {
  return new Promise((resolve) => {
    const image = new Image();
    image.onload = () => resolve(image.naturalWidth > 0);
    image.onerror = () => resolve(false);
    image.src = fileUrl(path);
  });
}

async function audioCheck(clip: Clip) {
  const probe = await call<ProbeResult>("probe", { path: clip.path });
  const streams = editableStreams(probe, clip.item.meta);
  const tracks = await prepareTracks(clip.path, streams, clip.durationMs);
  const video = document.createElement("video");
  video.muted = true;
  video.src = fileUrl(clip.path);
  const mixer = new TrackMixer(tracks);
  // Silent for the person at the machine, the levels are measured before the master.
  mixer.setMaster(0);
  mixer.attach(video);
  const states = streams.map((s) => ({ stream: s.stream, muted: false, solo: false, volume: 1 }));
  mixer.apply(states);
  let playError: string | null = null;
  await video.play().catch((e) => (playError = errorText(e)));
  const sample = async () => {
    const peak = new Map<number, number>();
    for (let i = 0; i < 12; i++) {
      await new Promise((r) => window.setTimeout(r, 100));
      for (const s of streams) peak.set(s.stream, Math.max(peak.get(s.stream) ?? 0, mixer.level(s.stream)));
    }
    return streams.map((s) => ({ name: s.name, level: Number((peak.get(s.stream) ?? 0).toFixed(3)) }));
  };
  const open = await sample();
  // Mute the first track: its level must drop to nothing while the others stay.
  mixer.apply(states.map((s, i) => (i === 0 ? { ...s, muted: true } : s)));
  await new Promise((r) => window.setTimeout(r, 200));
  const firstMuted = await sample();
  const result = { clip: clip.title, context: mixer.state, playError, videoTime: Number(video.currentTime.toFixed(2)), open, firstMuted };
  video.pause();
  mixer.dispose();
  return result;
}

export async function runSelfTest(clips: Clip[]): Promise<void> {
  if (started) return;
  started = true;
  const report: Record<string, unknown> = { when: new Date().toISOString(), userAgent: navigator.userAgent };
  try {
    const videos = clips.filter((c) => c.item.media === "video");
    // One clip per codec is enough.
    const byCodec = new Map<string, Clip>();
    for (const clip of videos) {
      const codec = clip.item.meta?.codec ?? "unknown";
      if (!byCodec.has(codec)) byCodec.set(codec, clip);
    }
    const playback = [];
    for (const [codec, clip] of byCodec) playback.push({ codec, file: clip.title, ...(await loadVideo(clip.path)) });
    report.playback = playback;

    const first = videos[0];
    if (first) {
      const thumb = await call<{ path: string }>("thumb", { path: first.path, at_ms: 500 });
      report.thumbnail = await loadImage(thumb.path);
    }
    const withSound = videos.find((c) => (c.item.meta?.audio_tracks.length ?? 0) > 2 && c.item.meta?.codec === "h264");
    report.audio = withSound ? await audioCheck(withSound).catch((e) => ({ error: errorText(e) })) : "no clip with several tracks";
    report.fonts = ["Chakra Petch", "Barlow", "JetBrains Mono"].map((f) => ({ font: f, loaded: document.fonts.check(`16px "${f}"`) }));
  } catch (error) {
    report.error = errorText(error);
  }
  await call("selftest_report", { report }).catch(() => undefined);
  await quitApp();
}
