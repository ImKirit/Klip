// Small building blocks shared by all views.

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { FolderOpen, X } from "lucide-react";
import { isTauri, pickFile, pickFolder } from "./lib/api";
import { t } from "./lib/i18n";

type ButtonKind = "default" | "primary" | "ghost" | "danger";

export function Button(props: {
  children?: ReactNode;
  kind?: ButtonKind;
  icon?: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  small?: boolean;
  active?: boolean;
}) {
  const cls = ["btn", `btn-${props.kind ?? "default"}`, props.small ? "btn-small" : "", props.active ? "is-active" : "", props.children ? "" : "btn-icon"];
  return (
    <button type="button" className={cls.filter(Boolean).join(" ")} onClick={props.onClick} disabled={props.disabled} title={props.title} aria-label={props.children ? undefined : props.title}>
      {props.icon}
      {props.children ? <span>{props.children}</span> : null}
    </button>
  );
}

export function Toggle(props: { checked: boolean; onChange: (value: boolean) => void; label?: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={props.checked}
      aria-label={props.label}
      disabled={props.disabled}
      className={`toggle${props.checked ? " is-on" : ""}`}
      onClick={() => props.onChange(!props.checked)}
    >
      <span className="toggle-knob" />
    </button>
  );
}

export function Select<T extends string | number>(props: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
  label?: string;
  disabled?: boolean;
}) {
  const numeric = typeof props.value === "number";
  return (
    <span className={`select-wrap${props.disabled ? " is-disabled" : ""}`}>
      <select
        className="select"
        aria-label={props.label}
        value={String(props.value)}
        disabled={props.disabled}
        onChange={(e) => props.onChange((numeric ? Number(e.target.value) : e.target.value) as T)}
      >
        {props.options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {o.label}
          </option>
        ))}
      </select>
    </span>
  );
}

/** A filter chip: one of a row, the chosen one carries the accent. */
export function Chip(props: { children: ReactNode; on?: boolean; count?: number; onClick?: () => void; title?: string }) {
  return (
    <button type="button" className={`chip${props.on ? " is-on" : ""}`} aria-pressed={props.on === true} title={props.title} onClick={props.onClick}>
      <span>{props.children}</span>
      {props.count !== undefined ? <span className="chip-count">{props.count}</span> : null}
    </button>
  );
}

/** A few exclusive choices side by side. */
export function Choice<T extends string>(props: { value: T; options: { value: T; label: string; icon?: ReactNode }[]; onChange: (value: T) => void; label: string; disabled?: boolean }) {
  return (
    <div className="choice" role="radiogroup" aria-label={props.label}>
      {props.options.map((o) => (
        <button key={o.value} type="button" role="radio" aria-checked={props.value === o.value} disabled={props.disabled} className={props.value === o.value ? "is-on" : ""} onClick={() => props.onChange(o.value)}>
          {o.icon}
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Number field that only reports values inside its bounds. */
export function NumberField(props: { value: number; min: number; max: number; step?: number; unit?: string; onChange: (value: number) => void; label?: string; disabled?: boolean }) {
  const [text, setText] = useState(String(props.value));
  useEffect(() => setText(String(props.value)), [props.value]);
  const commit = () => {
    const parsed = Number(text.replace(",", "."));
    if (!Number.isFinite(parsed)) return setText(String(props.value));
    const clamped = Math.max(props.min, Math.min(props.max, parsed));
    setText(String(clamped));
    if (clamped !== props.value) props.onChange(clamped);
  };
  return (
    <span className="number-field">
      <input
        className="input mono"
        inputMode="decimal"
        aria-label={props.label}
        value={text}
        disabled={props.disabled}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      {props.unit ? <span className="unit">{props.unit}</span> : null}
    </span>
  );
}

/** Text field for a path. The browse button only exists in the real app. */
export function PathField(props: { value: string; placeholder?: string; onChange: (value: string) => void; pick: "folder" | { name: string; extensions: string[] }; label?: string; disabled?: boolean }) {
  const [text, setText] = useState(props.value);
  useEffect(() => setText(props.value), [props.value]);
  const browse = async () => {
    const start = text || props.placeholder;
    const picked = props.pick === "folder" ? await pickFolder(start) : await pickFile(start, props.pick.name, props.pick.extensions);
    if (picked) props.onChange(picked);
  };
  return (
    <span className="path-field">
      <input
        className="input mono"
        aria-label={props.label}
        value={text}
        placeholder={props.placeholder}
        disabled={props.disabled}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => text.trim() !== props.value && props.onChange(text.trim())}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      {isTauri ? <Button icon={<FolderOpen size={15} />} title={t("common.browse")} onClick={browse} disabled={props.disabled} /> : null}
    </span>
  );
}

export function Modal(props: { title: string; children: ReactNode; onClose: () => void; footer?: ReactNode; wide?: boolean; locked?: boolean }) {
  const { onClose, locked } = props;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !locked) {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, locked]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !locked && onClose()}>
      <div className={`modal${props.wide ? " modal-wide" : ""}`} role="dialog" aria-modal="true" aria-label={props.title}>
        <header className="modal-head">
          <h2>{props.title}</h2>
          {locked ? null : <Button kind="ghost" icon={<X size={16} />} title={t("common.close")} onClick={onClose} />}
        </header>
        <div className="modal-body">{props.children}</div>
        {props.footer ? <footer className="modal-foot">{props.footer}</footer> : null}
      </div>
    </div>
  );
}

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
  separator?: boolean;
}

/** Context menu at a screen position, closes on any click or Escape. */
export function ContextMenu(props: { x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: props.x, y: props.y });
  useLayoutEffect(() => {
    const box = ref.current?.getBoundingClientRect();
    if (!box) return;
    setPos({ x: Math.min(props.x, window.innerWidth - box.width - 8), y: Math.min(props.y, window.innerHeight - box.height - 8) });
  }, [props.x, props.y]);
  useEffect(() => {
    const close = () => props.onClose();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && props.onClose();
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [props]);
  return (
    <div ref={ref} className="menu" style={{ left: pos.x, top: pos.y }} role="menu" onMouseDown={(e) => e.stopPropagation()}>
      {props.items.map((item, i) => (
        <div key={i}>
          {item.separator ? <div className="menu-separator" /> : null}
          <button
            type="button"
            role="menuitem"
            className={`menu-item${item.danger ? " is-danger" : ""}`}
            disabled={item.disabled}
            onClick={() => {
              props.onClose();
              item.onClick();
            }}
          >
            {item.icon}
            <span>{item.label}</span>
          </button>
        </div>
      ))}
    </div>
  );
}

interface Toast {
  id: number;
  text: string;
  kind: "info" | "error";
}

const ToastContext = createContext<(text: string, kind?: Toast["kind"]) => void>(() => undefined);

export function useToast() {
  return useContext(ToastContext);
}

export function ToastHost(props: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(1);
  const push = useCallback((text: string, kind: Toast["kind"] = "info") => {
    const id = next.current++;
    setToasts((list) => [...list.slice(-3), { id, text, kind }]);
    window.setTimeout(() => setToasts((list) => list.filter((x) => x.id !== id)), kind === "error" ? 9000 : 3500);
  }, []);
  return (
    <ToastContext.Provider value={push}>
      {props.children}
      <div className="toasts" aria-live="polite">
        {toasts.map((item) => (
          <div key={item.id} className={`toast toast-${item.kind}`}>
            <span>{item.text}</span>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

/** A row of the settings pages: label and help on the left, control on the right. */
export function Row(props: { label: string; help?: ReactNode; children: ReactNode; stacked?: boolean }) {
  return (
    <div className={`row${props.stacked ? " row-stacked" : ""}`}>
      <div className="row-text">
        <div className="row-label">{props.label}</div>
        {props.help ? <div className="row-help">{props.help}</div> : null}
      </div>
      <div className="row-control">{props.children}</div>
    </div>
  );
}

/** Section head: accent bar, title in capitals, a hairline to the edge. */
export function SectionHead(props: { title: string; note?: ReactNode; aside?: ReactNode }) {
  return (
    <header className="sec">
      <h2>{props.title}</h2>
      {props.note ? <span className="lbl">{props.note}</span> : null}
      <span className="sec-line" />
      {props.aside}
    </header>
  );
}

/** A group of rows under a section head. No box around it. */
export function Card(props: { title?: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className={`group${props.title ? "" : " group-plain"}`}>
      {props.title ? <SectionHead title={props.title} aside={props.aside} /> : null}
      {props.children}
    </section>
  );
}

/** A bar of small squares. Without `count` it fills its width with as many as fit. */
export function Segments(props: { value: number; warn?: boolean; count?: number; style?: CSSProperties }) {
  const ref = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState(props.count ?? 0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (props.count !== undefined || !el) return;
    // Squares of 6 px with 2 px between them.
    const measure = () => setFit(Math.max(1, Math.floor((el.clientWidth + 2) / 8)));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [props.count]);
  const total = props.count ?? fit;
  const value = Math.max(0, Math.min(1, props.value));
  const lit = value > 0 ? Math.max(1, Math.round(value * total)) : 0;
  return (
    <div
      ref={ref}
      className={`meter${props.count === undefined ? " meter-fill" : ""}${props.warn ? " is-warn" : ""}`}
      style={props.style}
      role="progressbar"
      aria-valuenow={Math.round(value * 100)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      {Array.from({ length: total }, (_, i) => (
        <i key={i} className={i < lit ? "on" : undefined} />
      ))}
    </div>
  );
}

export function ProgressBar(props: { value: number; warn?: boolean; style?: CSSProperties }) {
  return <Segments value={props.value} warn={props.warn} style={props.style} />;
}
