// The frame around every view: the icon rail on the left and the HUD bar on top. The
// state of the recorder and the storage live in the HUD bar, never in a box.

import logo from "../assets/logo.svg";
import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { ReactNode } from "react";
import { Camera, ChevronDown, Cloud, LayoutGrid, LogIn, LogOut, Scissors, Settings as SettingsIcon, Sparkles, Star, UserRound, Video } from "lucide-react";
import { Avatar, useAccount } from "../account";
import { call, errorText, isTauri } from "../lib/api";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { formatBytes, formatDuration } from "../lib/format";
import { t, tn } from "../lib/i18n";
import type { Key } from "../lib/i18n";
import type { Filters } from "../lib/library";
import { NO_FILTERS } from "../lib/library";
import { useApp } from "../state";
import { ContextMenu, Segments } from "../ui";
import type { MenuItem } from "../ui";

const RAIL: { view: Filters["view"]; short: Key; full: Key; icon: typeof Video }[] = [
  { view: "all", short: "rail.lib", full: "view.all", icon: LayoutGrid },
  { view: "clip", short: "rail.clip", full: "view.clips", icon: Scissors },
  { view: "auto_clip", short: "rail.auto", full: "view.auto", icon: Sparkles },
  { view: "recording", short: "rail.rec", full: "view.recordings", icon: Video },
  { view: "screenshot", short: "rail.shot", full: "view.screenshots", icon: Camera },
  { view: "favorites", short: "rail.fav", full: "view.favorites", icon: Star },
];

export function Mark(props: { size?: number }) {
  const size = props.size ?? 38;
  return <img className="mark" src={logo} width={size} height={size} alt="Klip" draggable={false} />;
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

/** Top right: the account. Signed in it shows the Discord name and picture, signed out a
 *  sign in button. Either way it opens the menu that leads to the settings. */
function AccountButton() {
  const { account, linking, linkUrl, signIn, signOut } = useAccount();
  const { go } = useApp();
  const ref = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const open = () => {
    const r = ref.current?.getBoundingClientRect();
    if (r) setMenu({ x: r.right, y: r.bottom + 6 });
  };
  const items: MenuItem[] = [
    ...(account
      ? []
      : linkUrl
        ? [{ label: t("acc.openLink", linking ?? ""), icon: <LogIn size={15} />, onClick: () => void call("open_url", { url: linkUrl }).catch(() => window.open(linkUrl, "_blank")) }]
        : [{ label: t("cl.signIn"), icon: <LogIn size={15} />, onClick: () => void signIn() }]),
    { label: t("nav.settings"), icon: <SettingsIcon size={15} />, separator: !account, onClick: () => go({ view: "settings", page: "recording" }) },
    { label: t("acc.cloud"), icon: <Cloud size={15} />, onClick: () => go({ view: "settings", page: "cloud" }) },
    ...(account ? [{ label: t("cl.signOut"), icon: <LogOut size={15} />, separator: true, danger: true, onClick: () => void signOut() }] : []),
  ];
  return (
    <>
      <button ref={ref} type="button" className={`account${menu ? " is-open" : ""}`} title={linking ? t("cl.waiting", linking) : account ? t("cl.signedIn", account.name) : t("acc.menu")} onMouseDown={(e) => e.stopPropagation()} onClick={() => (menu ? setMenu(null) : open())}>
        {account ? (
          <>
            <Avatar account={account} size={30} />
            <span className="account-name">{account.name}</span>
          </>
        ) : (
          <>
            <span className="account-icon">
              <UserRound size={17} />
            </span>
            <span className="account-name">{linking ? t("acc.waiting", linking) : t("acc.signIn")}</span>
          </>
        )}
        <ChevronDown size={14} className="account-chev" />
      </button>
      {/* Into the body: the frosted HUD bar would otherwise be the frame of the fixed menu. */}
      {menu ? createPortal(<ContextMenu x={menu.x} y={menu.y} items={items} onClose={() => setMenu(null)} />, document.body) : null}
    </>
  );
}

/** Anything you click on in the HUD bar. Everywhere else the bar moves the window. */
const INTERACTIVE = "button, a, input, select, textarea, label, [role='button'], [role='radio'], [role='menu'], .account";

function dragWindow(e: React.MouseEvent) {
  if (!isTauri || e.button !== 0 || (e.target as HTMLElement).closest(INTERACTIVE)) return;
  const win = getCurrentWindow();
  if (e.detail === 2) void win.toggleMaximize();
  else void win.startDragging();
}

/** Minimise, maximise and close, drawn by Klip instead of Windows. No box on hover, only
 *  the sign lights up, the cross in red. */
function WindowButtons() {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri) return;
    const win = getCurrentWindow();
    void win.isMaximized().then(setMaximized);
    const off = win.onResized(() => void win.isMaximized().then(setMaximized));
    return () => void off.then((f) => f());
  }, []);
  // In the browser preview they are drawn too, so the picture matches the app, and do nothing.
  const win = isTauri ? getCurrentWindow() : { minimize: async () => {}, toggleMaximize: async () => {}, close: async () => {} };
  const props = { width: 18, height: 18, viewBox: "0 0 18 18", fill: "none", stroke: "currentColor", strokeWidth: 1.6, strokeLinecap: "round" as const };
  return (
    <div className="winbtns">
      <button type="button" className="winbtn" title={t("win.min")} onClick={() => void win.minimize()}>
        <svg {...props}>
          <path d="M3.5 9h11" />
        </svg>
      </button>
      <button type="button" className="winbtn" title={maximized ? t("win.restore") : t("win.max")} onClick={() => void win.toggleMaximize()}>
        {maximized ? (
          <svg {...props}>
            <rect x="3.5" y="6" width="8.5" height="8.5" rx="1.5" />
            <path d="M6.5 6V4.8c0-.7.6-1.3 1.3-1.3h5.4c.7 0 1.3.6 1.3 1.3v5.4c0 .7-.6 1.3-1.3 1.3H12" />
          </svg>
        ) : (
          <svg {...props}>
            <rect x="3.5" y="3.5" width="11" height="11" rx="1.8" />
          </svg>
        )}
      </button>
      <button type="button" className="winbtn winbtn-close" title={t("win.close")} onClick={() => void win.close()}>
        <svg {...props}>
          <path d="m4 4 10 10M14 4 4 14" />
        </svg>
      </button>
    </div>
  );
}

export function Shell(props: { title: string; sub?: ReactNode; hud?: ReactNode; busy?: boolean; children: ReactNode }) {
  return (
    <div className="app">
      <Rail />
      <header className={`hud${props.busy ? " is-busy" : ""}`} onMouseDown={dragWindow}>
        <h1 title={props.title}>{props.title}</h1>
        {props.sub ? <span className="lbl hud-sub">{props.sub}</span> : null}
        <div className="hud-mid">{props.hud}</div>
        <Readouts />
        <AccountButton />
        <WindowButtons />
      </header>
      {props.children}
    </div>
  );
}
