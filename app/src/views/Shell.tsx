// The frame around every view: the icon rail on the left and the HUD bar on top. The
// state of the recorder and the storage live in the HUD bar, never in a box.

import { useMemo } from "react";
import type { ReactNode } from "react";
import { Camera, LayoutGrid, Scissors, Settings as SettingsIcon, Sparkles, Star, Video } from "lucide-react";
import { call, errorText } from "../lib/api";
import { formatBytes, formatDuration } from "../lib/format";
import { t, tn } from "../lib/i18n";
import type { Key } from "../lib/i18n";
import type { Filters } from "../lib/library";
import { NO_FILTERS } from "../lib/library";
import { useApp } from "../state";
import { Segments } from "../ui";

const RAIL: { view: Filters["view"]; short: Key; full: Key; icon: typeof Video }[] = [
  { view: "all", short: "rail.lib", full: "view.all", icon: LayoutGrid },
  { view: "clip", short: "rail.clip", full: "view.clips", icon: Scissors },
  { view: "auto_clip", short: "rail.auto", full: "view.auto", icon: Sparkles },
  { view: "recording", short: "rail.rec", full: "view.recordings", icon: Video },
  { view: "screenshot", short: "rail.shot", full: "view.screenshots", icon: Camera },
  { view: "favorites", short: "rail.fav", full: "view.favorites", icon: Star },
];

export function Mark() {
  return (
    <div className="mark" aria-label="Klip">
      K<i />
    </div>
  );
}

function Rail() {
  const { route, filters, setFilters, go } = useApp();
  return (
    <nav className="rail">
      <Mark />
      {RAIL.map((item) => {
        const on = route.view === "library" && filters.view === item.view;
        return (
          <button
            key={item.view}
            type="button"
            className={`rail-item${on ? " is-on" : ""}`}
            title={t(item.full)}
            aria-current={on ? "page" : undefined}
            onClick={() => {
              // From another view the rail leads back to the whole library of that kind.
              setFilters(route.view === "library" ? { ...filters, view: item.view } : { ...NO_FILTERS, view: item.view });
              go({ view: "library" });
            }}
          >
            <item.icon size={18} />
            <span>{t(item.short)}</span>
          </button>
        );
      })}
      <span className="rail-space" />
      <button
        type="button"
        className={`rail-item${route.view === "settings" ? " is-on" : ""}`}
        title={t("nav.settings")}
        aria-current={route.view === "settings" ? "page" : undefined}
        onClick={() => go({ view: "settings", page: "recording" })}
      >
        <SettingsIcon size={18} />
        <span>{t("rail.set")}</span>
      </button>
    </nav>
  );
}

/** The readouts on the right of the HUD bar: label on top, value below. */
function Readouts() {
  const { recorder, clips, settings, disk } = useApp();
  const used = useMemo(() => clips.reduce((sum, c) => sum + c.item.size, 0), [clips]);
  const limit = settings.storage_limit_gb * 1024 ** 3;
  const bound = limit > 0 ? limit : disk ? used + disk.free : 0;
  const ratio = bound > 0 ? used / bound : 0;
  const status = recorder?.connected ? recorder.status : null;
  const details = status
    ? [
        `klip-recorder ${status.version}`,
        status.encoder,
        status.height > 0 ? `${status.height}p${status.fps}` : "",
        status.audio_tracks.length > 0 ? tn("hud.audioTracks", status.audio_tracks.length) : t("hud.noAudio"),
      ]
        .filter(Boolean)
        .join(", ")
    : undefined;
  return (
    <div className="read">
      {recorder === null ? null : status ? (
        <>
          <div title={details}>
            <span>{t("hud.buffer")}</span>
            <strong>
              <i className="dot" />
              {Math.round(status.buffered_seconds)} / {status.buffer_seconds} S
            </strong>
          </div>
          <div title={details}>
            <span>{t("hud.rec")}</span>
            <strong>
              {status.recording ? <i className="dot rec" /> : null}
              {status.recording ? formatDuration(status.recording_ms) : "--:--"}
            </strong>
          </div>
          <div className="read-opt" title={details}>
            <span>{t("hud.game")}</span>
            <strong className="read-game">{status.game ?? "--"}</strong>
          </div>
          {status.hotkey_problems && status.hotkey_problems.length > 0 ? (
            <div title={status.hotkey_problems.join("\n")}>
              <span>{t("hud.keys")}</span>
              <strong className="read-warn">
                <i className="dot rec" />
                {t("hud.keysBroken", status.hotkey_problems.length)}
              </strong>
            </div>
          ) : null}
        </>
      ) : (
        <button type="button" className="read-button" title={t("hud.recorderOffHelp")} onClick={() => void call("recorder_start").catch((e) => window.alert(t("hud.startFailed", errorText(e))))}>
          <span>{t("hud.status")}</span>
          <strong>
            <i className="dot off" />
            {t("hud.recorderOff")} / {t("hud.startRecorder")}
          </strong>
        </button>
      )}
      <div className="read-opt" title={disk ? t("hud.diskHelp", formatBytes(disk.free), formatBytes(disk.total)) : t("hud.diskUnknown")}>
        <span>{t("hud.diskFree")}</span>
        <strong>{disk ? formatBytes(disk.free) : "--"}</strong>
      </div>
      <div className="read-opt" title={limit > 0 ? t("hud.storageOfLimit", formatBytes(used), formatBytes(limit)) : t("hud.storageNoLimit", formatBytes(used))}>
        <span>{t("hud.storage")}</span>
        <strong>
          {limit > 0 ? `${formatBytes(used)} / ${formatBytes(limit)}` : formatBytes(used)}
          <Segments value={ratio} warn={limit > 0 && ratio > 0.9} count={14} />
        </strong>
      </div>
    </div>
  );
}

export function Shell(props: { title: string; sub?: ReactNode; hud?: ReactNode; busy?: boolean; children: ReactNode }) {
  return (
    <div className="app">
      <Rail />
      <header className={`hud${props.busy ? " is-busy" : ""}`}>
        <h1 title={props.title}>{props.title}</h1>
        {props.sub ? <span className="lbl hud-sub">{props.sub}</span> : null}
        <div className="hud-mid">{props.hud}</div>
        <Readouts />
      </header>
      {props.children}
    </div>
  );
}
