// Settings. Every control here writes a real value: either to config.json, which the
// recorder reads, or to app.json, which only the app uses. Things the recorder cannot
// do yet have no page. The rules of the auto clips live in a file of their own, autoclip.json.

import { useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { AlertTriangle, AudioLines, Cloud, Download, Gamepad2, HardDrive, Keyboard, Languages, Layers, Moon, Palette, Plus, Share, Sparkles, Sun, Trash2, Upload, Video, X } from "lucide-react";
import { call, errorText, isTauri, pickFile, pickSaveFile } from "../lib/api";
import { forgetEncoderTrials, tryAllEncoders } from "../lib/export";
import type { EncoderTrial } from "../lib/export";
import { formatBytes } from "../lib/format";
import { actionLabel, checkHotkeys, comboFromEvent, keyCaps, withAllActions } from "../lib/hotkeys";
import { LANGUAGES, t, tn } from "../lib/i18n";
import type { Key } from "../lib/i18n";
import { eventLabel, planCleanup } from "../lib/library";
import { ACCENT_PRESETS, applyTheme, cleanAccent, CLOUD_READY, DEFAULT_ACCENT, normalizeSettings } from "../lib/settings";
import type { AutoClipRule, AutoClipRules, CloudMode, CloudSettings, Config, Corner, HotkeyAction, HotkeyBinding, MonitorChoice, VideoCodec } from "../lib/types";
import { useApp } from "../state";
import { AdvancedContext, Button, Card, Choice, Modal, NumberField, PathField, Row, Select, Toggle, useToast } from "../ui";
import { Shell } from "./Shell";

const PAGES: { id: string; label: Key; icon: typeof Video }[] = [
  { id: "recording", label: "set.recording", icon: Video },
  { id: "audio", label: "set.audio", icon: AudioLines },
  { id: "overlay", label: "set.overlay", icon: Layers },
  { id: "input", label: "set.input", icon: Gamepad2 },
  { id: "autoclips", label: "set.autoclips", icon: Sparkles },
  { id: "hotkeys", label: "set.hotkeys", icon: Keyboard },
  { id: "storage", label: "set.storage", icon: HardDrive },
  { id: "export", label: "set.export", icon: Share },
  { id: "appearance", label: "set.appearance", icon: Palette },
  { id: "language", label: "set.language", icon: Languages },
  { id: "cloud", label: "set.cloud", icon: Cloud },
  { id: "backup", label: "set.backup", icon: Download },
];

/** A longer remark. Only warnings show all the time, the rest with "More options". */
function Note(props: { children: ReactNode; warn?: boolean }) {
  const advanced = useContext(AdvancedContext);
  if (!props.warn && !advanced) return null;
  return (
    <div className={`note${props.warn ? " note-warn" : ""}`}>
      <span className="lbl">{props.warn ? t("common.warning") : t("common.note")}</span>
      <div>{props.children}</div>
    </div>
  );
}

/** The introduction of a page. Shown with "More options", the pages speak for themselves. */
function Lead(props: { children: ReactNode }) {
  const advanced = useContext(AdvancedContext);
  return advanced ? <p className="settings-lead">{props.children}</p> : null;
}

function useConfig() {
  const { config, saveConfig } = useApp();
  const toast = useToast();
  const set = (patch: Partial<Config>) => {
    saveConfig({ ...config, ...patch }).catch((e) => toast(t("set.notSaved", errorText(e)), "error"));
  };
  return { config, set };
}

function RestartNote() {
  const { recorderReload } = useApp();
  return (
    <Note>
      {t("set.restartNote")} {recorderReload === "sent" ? t("set.reloadSent") : recorderReload === "offline" ? t("set.reloadOffline") : ""}
    </Note>
  );
}

function RecordingPage() {
  const { config, set } = useConfig();
  const minutes = Math.floor(config.buffer_seconds / 60);
  // What the RAM buffer costs: bitrate times length.
  const bufferMb = (config.bitrate_kbps * config.buffer_seconds) / 8 / 1024;
  return (
    <>
      <RestartNote />
      <Card title={t("rec.buffer")}>
        <Row label={t("rec.bufferMode")} help={t("rec.bufferModeHelp")}>
          <Select<Config["buffer_mode"]> label={t("rec.bufferMode")} value={config.buffer_mode} onChange={(buffer_mode) => set({ buffer_mode })} options={[{ value: "game_only", label: t("rec.modeGame") }, { value: "always", label: t("rec.modeAlways") }]} />
        </Row>
        <Row label={t("rec.monitor")} help={t("rec.monitorHelp")}>
          <MonitorSelect value={config.monitor ?? { type: "primary" }} onChange={(monitor) => set({ monitor })} />
        </Row>
        <Row label={t("rec.length")} help={t("rec.lengthHelp")}>
          <input type="range" min={15} max={1200} step={15} value={config.buffer_seconds} aria-label={t("rec.bufferLength")} onChange={(e) => set({ buffer_seconds: Number(e.target.value) })} style={{ width: 220 }} />
          <span className="mono value">
            {minutes > 0 ? `${minutes} min ` : ""}
            {config.buffer_seconds % 60 > 0 || minutes === 0 ? `${config.buffer_seconds % 60} s` : ""}
          </span>
        </Row>
        <Row advanced label={t("rec.onDisk")} help={t("rec.onDiskHelp", formatBytes(bufferMb * 1024 * 1024))}>
          <Toggle checked={config.buffer_on_disk} label={t("rec.onDisk")} onChange={(buffer_on_disk) => set({ buffer_on_disk })} />
        </Row>
        <Row advanced label={t("rec.maxRecording")} help={t("rec.maxRecordingHelp")}>
          <NumberField label={t("rec.maxRecordingLabel")} value={config.max_recording_minutes} min={0} max={1440} unit="min" onChange={(max_recording_minutes) => set({ max_recording_minutes })} />
        </Row>
      </Card>
      <Card title={t("rec.picture")}>
        <Row label={t("rec.resolution")} help={t("rec.resolutionHelp")}>
          <Select<number>
            label={t("rec.resolution")}
            value={config.height ?? 0}
            onChange={(h) => set({ height: h === 0 ? null : h })}
            options={[{ value: 0, label: t("rec.likeSource") }, ...[2160, 1440, 1080, 720, 480].map((h) => ({ value: h, label: `${h}p` }))]}
          />
        </Row>
        <Row label={t("rec.fps")}>
          <Select<number> label={t("rec.fps")} value={config.fps} onChange={(fps) => set({ fps })} options={[...new Set([30, 60, 120, 144, 240, config.fps])].sort((a, b) => a - b).map((f) => ({ value: f, label: `${f} fps` }))} />
        </Row>
        <Row advanced label={t("rec.bitrate")} help={t("rec.bitrateHelp")}>
          <NumberField label={t("rec.bitrateLabel")} value={config.bitrate_kbps} min={500} max={200000} unit="kbit/s" onChange={(bitrate_kbps) => set({ bitrate_kbps })} />
        </Row>
        <Row advanced label={t("rec.codec")} help={t("rec.codecHelp")}>
          <Select<VideoCodec> label={t("rec.codec")} value={config.codec} onChange={(codec) => set({ codec })} options={[{ value: "h264", label: "H.264" }, { value: "hevc", label: "HEVC" }, { value: "av1", label: "AV1" }]} />
        </Row>
        <Row advanced label={t("rec.cursor")}>
          <Toggle checked={config.capture_cursor} label={t("rec.cursor")} onChange={(capture_cursor) => set({ capture_cursor })} />
        </Row>
        <Row advanced stacked label={t("rec.region")} help={t("rec.regionHelp")}>
          <RegionField value={config.region} onChange={(region) => set({ region })} />
        </Row>
      </Card>
      <CameraCard value={config.camera ?? DEFAULT_CAMERA} onChange={(camera) => set({ camera })} />
    </>
  );
}

const REGION_PARTS: Key[] = ["rec.regionLeft", "rec.regionTop", "rec.regionWidth", "rec.regionHeight"];

/** The four numbers of the region. They are saved together once all are valid, and as
 *  "no region" once all are empty. */
function RegionField(props: { value: Config["region"]; onChange: (region: Config["region"]) => void }) {
  const saved = props.value ? props.value.map(String) : ["", "", "", ""];
  const key = saved.join(",");
  const [text, setText] = useState(saved);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => setText(saved), [key]);
  const filled = text.filter((x) => x.trim() !== "").length;
  const numbers = text.map((x) => (/^\d{1,5}$/.test(x.trim()) ? Number(x.trim()) : NaN));
  // The recorder ignores a region with an edge below 64 pixels.
  const valid = numbers.every((n) => !Number.isNaN(n)) && numbers[2] >= 64 && numbers[3] >= 64;
  const commit = () => {
    if (filled === 0) return void (props.value !== null && props.onChange(null));
    if (valid && numbers.join(",") !== key) props.onChange(numbers as [number, number, number, number]);
  };
  return (
    <div className="region-field">
      {REGION_PARTS.map((part, i) => (
        <label key={part}>
          <span className="lbl">{t(part)}</span>
          <input
            className="input mono"
            inputMode="numeric"
            aria-label={`${t("rec.region")} ${t(part)}`}
            value={text[i]}
            onChange={(e) => setText(text.map((x, n) => (n === i ? e.target.value : x)))}
            onBlur={commit}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
        </label>
      ))}
      <Button disabled={props.value === null && filled === 0} onClick={() => (props.value === null ? setText(["", "", "", ""]) : props.onChange(null))}>{t("rec.wholeSource")}</Button>
      <span className="small muted region-state">{filled === 0 ? t("rec.wholeSource") : valid ? (numbers.join(",") === key ? t("common.saved") : "") : t("rec.regionInvalid")}</span>
    </div>
  );
}

function AudioPage() {
  const { config, set } = useConfig();
  const audio = config.audio;
  const all = audio.mode !== "selected";
  const setAudio = (patch: Partial<Config["audio"]>) => set({ audio: { ...audio, ...patch } });
  const setApp = (index: number, patch: Partial<Config["audio"]["apps"][number]>) => setAudio({ apps: audio.apps.map((a, i) => (i === index ? { ...a, ...patch } : a)) });
  return (
    <>
      <Lead>{t("audio.lead")}</Lead>
      <RestartNote />
      <Card title={t("audio.tracks")}>
        <Row label={t("audio.record")} help={t("audio.recordHelp")}>
          <Toggle checked={audio.enabled} label={t("audio.record")} onChange={(enabled) => setAudio({ enabled })} />
        </Row>
        <Row label={t("audio.mic")} help={t("audio.micHelp")}>
          <Toggle checked={audio.microphone} label={t("audio.mic")} disabled={!audio.enabled} onChange={(microphone) => setAudio({ microphone })} />
        </Row>
        <Row label={t("audio.mode")} help={all ? t("audio.modeAllHelp") : t("audio.modeSelectedHelp")}>
          <Choice<Config["audio"]["mode"]>
            label={t("audio.mode")}
            value={all ? "all" : "selected"}
            disabled={!audio.enabled}
            onChange={(mode) => setAudio({ mode })}
            options={[
              { value: "all", label: t("audio.modeAll") },
              { value: "selected", label: t("audio.modeSelected") },
            ]}
          />
        </Row>
      </Card>
      <Card title={all ? t("audio.appsAll") : t("audio.appsSelected")} aside={<Button small icon={<Plus size={14} />} onClick={() => setAudio({ apps: [...audio.apps, { name: t("audio.newTrack"), exe: [] }] })}>{t("audio.addTrack")}</Button>}>
        {audio.apps.length === 0 ? <p className="muted group-empty">{all ? t("audio.emptyAll") : t("audio.emptySelected")}</p> : null}
        {audio.apps.map((app, i) => (
          <div key={i} className="app-track">
            <TextCommit label={t("audio.trackName")} value={app.name} onCommit={(name) => setApp(i, { name })} />
            <TextCommit
              label={t("audio.programs")}
              mono
              value={app.exe.join(", ")}
              placeholder="discord.exe, teamspeak.exe"
              onCommit={(text) => setApp(i, { exe: text.split(/[,;\s]+/).map((e) => e.trim().toLowerCase()).filter(Boolean) })}
            />
            <Button kind="ghost" icon={<Trash2 size={15} />} title={t("audio.removeTrack")} onClick={() => setAudio({ apps: audio.apps.filter((_, x) => x !== i) })} />
          </div>
        ))}
      </Card>
      <Note>{t("audio.note")}</Note>
    </>
  );
}

/** Text field that saves on blur or Enter, not on every key. */
function TextCommit(props: { value: string; label: string; placeholder?: string; mono?: boolean; onCommit: (value: string) => void }) {
  const [text, setText] = useState(props.value);
  useEffect(() => setText(props.value), [props.value]);
  return (
    <input
      className={`input${props.mono ? " mono" : ""}`}
      aria-label={props.label}
      value={text}
      placeholder={props.placeholder}
      spellCheck={false}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => text.trim() !== props.value && props.onCommit(text.trim())}
      onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
    />
  );
}

/** The recorder reads the overlay values at its start only, the page says so. */
function OverlayPage() {
  const { config, set } = useConfig();
  const overlay = config.overlay;
  const setOverlay = (patch: Partial<Config["overlay"]>) => set({ overlay: { ...overlay, ...patch } });
  const off = !overlay.enabled;
  return (
    <>
      <Lead>{t("ov.lead")}</Lead>
      <Note>{t("ov.startNote")}</Note>
      <Card title={t("ov.inGame")}>
        <Row label={t("ov.enable")} help={t("ov.enableHelp")}>
          <Toggle checked={overlay.enabled} label={t("ov.enable")} onChange={(enabled) => setOverlay({ enabled })} />
        </Row>
        <Row advanced label={t("ov.indicator")} help={t("ov.indicatorHelp")}>
          <Toggle checked={overlay.indicator} label={t("ov.indicator")} disabled={off} onChange={(indicator) => setOverlay({ indicator })} />
        </Row>
        <Row label={t("ov.toasts")} help={t("ov.toastsHelp")}>
          <Toggle checked={overlay.toasts} label={t("ov.toasts")} disabled={off} onChange={(toasts) => setOverlay({ toasts })} />
        </Row>
      </Card>
      <Card title={t("ov.sound")}>
        <Row label={t("ov.saveSound")} help={t("ov.saveSoundHelp")}>
          <Toggle checked={overlay.sound} label={t("ov.saveSound")} disabled={off} onChange={(sound) => setOverlay({ sound })} />
        </Row>
        <Row label={t("ov.volume")}>
          <input
            type="range"
            min={0}
            max={100}
            step={5}
            value={Math.round(overlay.sound_volume * 100)}
            aria-label={t("ov.volumeLabel")}
            disabled={off || !overlay.sound}
            onChange={(e) => setOverlay({ sound_volume: Number(e.target.value) / 100 })}
            style={{ width: 220 }}
          />
          <span className="mono value">{Math.round(overlay.sound_volume * 100)} %</span>
        </Row>
      </Card>
      <SoundsCard volume={overlay.sound_volume} />
      <Note>{t("ov.panelNote", actionLabel({ type: "overlay" }))}</Note>
    </>
  );
}

/** On or off, which camera, and where it sits by default. */
function CameraCard(props: { value: NonNullable<Config["camera"]>; onChange: (value: NonNullable<Config["camera"]>) => void }) {
  const { recorder } = useApp();
  const cam = props.value;
  const set = (patch: Partial<NonNullable<Config["camera"]>>) => props.onChange({ ...cam, ...patch });
  const known = recorder?.status?.cameras ?? [];
  // A chosen camera stays in the list while it is unplugged or the recorder is off.
  const names = cam.device && !known.includes(cam.device) ? [...known, cam.device] : known;
  return (
    <Card title={t("cam.title")}>
      <Row label={t("cam.enabled")} help={t("cam.enabledHelp")}>
        <Toggle checked={cam.enabled} label={t("cam.enabled")} onChange={(enabled) => set({ enabled })} />
      </Row>
      <Row label={t("cam.device")} info={recorder?.status?.camera ? t("cam.running", recorder.status.camera) : known.length === 0 && recorder?.connected ? t("cam.none") : undefined}>
        <Select
          label={t("cam.device")}
          value={cam.device ?? ""}
          disabled={!cam.enabled}
          onChange={(device) => set({ device: device || null })}
          options={[{ value: "", label: t("cam.first") }, ...names.map((n) => ({ value: n, label: n }))]}
        />
      </Row>
      <Row label={t("cam.corner")}>
        <Select<Corner>
          label={t("cam.corner")}
          value={cam.corner}
          disabled={!cam.enabled}
          onChange={(corner) => set({ corner })}
          options={[
            { value: "top_left", label: t("in.cornerTopLeft") },
            { value: "top_right", label: t("in.cornerTopRight") },
            { value: "bottom_left", label: t("in.cornerBottomLeft") },
            { value: "bottom_right", label: t("in.cornerBottomRight") },
          ]}
        />
      </Row>
      <Row label={t("cam.size")}>
        <input type="range" min={10} max={60} step={1} value={cam.size_percent} disabled={!cam.enabled} aria-label={t("cam.size")} onChange={(e) => set({ size_percent: Number(e.target.value) })} style={{ width: 220 }} />
        <span className="mono value">{cam.size_percent} %</span>
      </Row>
    </Card>
  );
}

const DEFAULT_CAMERA: NonNullable<Config["camera"]> = { enabled: false, device: null, corner: "bottom_right", size_percent: 25 };

/** Main monitor, the monitor of the game, or one monitor out of the recorder's list. */
function MonitorSelect(props: { value: MonitorChoice; onChange: (value: MonitorChoice) => void }) {
  const { recorder } = useApp();
  const known = recorder?.status?.monitors ?? [];
  const chosen = props.value.type === "index" ? props.value.index : -1;
  // A chosen monitor stays in the list while the recorder is off or the monitor unplugged.
  const count = Math.max(known.length, chosen + 1);
  const single = Array.from({ length: count }, (_, i) => {
    const size = known[i] ? (i === 0 ? t("rec.monMain", known[i]) : known[i]) : null;
    return { value: `i${i}`, label: size ? t("rec.monIndex", i + 1, size) : t("rec.monIndexPlain", i + 1) };
  });
  const value = props.value.type === "index" ? `i${props.value.index}` : props.value.type;
  return (
    <Select
      label={t("rec.monitor")}
      value={value}
      onChange={(v) => props.onChange(v === "primary" || v === "game" || v === "all" ? { type: v } : { type: "index", index: Number(v.slice(1)) })}
      options={[{ value: "primary", label: t("rec.monPrimary") }, { value: "game", label: t("rec.monGame") }, { value: "all", label: t("rec.monAll") }, ...single]}
    />
  );
}

const SOUND_NAMES = ["clip", "auto", "rec_start", "rec_stop", "shot", "tick"] as const;

/** One row per notice sound: listen, replace with an own WAV file, back to the built in one. */
function SoundsCard(props: { volume: number }) {
  const toast = useToast();
  const [custom, setCustom] = useState<Record<string, boolean>>({});
  const [dir, setDir] = useState("");
  const load = () =>
    call<{ dir: string; sounds: { name: string; custom: boolean }[] }>("sounds_list").then(
      (r) => {
        setDir(r.dir);
        setCustom(Object.fromEntries(r.sounds.map((x) => [x.name, x.custom])));
      },
      (e) => toast(errorText(e), "error"),
    );
  useEffect(() => {
    void load();
  }, []);
  const play = async (name: string) => {
    try {
      const data = await call<{ base64: string }>("sound_data", { name });
      const audio = new Audio("data:audio/wav;base64," + data.base64);
      // The files are quiet on purpose. The preview follows the volume of the setting.
      audio.volume = Math.min(1, Math.max(0.05, props.volume));
      await audio.play();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const choose = async (name: string) => {
    // The native dialog only exists in the real app, the browser preview asks for a path.
    const path = isTauri ? await pickFile(dir || undefined, "WAV", ["wav"]) : window.prompt(t("ov.sndTyped"));
    if (!path) return;
    try {
      await call("sound_set", { name, path });
      await load();
      void play(name);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const reset = (name: string) => call("sound_reset", { name }).then(load, (e) => toast(errorText(e), "error"));
  return (
    <Card title={t("ov.sounds")}>
      <Lead>{t("ov.soundsHelp")}</Lead>
      {SOUND_NAMES.map((name) => (
        <Row key={name} label={t(("ov.snd." + name) as Key)} info={custom[name] ? t("ov.sndOwn") : t("ov.sndBuiltIn")}>
          <Button small onClick={() => void play(name)}>{t("ov.sndPlay")}</Button>
          <Button small onClick={() => void choose(name)}>{t("ov.sndChoose")}</Button>
          {custom[name] ? <Button small kind="ghost" onClick={() => void reset(name)}>{t("ov.sndReset")}</Button> : null}
        </Row>
      ))}
      {dir ? (
        <Row advanced label={t("ov.sndFolder")} info={dir}>
          <Button small kind="ghost" onClick={() => void call("reveal", { path: dir }).catch((e) => toast(errorText(e), "error"))}>{t("ov.sndFolder")}</Button>
        </Row>
      ) : null}
    </Card>
  );
}

const DEFAULT_CLOUD: CloudSettings = { mode: "off", server: "https://imkirit.dev/klip/cloud", keep_local: true, since: null };

/** Klip Cloud: the account on a server, and which clips go there. */
function CloudPage() {
  const { config, set } = useConfig();
  const { recorder } = useApp();
  const toast = useToast();
  const cloud = { ...DEFAULT_CLOUD, ...(config.cloud ?? {}) };
  const setCloud = (patch: Partial<CloudSettings>) => set({ cloud: { ...cloud, ...patch } });
  const [account, setAccount] = useState<{ server: string; name: string } | null>(null);
  const [linking, setLinking] = useState<{ code: string } | null>(null);
  const [server, setServer] = useState(cloud.server);
  const load = () => call<{ account: { server: string; name: string } | null }>("cloud_account").then((r) => setAccount(r.account), () => setAccount(null));
  useEffect(() => {
    void load();
  }, []);
  const signedIn = account !== null && account.server.replace(/\/+$/, "") === cloud.server.replace(/\/+$/, "");

  const signIn = async () => {
    try {
      const link = await call<{ device_code: string; user_code: string; verify_url: string; interval: number; expires_in: number }>("cloud_link_start", { server: cloud.server });
      setLinking({ code: link.user_code });
      await call("open_url", { url: link.verify_url }).catch(() => window.open(link.verify_url, "_blank"));
      const until = Date.now() + link.expires_in * 1000;
      while (Date.now() < until) {
        await new Promise((r) => setTimeout(r, Math.max(2, link.interval) * 1000));
        const poll = await call<{ state: "pending" | "done"; name?: string }>("cloud_link_poll", { server: cloud.server, device_code: link.device_code });
        if (poll.state === "done") {
          toast(t("cl.signedIn", poll.name ?? ""));
          break;
        }
      }
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setLinking(null);
      void load();
    }
  };

  return (
    <>
      <Lead>{t("cl.lead")}</Lead>
      {CLOUD_READY ? null : (
        <div className="note soon-note">
          <span className="soon-badge">{t("cl.soon")}</span>
          <div>{t("cl.soonNote")}</div>
        </div>
      )}
      <fieldset className={`cloud-fields${CLOUD_READY ? "" : " is-soon"}`} disabled={!CLOUD_READY} aria-disabled={!CLOUD_READY}>
      <Card title={t("cl.account")}>
        <Row label={t("cl.server")} help={t("cl.serverHelp")}>
          <input className="input" style={{ width: 300 }} value={server} onChange={(e) => setServer(e.target.value)} onBlur={() => server.trim() && server.trim() !== cloud.server && setCloud({ server: server.trim().replace(/\/+$/, "") })} />
        </Row>
        <Row label={signedIn ? t("cl.signedIn", account?.name ?? "") : t("cl.signedOut")} info={linking ? t("cl.waiting", linking.code) : undefined}>
          {signedIn ? (
            <Button small onClick={() => void call("cloud_sign_out").then(load)}>{t("cl.signOut")}</Button>
          ) : (
            <Button small kind="primary" disabled={linking !== null} onClick={() => void signIn()}>{t("cl.signIn")}</Button>
          )}
        </Row>
      </Card>
      <Card title={t("cl.what")}>
        <Row label={t("cl.mode")} help={t("cl.modeHelp")}>
          <Choice<CloudMode>
            label={t("cl.mode")}
            value={cloud.mode}
            onChange={(mode) => setCloud({ mode, since: mode === "all" && cloud.mode !== "all" ? new Date().toISOString() : cloud.since })}
            options={[
              { value: "off", label: t("cl.modeOff") },
              { value: "selected", label: t("cl.modeSelected") },
              { value: "all", label: t("cl.modeAll") },
            ]}
          />
        </Row>
        <Row label={t("cl.keep")} help={t("cl.keepHelp")}>
          <Toggle checked={cloud.keep_local} label={t("cl.keep")} disabled={cloud.mode === "off"} onChange={(keep_local) => setCloud({ keep_local })} />
        </Row>
        <Row label={t("cl.where")} help={t("cl.whereHelp")} info={recorder?.status?.cloud_uploaded ? t("cl.uploaded", recorder.status.cloud_uploaded) : undefined}>
          <Button small onClick={() => void call("open_url", { url: `${cloud.server}/settings` })}>{t("cl.open")}</Button>
        </Row>
      </Card>
      </fieldset>
      {recorder?.status?.cloud_error ? <Note warn>{t("cl.error", recorder.status.cloud_error)}</Note> : null}
    </>
  );
}

function InputPage() {
  const { config, set } = useConfig();
  const input = config.input;
  const setInput = (patch: Partial<Config["input"]>) => set({ input: { ...input, ...patch } });
  return (
    <>
      <Lead>{t("in.lead")}</Lead>
      <Note>{t("in.privacy")}</Note>
      <Card title={t("in.track")}>
        <Row advanced label={t("in.capture")} help={t("in.captureHelp")}>
          <Toggle checked={input.enabled} label={t("in.capture")} onChange={(enabled) => setInput({ enabled })} />
        </Row>
      </Card>
      <Card title={t("in.inClips")}>
        <Row label={t("in.showInClips")} help={t("in.showInClipsHelp")}>
          <Toggle checked={input.show_in_clips !== false} label={t("in.showInClips")} disabled={!input.enabled} onChange={(show_in_clips) => setInput({ show_in_clips })} />
        </Row>
        <Row label={t("in.corner")} help={t("in.cornerHelp")}>
          <Select<Corner>
            label={t("in.corner")}
            value={input.corner ?? "top_right"}
            disabled={!input.enabled}
            onChange={(corner) => setInput({ corner })}
            options={[
              { value: "top_left", label: t("in.cornerTopLeft") },
              { value: "top_right", label: t("in.cornerTopRight") },
              { value: "bottom_left", label: t("in.cornerBottomLeft") },
              { value: "bottom_right", label: t("in.cornerBottomRight") },
            ]}
          />
        </Row>
      </Card>
      <Note>{t("in.pauseNote", actionLabel({ type: "input_pause" }))}</Note>
    </>
  );
}

/** Events the Valorant detector really reports, see `SUPPORTED` in
 *  crates/klip-detect/src/valorant/mod.rs. Must stay in step with it. */
const AUTOCLIP_EVENTS = ["kill", "double_kill", "triple_kill", "quadra_kill", "ace", "death", "round_won", "round_lost"];

interface AutoClipFile {
  rules: AutoClipRules;
  exists: boolean;
  error: string | null;
  path: string;
}

function AutoClipsPage() {
  const { config, set } = useConfig();
  const toast = useToast();
  const [file, setFile] = useState<AutoClipFile | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    call<AutoClipFile>("autoclip_load").then(setFile, (e) => setFailure(errorText(e)));
  }, []);

  const save = (rules: AutoClipRules) => {
    if (!file) return;
    setFile({ ...file, rules, exists: true, error: null });
    call("autoclip_save", { rules }).catch((e) => toast(t("set.notSaved", errorText(e)), "error"));
  };
  const off = !config.auto_clips;

  return (
    <>
      <Lead>{t("ac.lead")}</Lead>
      <Note warn>{t("ac.warning")}</Note>
      <Card>
        <Row label={t("set.autoclips")} help={t("ac.enableHelp")}>
          <Toggle checked={config.auto_clips} label={t("set.autoclips")} onChange={(auto_clips) => set({ auto_clips })} />
        </Row>
      </Card>
      {failure ? <Note warn>{t("ac.loadFailed", failure)}</Note> : null}
      {file?.error ? (
        <Note warn>
          {t("ac.fileBroken")} <span className="mono small">{file.error}</span>
        </Note>
      ) : null}
      {file ? (
        <>
          <Card title={t("ac.events")}>
            <div className="rule-row rule-head">
              <span>{t("ac.event")}</span>
              <span>{t("ac.before")}</span>
              <span>{t("ac.after")}</span>
              <span>{t("ac.clip")}</span>
            </div>
            {AUTOCLIP_EVENTS.map((id) => {
              const label = eventLabel(id);
              // An event without an entry counts as switched off in the recorder.
              const rule: AutoClipRule = file.rules.rules[id] ?? { enabled: false, before_s: 10, after_s: 5 };
              const setRule = (patch: Partial<AutoClipRule>) => save({ ...file.rules, rules: { ...file.rules.rules, [id]: { ...rule, ...patch } } });
              return (
                <div key={id} className="rule-row">
                  <span className="rule-name">{label}</span>
                  <NumberField label={t("ac.beforeLabel", label)} value={rule.before_s} min={0} max={600} unit="s" disabled={off} onChange={(before_s) => setRule({ before_s })} />
                  <NumberField label={t("ac.afterLabel", label)} value={rule.after_s} min={0} max={600} unit="s" disabled={off} onChange={(after_s) => setRule({ after_s })} />
                  <Toggle checked={rule.enabled} label={t("ac.saveLabel", label)} disabled={off} onChange={(enabled) => setRule({ enabled })} />
                </div>
              );
            })}
          </Card>
          <Note>{t("ac.multiNote")}</Note>
          <Card advanced title={t("ac.build")}>
            <Row label={t("ac.merge")} help={t("ac.mergeHelp")}>
              <NumberField label={t("ac.mergeLabel")} value={file.rules.merge_gap_s} min={0} max={600} unit="s" disabled={off} onChange={(merge_gap_s) => save({ ...file.rules, merge_gap_s })} />
            </Row>
            <Row label={t("ac.maxClip")} help={t("ac.maxClipHelp")}>
              <NumberField label={t("ac.maxClipLabel")} value={file.rules.max_clip_s} min={0} max={3600} unit="s" disabled={off} onChange={(max_clip_s) => save({ ...file.rules, max_clip_s })} />
            </Row>
            <Row label={t("ac.confidence")} help={t("ac.confidenceHelp")}>
              <NumberField label={t("ac.confidenceLabel")} value={Math.round(file.rules.min_confidence * 100)} min={0} max={100} unit="%" disabled={off} onChange={(percent) => save({ ...file.rules, min_confidence: Math.round(percent) / 100 })} />
            </Row>
          </Card>
          <Note>
            {file.exists ? t("ac.fileAt") : t("ac.fileMissing")} <span className="mono small">{file.path}</span>. {t("ac.fileRead")}
          </Note>
        </>
      ) : null}
    </>
  );
}

function HotkeysPage() {
  const { config, set } = useConfig();
  const [capturing, setCapturing] = useState<number | null>(null);
  const [rejected, setRejected] = useState<string | null>(null);
  // An older config.json lacks the actions that came later, they show up unassigned.
  const hotkeys = useMemo(() => withAllActions(config.hotkeys), [config.hotkeys]);
  const problems = useMemo(() => checkHotkeys(hotkeys), [hotkeys]);
  const setBinding = (index: number, patch: Partial<HotkeyBinding>) => set({ hotkeys: hotkeys.map((b, i) => (i === index ? { ...b, ...patch } : b)) });

  // While a field is armed, the next combination goes into it. Escape leaves it as it was.
  useEffect(() => {
    if (capturing === null) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") return setCapturing(null);
      const combo = comboFromEvent(e);
      if (combo.unsupported) return setRejected(t("hk.unsupported", combo.unsupported));
      if (!combo.keys) return;
      setRejected(null);
      setBinding(capturing, { keys: combo.keys });
      setCapturing(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
    // setBinding changes with every render, the listener only depends on the armed field.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [capturing, config.hotkeys]);

  const addClip = () => set({ hotkeys: [...hotkeys, { keys: "", action: { type: "clip", seconds: 120 } }] });
  const conflicts = problems.flat().filter((p) => p.level === "error").length;

  return (
    <>
      <Lead>{t("hk.lead")}</Lead>
      {conflicts > 0 ? (
        <Note warn>
          {tn("hk.conflicts", conflicts)} {t("hk.conflictsRule")}
        </Note>
      ) : null}
      <Card title={t("hk.bindings")} aside={<Button small icon={<Plus size={14} />} onClick={addClip}>{t("hk.addClip")}</Button>}>
        {hotkeys.map((binding, i) => {
          const action = binding.action;
          const isClip = action.type === "clip";
          const errors = problems[i];
          return (
            <div key={i} className="hotkey-row">
              <div className="hotkey-action">
                {isClip ? (
                  <>
                    <span>{t("hk.clipLast")}</span>
                    <NumberField label={t("hk.clipSeconds")} value={action.seconds} min={5} max={1200} unit="s" onChange={(seconds) => setBinding(i, { action: { type: "clip", seconds } as HotkeyAction })} />
                  </>
                ) : (
                  <span>{actionLabel(action)}</span>
                )}
              </div>
              <button type="button" className={`hotkey-keys${capturing === i ? " is-capturing" : ""}${errors.length > 0 ? " has-error" : ""}`} onClick={() => setCapturing(capturing === i ? null : i)} aria-label={t("hk.keyFor", actionLabel(action))}>
                {capturing === i ? (
                  <span>{t("hk.press")}</span>
                ) : binding.keys ? (
                  keyCaps(binding.keys).map((cap, k) => (
                    <span key={k} className="keycap">
                      {cap}
                    </span>
                  ))
                ) : (
                  <span className="muted">{t("hk.unassigned")}</span>
                )}
              </button>
              <span className="hotkey-buttons">
                <Button kind="ghost" icon={<X size={15} />} title={t("hk.clear")} disabled={!binding.keys} onClick={() => setBinding(i, { keys: "" })} />
                {isClip && hotkeys.filter((b) => b.action.type === "clip").length > 1 ? (
                  <Button kind="ghost" icon={<Trash2 size={15} />} title={t("hk.removeClip")} onClick={() => set({ hotkeys: hotkeys.filter((_, x) => x !== i) })} />
                ) : null}
              </span>
              {errors.map((p, k) => (
                <div key={k} className={`hotkey-problem${p.level === "warning" ? " is-soft" : ""}`}>
                  <AlertTriangle size={14} />
                  <span>{p.text}</span>
                </div>
              ))}
            </div>
          );
        })}
      </Card>
      {rejected ? <Note warn>{rejected}</Note> : null}
      <Note>{t("hk.note")}</Note>
    </>
  );
}

type CacheMode = "default" | "folder" | "drive";

/** The default cache path on another drive: same path, other letter. Mirrors
 *  `Config::cache_dir` in klip-core. */
function onDrive(path: string, drive: string): string {
  const letter = drive.trim().charAt(0);
  return /^[a-zA-Z]$/.test(letter) && path.length > 2 && path[1] === ":" ? `${letter.toUpperCase()}${path.slice(1)}` : path;
}

function CacheCard() {
  const { config, set } = useConfig();
  const { env } = useApp();
  const toast = useToast();
  const saved: CacheMode = config.cache_dir ? "folder" : config.cache_drive ? "drive" : "default";
  const [mode, setMode] = useState<CacheMode>(saved);
  const [drives, setDrives] = useState<string[]>([]);
  const [bytes, setBytes] = useState<number | null>(null);
  const loadStats = () => call<{ bytes: number }>("cache_stats").then((r) => setBytes(r.bytes), () => setBytes(null));
  useEffect(() => {
    call<{ drives: string[] }>("drives").then((r) => setDrives(r.drives), () => setDrives([]));
  }, []);
  // Another place means another content, so the size is read again.
  useEffect(() => void loadStats(), [env.cache_dir]);

  const defaultDrive = env.default_cache_dir.slice(0, 2).toUpperCase();
  const drive = (config.cache_drive ?? defaultDrive).trim().slice(0, 2).toUpperCase();
  const choose = (next: CacheMode) => {
    setMode(next);
    if (next === "default" && saved !== "default") set({ cache_dir: null, cache_drive: null });
    if (next === "drive" && saved !== "drive") set({ cache_dir: null, cache_drive: drive });
    // A folder is only saved once a path is entered.
  };
  const options = [...new Set([...drives, drive])].sort();

  return (
    <Card advanced title={t("cache.title")}>
      <Row label={t("cache.location")} help={t("cache.locationHelp")}>
        <Choice<CacheMode>
          label={t("cache.location")}
          value={mode}
          onChange={choose}
          options={[
            { value: "default", label: t("cache.modeDefault") },
            { value: "folder", label: t("cache.modeFolder") },
            { value: "drive", label: t("cache.modeDrive") },
          ]}
        />
      </Row>
      {mode === "folder" ? (
        <Row stacked label={t("cache.folder")} help={config.cache_dir ? t("cache.folderHelp") : t("cache.folderEmpty")}>
          <PathField label={t("cache.folder")} value={config.cache_dir ?? ""} placeholder={env.default_cache_dir} pick="folder" onChange={(dir) => (dir === "" ? set({ cache_dir: null }) : set({ cache_dir: dir, cache_drive: null }))} />
        </Row>
      ) : null}
      {mode === "drive" ? (
        <Row label={t("cache.drive")} help={<>{t("cache.driveHelp")} <span className="mono small">{onDrive(env.default_cache_dir, drive)}</span></>}>
          <Select<string> label={t("cache.drive")} value={drive} onChange={(next) => set({ cache_dir: null, cache_drive: next })} options={options.map((d) => ({ value: d, label: d }))} />
        </Row>
      ) : null}
      <Row
        label={t("cache.content")}
        help={
          <>
            <span className="mono small">{env.cache_dir}</span>
            {env.cache_dir_from_env ? <> {t("cache.fromEnv")}</> : null}
          </>
        }
      >
        <span className="mono value">{bytes === null ? "" : formatBytes(bytes)}</span>
        <Button onClick={() => void call("cache_clear").then(loadStats).then(() => toast(t("cache.cleared")))}>{t("cache.clear")}</Button>
      </Row>
    </Card>
  );
}

function StoragePage() {
  const { config, set } = useConfig();
  const { env, settings, saveSettings, clips, trash, disk } = useApp();
  const toast = useToast();
  const [confirm, setConfirm] = useState(false);
  const used = clips.reduce((sum, c) => sum + c.item.size, 0);
  const plan = useMemo(() => planCleanup(clips, settings.cleanup), [clips, settings.cleanup]);
  const rules = settings.cleanup;
  const setRules = (patch: Partial<typeof rules>) => void saveSettings({ ...settings, cleanup: { ...rules, ...patch } });

  const run = async () => {
    setConfirm(false);
    const count = plan.remove.length;
    await trash(plan.remove);
    toast(tn("lib.trashed", count));
  };

  return (
    <>
      <Card title={t("st.clipFolder")}>
        <Row stacked label={t("st.location")} help={env.clip_dir_from_env ? t("st.locationEnv") : t("st.locationHelp")}>
          <PathField label={t("st.clipFolder")} value={config.clip_dir ?? ""} placeholder={env.default_clip_dir} pick="folder" onChange={(dir) => set({ clip_dir: dir === "" ? null : dir })} />
        </Row>
        <Row label={t("st.used")} info={disk ? t("hud.diskHelp", formatBytes(disk.free), formatBytes(disk.total)) : undefined}>
          <span className="mono">{tn("st.usedValue", clips.length, formatBytes(used))}</span>
        </Row>
        <Row advanced label={t("st.limit")} help={t("st.limitHelp")}>
          <NumberField label={t("st.limitLabel")} value={settings.storage_limit_gb} min={0} max={100000} unit="GB" onChange={(storage_limit_gb) => void saveSettings({ ...settings, storage_limit_gb })} />
        </Row>
      </Card>
      <Card advanced title={t("st.cleanup")}>
        <Row label={t("st.cleanupOnStart")} help={t("st.cleanupHelp")}>
          <Toggle checked={rules.enabled} label={t("st.cleanup")} onChange={(enabled) => setRules({ enabled })} />
        </Row>
        <Row label={t("st.olderThan")} help={t("st.ruleOff")}>
          <NumberField label={t("st.olderThanLabel")} value={rules.max_age_days} min={0} max={3650} unit={t("unit.days")} onChange={(max_age_days) => setRules({ max_age_days })} />
        </Row>
        <Row label={t("st.largerThan")} help={t("st.largerThanHelp")}>
          <NumberField label={t("st.largerThanLabel")} value={rules.max_size_gb} min={0} max={100000} step={0.1} unit="GB" onChange={(max_size_gb) => setRules({ max_size_gb })} />
        </Row>
        <Row label={t("st.now")} info={plan.remove.length === 0 ? t("st.nothing") : t("st.nowHelp", plan.remove.length, formatBytes(plan.bytes), clips.filter((c) => c.favorite).length)}>
          <Button icon={<Trash2 size={15} />} disabled={plan.remove.length === 0} onClick={() => setConfirm(true)}>{t("st.cleanNow")}</Button>
        </Row>
      </Card>
      <Note>{t("st.note")}</Note>
      <CacheCard />
      {confirm ? (
        <Modal
          title={tn("st.confirmTitle", plan.remove.length)}
          onClose={() => setConfirm(false)}
          footer={
            <>
              <Button kind="ghost" onClick={() => setConfirm(false)}>{t("common.cancel")}</Button>
              <Button kind="danger" icon={<Trash2 size={15} />} onClick={() => void run()}>{t("common.toTrash")}</Button>
            </>
          }
        >
          <p>{t("st.confirmText", formatBytes(plan.bytes))}</p>
          <table className="table">
            <tbody>
              {plan.remove.slice(0, 12).map((c) => (
                <tr key={c.path}>
                  <td className="mono">{c.title}</td>
                  <td>{plan.reason.get(c.path) === "age" ? t("st.reasonAge") : t("st.reasonSize")}</td>
                  <td className="mono num">{formatBytes(c.item.size)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {plan.remove.length > 12 ? <p className="muted small">{t("st.more", plan.remove.length - 12)}</p> : null}
        </Modal>
      ) : null}
    </>
  );
}

function ExportPage() {
  const { env, settings, saveSettings } = useApp();
  const [info, setInfo] = useState<{ path: string; ok: boolean; version: string } | null>(null);
  const [trials, setTrials] = useState<EncoderTrial[] | null>(null);
  const [testing, setTesting] = useState(false);
  useEffect(() => {
    setInfo(null);
    void call<{ path: string; ok: boolean; version: string }>("ffmpeg_info").then(setInfo);
  }, [settings.ffmpeg_path]);
  const test = async () => {
    setTesting(true);
    forgetEncoderTrials();
    setTrials(await tryAllEncoders());
    setTesting(false);
  };
  return (
    <>
      <Card title="ffmpeg">
        <Row stacked label={t("exs.path")} help={t("exs.pathHelp")}>
          <PathField
            label={t("exs.path")}
            value={settings.ffmpeg_path}
            placeholder={env.default_ffmpeg}
            pick={{ name: "ffmpeg", extensions: ["exe"] }}
            onChange={(ffmpeg_path) => {
              forgetEncoderTrials();
              setTrials(null);
              void saveSettings({ ...settings, ffmpeg_path });
            }}
          />
        </Row>
        <Row label={t("exs.found")}>
          <span className={`mono small found${info && !info.ok ? " error-text" : ""}`}>{info === null ? t("exs.checking") : info.ok ? info.version : t("exs.notStartable", info.version)}</span>
        </Row>
        <Row advanced stacked label={t("exs.dir")} help={t("exs.dirHelp")}>
          <PathField label={t("exs.dir")} value={settings.export_dir} placeholder={env.export_dir} pick="folder" onChange={(export_dir) => void saveSettings({ ...settings, export_dir })} />
        </Row>
      </Card>
      <Card advanced title={t("exs.encoders")} aside={<Button small disabled={testing} onClick={() => void test()}>{testing ? t("exs.running") : t("exs.startTrial")}</Button>}>
        <p className="muted group-text">{t("exs.encodersText")}</p>
        {trials ? (
          <table className="table">
            <thead>
              <tr>
                <th>{t("exs.encoder")}</th>
                <th>{t("exs.result")}</th>
                <th>{t("exs.time")}</th>
              </tr>
            </thead>
            <tbody>
              {trials.map((trial) => (
                <tr key={trial.encoder}>
                  <td className="mono">{trial.encoder}</td>
                  <td className={trial.ok ? "ok" : "muted"}>{trial.ok ? t("exs.works") : t("exs.fails", trial.message.slice(0, 110))}</td>
                  <td className="mono">{trial.ms} ms</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </Card>
    </>
  );
}

function AppearancePage() {
  const { settings, saveSettings } = useApp();
  const { config, set } = useConfig();
  const accent = cleanAccent(config.accent);
  // The colour field reports every step of a drag. The page follows at once, the file
  // is written once the colour has rested for a moment.
  const [draft, setDraft] = useState(accent);
  useEffect(() => setDraft(accent), [accent]);
  useEffect(() => {
    if (draft === accent) return;
    applyTheme(settings.theme, draft);
    const timer = window.setTimeout(() => set({ accent: draft }), 300);
    return () => window.clearTimeout(timer);
    // `set` changes with every render, the timer only depends on the colour.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft, accent]);
  return (
    <>
      <Card title={t("ap.accent")}>
        <Row label={t("ap.accentColor")} help={t("ap.accentHelp")}>
          <div className="swatches">
            {ACCENT_PRESETS.map((preset) => (
              <button key={preset.color} type="button" className={`swatch${draft === preset.color ? " is-on" : ""}`} aria-pressed={draft === preset.color} onClick={() => set({ accent: preset.color })}>
                <i style={{ background: preset.color }} />
                <span>{t(preset.name === "violet" ? "ap.violet" : "ap.blue")}</span>
                <span className="lbl">{preset.color}</span>
              </button>
            ))}
            <label className={`swatch swatch-free${ACCENT_PRESETS.some((p) => p.color === draft) ? "" : " is-on"}`}>
              <input type="color" className="color-input" aria-label={t("ap.custom")} value={draft} onChange={(e) => setDraft(e.target.value.toUpperCase())} />
              <span>{t("ap.custom")}</span>
              <span className="lbl">{draft}</span>
            </label>
          </div>
        </Row>
      </Card>
      <Card title={t("ap.mode")}>
        <Row advanced label={t("ap.mode")} help={t("ap.modeHelp")}>
          <Choice<"dark" | "light">
            label={t("ap.mode")}
            value={settings.theme}
            onChange={(theme) => void saveSettings({ ...settings, theme })}
            options={[
              { value: "dark", label: t("ap.dark"), icon: <Moon size={14} /> },
              { value: "light", label: t("ap.light"), icon: <Sun size={14} /> },
            ]}
          />
        </Row>
        <Row advanced label={t("ap.reset")}>
          <Button
            disabled={accent === DEFAULT_ACCENT && settings.theme === "dark"}
            onClick={() => {
              if (accent !== DEFAULT_ACCENT) set({ accent: DEFAULT_ACCENT });
              if (settings.theme !== "dark") void saveSettings({ ...settings, theme: "dark" });
            }}
          >
            {t("ap.default")}
          </Button>
        </Row>
      </Card>
    </>
  );
}

function LanguagePage() {
  const { lang } = useApp();
  const { set } = useConfig();
  return (
    <>
      <Lead>{t("lang.lead")}</Lead>
      <Card title={t("set.language")}>
        <div className="lang-grid" role="radiogroup" aria-label={t("set.language")}>
          {LANGUAGES.map((l) => (
            <button key={l.id} type="button" role="radio" aria-checked={lang === l.id} className={`lang-option${lang === l.id ? " is-on" : ""}`} onClick={() => lang !== l.id && set({ language: l.id })}>
              <span className="lang-code">{l.id.toUpperCase()}</span>
              <strong>{l.name}</strong>
              <span className="lbl">{lang === l.id ? t("lang.active") : l.id === "en" ? t("lang.default") : ""}</span>
            </button>
          ))}
        </div>
      </Card>
      <Note>{t("lang.note")}</Note>
    </>
  );
}

function BackupPage() {
  const { env, saveConfig, saveSettings } = useApp();
  const toast = useToast();
  const [path, setPath] = useState(`${env.data_dir}\\klip-settings.json`);
  const exportTo = async () => {
    try {
      const target = isTauri ? ((await pickSaveFile(path, t("bk.fileType"), ["json"])) ?? null) : path;
      if (!target) return;
      await call("settings_export", { path: target });
      setPath(target);
      toast(t("bk.exported", target));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const importFrom = async () => {
    try {
      const source = isTauri ? ((await pickFile(path, t("bk.fileType"), ["json"])) ?? null) : path;
      if (!source) return;
      const bundle = await call<{ klip_settings?: number; config?: Config; app?: unknown }>("settings_import", { path: source });
      if (bundle.klip_settings !== 1 || !bundle.config) throw new Error(t("bk.notKlip"));
      // The backend checks the config against the format of the recorder before it writes.
      await saveConfig(bundle.config);
      await saveSettings(normalizeSettings(bundle.app));
      toast(t("bk.imported"));
    } catch (e) {
      toast(t("bk.importFailed", errorText(e)), "error");
    }
  };
  return (
    <>
      <Lead>{t("bk.lead")}</Lead>
      <Card title={t("bk.file")}>
        {isTauri ? null : (
          <Row stacked label={t("bk.file")} help={t("bk.fileHelp")}>
            <PathField label={t("bk.file")} value={path} pick={{ name: t("bk.fileType"), extensions: ["json"] }} onChange={setPath} />
          </Row>
        )}
        <Row label={t("bk.export")} help={t("bk.exportHelp")}>
          <Button icon={<Upload size={15} />} onClick={() => void exportTo()}>{t("bk.export")}</Button>
        </Row>
        <Row label={t("bk.import")} help={t("bk.importHelp")}>
          <Button icon={<Download size={15} />} onClick={() => void importFrom()}>{t("bk.import")}</Button>
        </Row>
      </Card>
      <Note>
        <span className="mono small">{env.config_path}</span>
      </Note>
    </>
  );
}

function loadAdvanced(): boolean {
  try {
    return localStorage.getItem("klip.settings.advanced") === "1";
  } catch {
    return false;
  }
}

export function Settings({ page }: { page: string }) {
  const { go, configError } = useApp();
  const current = PAGES.find((p) => p.id === page) ?? PAGES[0];
  const [advanced, setAdvancedState] = useState(loadAdvanced);
  const setAdvanced = (on: boolean) => {
    setAdvancedState(on);
    try {
      localStorage.setItem("klip.settings.advanced", on ? "1" : "0");
    } catch {
      // Without storage the choice lasts as long as the window.
    }
  };
  return (
    <Shell title={t("nav.settings")} sub={t(current.label)}>
      <div className="settings">
        <nav className="set-nav">
          {PAGES.map((p) => (
            <button key={p.id} type="button" className={`set-item${current.id === p.id ? " is-on" : ""}`} aria-current={current.id === p.id ? "page" : undefined} onClick={() => go({ view: "settings", page: p.id })}>
              <p.icon size={16} />
              <span>{t(p.label)}</span>
              {p.id === "cloud" && !CLOUD_READY ? <span className="soon-badge">{t("cl.soon")}</span> : null}
            </button>
          ))}
        </nav>
        <main className="settings-main">
          <AdvancedContext.Provider value={advanced}>
          <div className="settings-inner">
            <div className="page-head">
              <h2 className="page-title">{t(current.label)}</h2>
              <label className="more-switch">
                <span>{t("set.more")}</span>
                <Toggle checked={advanced} label={t("set.more")} onChange={setAdvanced} />
              </label>
            </div>
            {configError ? (
              <Note warn>
                {t("set.configBroken")} <span className="mono small">{configError}</span>
              </Note>
            ) : null}
            {current.id === "recording" ? <RecordingPage /> : null}
            {current.id === "audio" ? <AudioPage /> : null}
            {current.id === "overlay" ? <OverlayPage /> : null}
            {current.id === "input" ? <InputPage /> : null}
            {current.id === "autoclips" ? <AutoClipsPage /> : null}
            {current.id === "hotkeys" ? <HotkeysPage /> : null}
            {current.id === "storage" ? <StoragePage /> : null}
            {current.id === "export" ? <ExportPage /> : null}
            {current.id === "appearance" ? <AppearancePage /> : null}
            {current.id === "language" ? <LanguagePage /> : null}
            {current.id === "cloud" ? <CloudPage /> : null}
            {current.id === "backup" ? <BackupPage /> : null}
          </div>
          </AdvancedContext.Provider>
        </main>
      </div>
    </Shell>
  );
}
