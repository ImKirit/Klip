import type { OverlayConfig } from "./inputs";

// Shapes of the files Klip reads and writes. ClipMeta and Config mirror klip-core,
// the rest belongs to the app.

export type ClipKind = "clip" | "recording" | "screenshot" | "auto_clip";
export type VideoCodec = "h264" | "hevc" | "av1";

export interface ClipEvent {
  at_ms: number;
  kind: string;
  label?: string;
}

/** `<name>.klip.json`, written by the recorder. Unknown fields are kept on save. */
export interface ClipMeta {
  version: number;
  kind: ClipKind;
  created: string;
  game?: string;
  /** Why an automatic clip was saved, such as "ace". */
  reason?: string;
  duration_ms: number;
  width: number;
  height: number;
  fps: number;
  codec: VideoCodec;
  encoder: string;
  audio_tracks: string[];
  events: ClipEvent[];
  favorite: boolean;
  tags: string[];
}

/** One file of the clip folder as the backend reports it. */
export interface LibraryItem {
  path: string;
  name: string;
  ext: string;
  folder: string;
  media: "video" | "image";
  size: number;
  modified_ms: number;
  meta: ClipMeta | null;
  meta_error: string | null;
  has_edit: boolean;
  has_inputs: boolean;
  thumb: string | null;
}

export type HotkeyAction =
  | { type: "clip"; seconds: number }
  | { type: "record_toggle" }
  | { type: "record_pause" }
  | { type: "marker" }
  | { type: "screenshot" }
  | { type: "overlay" }
  | { type: "input_pause" };

/** A corner of the picture, where the input overlay sits by default. */
export type Corner = "top_left" | "top_right" | "bottom_left" | "bottom_right";

export interface HotkeyBinding {
  keys: string;
  action: HotkeyAction;
}

/** A program that gets its own audio track. */
export interface AppTrack {
  name: string;
  exe: string[];
}

export interface AudioConfig {
  enabled: boolean;
  microphone: boolean;
  /** "all": every program that plays sound gets a track, `apps` only groups and names.
   *  "selected": only the programs in `apps` get one. */
  mode: "all" | "selected";
  apps: AppTrack[];
}

/** What the recorder reports over its pipe. */
export interface RecorderStatus {
  version: string;
  buffer_seconds: number;
  buffered_seconds: number;
  recording: boolean;
  recording_ms: number;
  encoder: string;
  width: number;
  height: number;
  fps: number;
  audio_tracks: string[];
  last_saved?: string;
  game?: string;
}

/** `%APPDATA%\Klip\config.json`, read by the recorder. */
export interface Config {
  version: number;
  buffer_seconds: number;
  buffer_on_disk: boolean;
  fps: number;
  bitrate_kbps: number;
  codec: VideoCodec;
  height: number | null;
  capture_cursor: boolean;
  clip_dir: string | null;
  max_recording_minutes: number;
  hotkeys: HotkeyBinding[];
  audio: AudioConfig;
  buffer_mode: "always" | "game_only";
  input: { enabled: boolean; layout: string | null; live: boolean; show_in_clips: boolean; corner: Corner };
  overlay: { enabled: boolean; indicator: boolean; toasts: boolean; sound: boolean; sound_volume: number; panel_pos?: [number, number] | null };
  auto_clips: boolean;
  /** Left, top, width, height in pixels of the source. Null means the whole source. */
  region: [number, number, number, number] | null;
  /** Language of every visible text: "en" or "de". */
  language: string;
  /** Accent colour as "#RRGGBB", shared by app, tray and overlay. */
  accent: string;
  /** Folder for temporary files, the default if null. */
  cache_dir: string | null;
  /** Only a drive like "E:", the default path is then used on that drive. Ignored when
   *  `cache_dir` is set. */
  cache_drive: string | null;
  [unknown: string]: unknown;
}

/** One entry of `autoclip.json`, mirrors `klip_detect::Rule`. */
export interface AutoClipRule {
  enabled: boolean;
  before_s: number;
  after_s: number;
}

/** `%APPDATA%\Klip\autoclip.json`, mirrors `klip_detect::RuleSet`. */
export interface AutoClipRules {
  rules: Record<string, AutoClipRule>;
  merge_gap_s: number;
  max_clip_s: number;
  min_confidence: number;
}

/** `%APPDATA%\Klip\app.json`, settings only the app cares about. */
export interface AppSettings {
  version: number;
  theme: "dark" | "light";
  /** Empty means the default path. */
  ffmpeg_path: string;
  /** Shown as the upper bound of the storage bar. 0 means no limit. */
  storage_limit_gb: number;
  cleanup: {
    enabled: boolean;
    /** 0 switches the rule off. */
    max_age_days: number;
    /** 0 switches the rule off. */
    max_size_gb: number;
  };
  /** Empty means `<clip folder>\Exporte`. */
  export_dir: string;
  sort: SortKey;
}

export type SortKey = "date_desc" | "date_asc" | "length_desc" | "length_asc" | "name" | "size_desc";

export interface Segment {
  start_ms: number;
  end_ms: number;
  /** A removed section stays in the file and can be brought back. */
  enabled: boolean;
}

export interface TrackEdit {
  /** Index among the audio streams of the file, 0 based. */
  stream: number;
  muted: boolean;
  solo: boolean;
  /** 1 is unchanged, 0 to 2. */
  volume: number;
}

/** `<name>.edit.json`, the cuts of the editor. The video itself is never changed. */
export interface EditDoc {
  version: 1;
  segments: Segment[];
  tracks: TrackEdit[];
  /** Only present once the overlay was touched. */
  input_overlay?: OverlayConfig;
}

export interface Env {
  clip_dir: string;
  clip_dir_from_env: boolean;
  default_clip_dir: string;
  export_dir: string;
  data_dir: string;
  cache_dir: string;
  /** True while KLIP_CACHE_DIR overrides the settings. */
  cache_dir_from_env: boolean;
  default_cache_dir: string;
  config_path: string;
  default_ffmpeg: string;
  ffmpeg: string;
  selftest: string | null;
}

export interface JobStatus {
  state: "running" | "done" | "failed" | "cancelled";
  out_ms: number;
  progress: number;
  speed: string;
  log: string;
  code: number | null;
}

export interface ProbeStream {
  index: number;
  codec_type: string;
  codec_name?: string;
  width?: number;
  height?: number;
  r_frame_rate?: string;
  avg_frame_rate?: string;
  channels?: number;
  duration?: string;
  tags?: Record<string, string>;
}

export interface ProbeResult {
  streams: ProbeStream[];
  format: { duration?: string; size?: string; format_name?: string; bit_rate?: string };
}
