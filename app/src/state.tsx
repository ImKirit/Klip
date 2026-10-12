// Everything the views share: environment, both settings files, the library, the state
// of the recorder and the current route.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { call, errorText } from "./lib/api";
import { toClip } from "./lib/library";
import type { Clip, Filters } from "./lib/library";
import { NO_FILTERS, planCleanup } from "./lib/library";
import { applyTheme, normalizeSettings } from "./lib/settings";
import { formatBytes } from "./lib/format";
import { cleanLang, setLang, t, tn } from "./lib/i18n";
import type { Lang } from "./lib/i18n";
import type { AppSettings, ClipMeta, Config, Env, LibraryItem, ProbeResult, RecorderStatus } from "./lib/types";
import { useToast } from "./ui";

export type Route = { view: "library" } | { view: "editor"; path: string } | { view: "settings"; page: string };

function parseRoute(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/");
  if (parts[0] === "editor" && parts[1]) return { view: "editor", path: decodeURIComponent(parts.slice(1).join("/")) };
  if (parts[0] === "settings") return { view: "settings", page: parts[1] || "recording" };
  return { view: "library" };
}

function routeHash(route: Route): string {
  if (route.view === "editor") return `#/editor/${encodeURIComponent(route.path)}`;
  if (route.view === "settings") return `#/settings/${route.page}`;
  return "#/library";
}

/** What the pipe of the recorder answered last. Null until the first answer is in. */
export type RecorderState = { connected: boolean; status?: RecorderStatus | null } | null;

interface AppState {
  env: Env;
  config: Config;
  configError: string | null;
  settings: AppSettings;
  /** Language of every visible text, from `config.language`. */
  lang: Lang;
  saveConfig: (config: Config) => Promise<void>;
  /** What happened to the last reload request: the recorder took it, or it is not running. */
  recorderReload: "none" | "sent" | "offline";
  recorder: RecorderState;
  saveSettings: (settings: AppSettings) => Promise<void>;
  clips: Clip[];
  libraryDir: string;
  libraryMissing: boolean;
  scanning: boolean;
  rescan: () => Promise<Clip[]>;
  /** Writes the sidecar and updates the list. Creates the sidecar if the clip has none. */
  updateMeta: (clip: Clip, patch: Partial<ClipMeta>) => Promise<void>;
  trash: (clips: Clip[]) => Promise<void>;
  rename: (clip: Clip, name: string) => Promise<string>;
  disk: { free: number; total: number } | null;
  route: Route;
  go: (route: Route) => void;
  filters: Filters;
  setFilters: (filters: Filters) => void;
}

const Context = createContext<AppState | null>(null);

export function useApp(): AppState {
  const state = useContext(Context);
  if (!state) throw new Error("AppState is missing");
  return state;
}

function frameRate(text: string | undefined): number {
  if (!text) return 0;
  const [a, b] = text.split("/").map(Number);
  return b ? Math.round(a / b) : Math.round(a || 0);
}

/** Builds a sidecar for a file that has none, from what ffprobe sees. */
export async function metaFromProbe(item: LibraryItem): Promise<ClipMeta> {
  const probe = await call<ProbeResult>("probe", { path: item.path });
  const video = probe.streams.find((s) => s.codec_type === "video");
  const audio = probe.streams.filter((s) => s.codec_type === "audio");
  const codec = video?.codec_name ?? "";
  const isImage = item.media === "image";
  if (!isImage && !["h264", "hevc", "av1"].includes(codec)) {
    throw new Error(t("state.codecNoSidecar", codec || t("common.unknown")));
  }
  const local = new Date(item.modified_ms);
  const offset = -local.getTimezoneOffset();
  const pad = (n: number) => String(Math.floor(Math.abs(n))).padStart(2, "0");
  const created =
    `${local.getFullYear()}-${pad(local.getMonth() + 1)}-${pad(local.getDate())}T${pad(local.getHours())}:${pad(local.getMinutes())}:${pad(local.getSeconds())}` +
    `${offset >= 0 ? "+" : "-"}${pad(offset / 60)}:${pad(offset % 60)}`;
  return {
    version: 1,
    kind: isImage ? "screenshot" : "clip",
    created,
    game: item.folder.split(/[\\/]/)[0] || undefined,
    duration_ms: isImage ? 0 : Math.round(Number(probe.format.duration ?? 0) * 1000),
    width: video?.width ?? 0,
    height: video?.height ?? 0,
    fps: isImage ? 0 : frameRate(video?.avg_frame_rate ?? video?.r_frame_rate),
    codec: isImage ? "h264" : (codec as ClipMeta["codec"]),
    // Data in the sidecar, not a text of the interface: always the same wording.
    encoder: isImage ? item.ext : "unknown (imported without sidecar)",
    audio_tracks: audio.map((s, i) => s.tags?.title ?? (audio.length === 1 ? t("track.sound") : t("track.n", i + 1))),
    events: [],
    favorite: false,
    tags: [],
  };
}

export function AppProvider(props: { env: Env; config: Config; configError: string | null; settings: AppSettings; children: ReactNode }) {
  const toast = useToast();
  const [env, setEnv] = useState(props.env);
  const [config, setConfig] = useState(props.config);
  const [configError, setConfigError] = useState(props.configError);
  const [settings, setSettings] = useState(props.settings);
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [libraryDir, setLibraryDir] = useState(props.env.clip_dir);
  const [libraryMissing, setLibraryMissing] = useState(false);
  const [scanning, setScanning] = useState(true);
  const [disk, setDisk] = useState<{ free: number; total: number } | null>(null);
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.hash));
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const cleaned = useRef(false);
  const [recorderReload, setRecorderReload] = useState<AppState["recorderReload"]>("none");
  const [recorder, setRecorder] = useState<RecorderState>(null);

  // The language is set while rendering, before any child asks for a text.
  const lang = cleanLang(config.language);
  setLang(lang);

  useEffect(() => applyTheme(settings.theme, config.accent), [settings.theme, config.accent]);

  useEffect(() => {
    const onHash = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  // The answer comes from the recorder itself over its pipe. No answer means no recorder.
  useEffect(() => {
    let alive = true;
    const check = () => call<NonNullable<RecorderState>>("recorder_status").then((r) => alive && setRecorder(r), () => alive && setRecorder({ connected: false }));
    check();
    const timer = window.setInterval(check, 3000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const go = useCallback((next: Route) => {
    window.location.hash = routeHash(next);
  }, []);

  const rescan = useCallback(async () => {
    setScanning(true);
    try {
      const result = await call<{ dir: string; exists: boolean; items: LibraryItem[] }>("library_scan");
      setItems(result.items);
      setLibraryDir(result.dir);
      setLibraryMissing(!result.exists);
      call<{ free: number; total: number }>("disk").then(setDisk, () => setDisk(null));
      return result.items.map(toClip);
    } catch (error) {
      toast(t("state.scanFailed", errorText(error)), "error");
      return [];
    } finally {
      setScanning(false);
    }
  }, [toast]);

  const trash = useCallback(
    async (clips: Clip[]) => {
      const result = await call<{ trashed: string[]; errors: string[] }>("trash", { paths: clips.map((c) => c.path) });
      const gone = new Set(result.trashed);
      setItems((list) => list.filter((i) => !gone.has(i.path)));
      if (result.errors.length > 0) toast(t("state.notDeleted", result.errors.join("; ")), "error");
      call<{ free: number; total: number }>("disk").then(setDisk, () => undefined);
    },
    [toast],
  );

  // First scan, then the automatic cleanup, once per start.
  useEffect(() => {
    void rescan().then(async (clips) => {
      if (cleaned.current || !settings.cleanup.enabled) return;
      cleaned.current = true;
      const plan = planCleanup(clips, settings.cleanup);
      if (plan.remove.length === 0) return;
      await trash(plan.remove);
      toast(tn("state.cleaned", plan.remove.length, formatBytes(plan.bytes)));
    });
    // Runs once on start, later changes of the rules apply on the next start or by hand.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveConfig = useCallback(
    async (next: Config) => {
      await call("config_save", { config: next });
      setConfig(next);
      setConfigError(null);
      // Tell a running recorder to read the file again. Hotkeys change at once there.
      call<{ connected: boolean; skipped?: boolean }>("recorder_reload").then(
        (r) => setRecorderReload(r.connected ? "sent" : r.skipped ? "none" : "offline"),
        () => setRecorderReload("offline"),
      );
      const fresh = await call<Env>("env");
      setEnv(fresh);
      if (fresh.clip_dir !== libraryDir) await rescan();
    },
    [libraryDir, rescan],
  );

  const saveSettings = useCallback(async (next: AppSettings) => {
    const clean = normalizeSettings(next);
    setSettings(clean);
    await call("settings_save", { settings: clean });
    setEnv(await call<Env>("env"));
  }, []);

  const updateMeta = useCallback(async (clip: Clip, patch: Partial<ClipMeta>) => {
    const base = clip.item.meta ?? (await metaFromProbe(clip.item));
    const meta = { ...base, ...patch };
    await call("meta_write", { path: clip.path, meta });
    setItems((list) => list.map((i) => (i.path === clip.path ? { ...i, meta, meta_error: null } : i)));
  }, []);

  const rename = useCallback(async (clip: Clip, name: string) => {
    const result = await call<{ path: string }>("rename", { path: clip.path, name });
    setItems((list) => list.map((i) => (i.path === clip.path ? { ...i, path: result.path, name: name.trim() } : i)));
    return result.path;
  }, []);

  const clips = useMemo(() => items.map(toClip), [items]);

  const value: AppState = {
    env, config, configError, settings, lang, saveConfig, recorderReload, recorder, saveSettings, clips, libraryDir, libraryMissing, scanning,
    rescan, updateMeta, trash, rename, disk, route, go, filters, setFilters,
  };
  return <Context.Provider value={value}>{props.children}</Context.Provider>;
}
