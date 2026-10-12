// The library: search in the HUD bar, kinds and games as filter chips, the grid grouped by day.

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { Check, Film, FolderOpen, Image as ImageIcon, Pencil, RefreshCw, Search, Star, Tag, Trash2, X } from "lucide-react";
import { call, dragFilesOut, errorText, fileUrl } from "../lib/api";
import { dayLabel, formatBytes, formatDuration, formatTime } from "../lib/format";
import { t, tn } from "../lib/i18n";
import type { Key } from "../lib/i18n";
import type { Clip, DateRange, Filters, LengthRange } from "../lib/library";
import { countBy, eventLabel, gameLabel, groupByDay, highlightEvent, kindLabel, matches, NO_FILTERS, sortClips } from "../lib/library";
import { knownThumb, requestThumb } from "../lib/thumbs";
import type { SortKey } from "../lib/types";
import { useApp } from "../state";
import { Button, Chip, ContextMenu, Modal, SectionHead, Select, useToast } from "../ui";
import type { MenuItem } from "../ui";
import { Shell } from "./Shell";

/** Value of the game dropdown for "every game". No game is called like this. */
const ALL_GAMES = "::all";

const VIEWS: { id: Filters["view"]; label: Key; title: Key }[] = [
  { id: "all", label: "chip.all", title: "view.all" },
  { id: "clip", label: "chip.clips", title: "view.clips" },
  { id: "auto_clip", label: "chip.auto", title: "view.auto" },
  { id: "recording", label: "chip.recordings", title: "view.recordings" },
  { id: "screenshot", label: "chip.screenshots", title: "view.screenshots" },
  { id: "favorites", label: "chip.favorites", title: "view.favorites" },
];

interface CardProps {
  clip: Clip;
  selected: boolean;
  selecting: boolean;
  onOpen: (clip: Clip, event: ReactMouseEvent) => void;
  onToggle: (clip: Clip, range: boolean) => void;
  onFavorite: (clip: Clip) => void;
  onMenu: (clip: Clip, x: number, y: number) => void;
  onDragOut: (clip: Clip) => void;
}

const ClipCard = memo(function ClipCard({ clip, selected, selecting, onOpen, onToggle, onFavorite, onMenu, onDragOut }: CardProps) {
  const ref = useRef<HTMLDivElement>(null);
  const scrub = useRef<HTMLElement>(null);
  const [thumb, setThumb] = useState<string | null>(() => knownThumb(clip));
  const [thumbFailed, setThumbFailed] = useState(false);
  const [preview, setPreview] = useState(false);
  const hover = useRef(0);
  const press = useRef<{ x: number; y: number } | null>(null);

  // Ask for the thumbnail only once the card scrolls into view.
  useEffect(() => {
    if (thumb || !ref.current) return;
    let alive = true;
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        observer.disconnect();
        requestThumb(clip).then((path) => alive && setThumb(path), () => alive && setThumbFailed(true));
      },
      { rootMargin: "300px" },
    );
    observer.observe(ref.current);
    return () => {
      alive = false;
      observer.disconnect();
    };
  }, [clip, thumb]);

  const enter = () => {
    if (clip.item.media !== "video") return;
    hover.current = window.setTimeout(() => setPreview(true), 220);
  };
  const leave = () => {
    window.clearTimeout(hover.current);
    setPreview(false);
    press.current = null;
  };
  const badge = highlightEvent(clip);
  const kills = clip.events.filter((e) => e.kind === "kill").length;
  const badgeText = badge ? (badge === "kill" && kills > 1 ? t("lib.kills", kills) : eventLabel(badge)) : clip.kind === "recording" ? t("lib.badgeRec") : null;

  return (
    <div
      ref={ref}
      className={`clip-card${selected ? " is-selected" : ""}${selecting ? " is-selecting" : ""}`}
      onMouseEnter={enter}
      onMouseLeave={leave}
      onMouseDown={(e) => {
        if (e.button === 0) press.current = { x: e.clientX, y: e.clientY };
      }}
      onMouseMove={(e) => {
        // A press that travels becomes a drag of the file out of the window.
        const start = press.current;
        if (start && e.buttons === 1 && Math.hypot(e.clientX - start.x, e.clientY - start.y) > 8) {
          press.current = null;
          onDragOut(clip);
        }
      }}
      onMouseUp={() => (press.current = null)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(clip, e.clientX, e.clientY);
      }}
    >
      {/* The four corner brackets of the HUD: two on hover, all four when selected. */}
      <i className="br br-tl" />
      <i className="br br-tr" />
      <i className="br br-bl" />
      <i className="br br-br" />
      <button type="button" className="clip-thumb" onClick={(e) => onOpen(clip, e)} aria-label={t("lib.open", clip.title)}>
        {thumb ? <img src={fileUrl(thumb)} alt="" draggable={false} loading="lazy" /> : <span className="thumb-empty">{thumbFailed ? <ImageIcon size={22} /> : null}</span>}
        {preview ? (
          <video
            src={fileUrl(clip.path)}
            muted
            loop
            autoPlay
            playsInline
            preload="auto"
            onTimeUpdate={(e) => {
              // The thin line at the lower edge follows the preview.
              const video = e.currentTarget;
              if (scrub.current && video.duration > 0) scrub.current.style.width = `${(video.currentTime / video.duration) * 100}%`;
            }}
          />
        ) : null}
        <span className="thumb-foot" />
        {clip.kind === "auto_clip" ? (
          <span className="tag" title={t("lib.auto")}>
            {t("lib.autoBadge")}
            {clip.reason ? ` / ${badge === "kill" && clip.reason === "kill" && kills > 1 ? t("lib.kills", kills) : eventLabel(clip.reason)}` : ""}
          </span>
        ) : badgeText ? (
          <span className={`tag${!badge ? " is-rec" : badge === "death" ? " is-plain" : ""}`}>{badgeText}</span>
        ) : null}
        <span className="dur">{clip.kind === "screenshot" ? clip.item.ext.toUpperCase() : clip.durationMs > 0 ? formatDuration(clip.durationMs) : ""}</span>
        {preview ? (
          <span className="scrub">
            <i ref={scrub} />
          </span>
        ) : null}
      </button>
      <button type="button" className={`clip-check${selected ? " is-on" : ""}`} aria-label={selected ? t("lib.deselect") : t("lib.select")} onClick={(e) => onToggle(clip, e.shiftKey)}>
        {selected ? <Check size={13} strokeWidth={3} /> : null}
      </button>
      <button type="button" className={`clip-star${clip.favorite ? " is-on" : ""}`} aria-label={clip.favorite ? t("lib.unfavorite") : t("lib.favorite")} onClick={() => onFavorite(clip)}>
        <Star size={14} fill={clip.favorite ? "currentColor" : "none"} />
      </button>
      <div className="clip-info">
        <h3 className="clip-title" title={clip.title}>
          {clip.title}
        </h3>
        <p className="lbl clip-sub">
          <b className="clip-game">{gameLabel(clip.game)}</b> / {clip.kind === "auto_clip" ? `${t("lib.auto")}${clip.reason ? `: ${eventLabel(clip.reason)}` : ""}` : kindLabel(clip.kind)} / {formatTime(clip.created)}
        </p>
        {clip.tags.length > 0 || !clip.hasSidecar || clip.item.has_edit ? (
          <div className="clip-tags">
            {clip.item.has_edit ? <span className="mini mini-edit">{t("lib.edited")}</span> : null}
            {!clip.hasSidecar ? (
              <span className="mini mini-warn" title={clip.item.meta_error ?? t("lib.noSidecarHelp")}>
                {clip.item.meta_error ? t("lib.sidecarBroken") : t("lib.noSidecar")}
              </span>
            ) : null}
            {clip.tags.map((tag) => (
              <span key={tag} className="mini">
                {tag}
              </span>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
});

function TagsDialog(props: { clips: Clip[]; allTags: string[]; onClose: () => void }) {
  const { updateMeta } = useApp();
  const toast = useToast();
  // A tag counts as set when every chosen clip has it.
  const [tags, setTags] = useState<string[]>(() => props.allTags.filter((x) => props.clips.every((c) => c.tags.includes(x))));
  const [text, setText] = useState("");
  const add = (raw: string) => {
    const tag = raw.trim().toLowerCase();
    if (tag && !tags.includes(tag)) setTags([...tags, tag]);
    setText("");
  };
  const before = useMemo(() => props.allTags.filter((x) => props.clips.every((c) => c.tags.includes(x))), [props.allTags, props.clips]);
  const save = async () => {
    const pending = text.trim().toLowerCase();
    const final = pending && !tags.includes(pending) ? [...tags, pending] : tags;
    const removed = before.filter((x) => !final.includes(x));
    try {
      for (const clip of props.clips) {
        const next = [...clip.tags.filter((x) => !removed.includes(x)), ...final.filter((x) => !clip.tags.includes(x))];
        await updateMeta(clip, { tags: next });
      }
      props.onClose();
    } catch (error) {
      toast(errorText(error), "error");
    }
  };
  const suggestions = props.allTags.filter((x) => !tags.includes(x));
  return (
    <Modal
      title={props.clips.length === 1 ? t("lib.tags") : t("lib.tagsFor", props.clips.length)}
      onClose={props.onClose}
      footer={
        <>
          <Button kind="ghost" onClick={props.onClose}>{t("common.cancel")}</Button>
          <Button kind="primary" onClick={save}>{t("common.save")}</Button>
        </>
      }
    >
      <div className="tag-editor">
        {tags.map((x) => (
          <button key={x} type="button" className="mini mini-removable" onClick={() => setTags(tags.filter((y) => y !== x))} title={t("common.remove")}>
            {x}
            <X size={12} />
          </button>
        ))}
        <input
          className="tag-input"
          autoFocus
          value={text}
          placeholder={t("lib.tagPlaceholder")}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === ",") {
              e.preventDefault();
              add(text);
            } else if (e.key === "Backspace" && text === "" && tags.length > 0) {
              setTags(tags.slice(0, -1));
            }
          }}
        />
      </div>
      {suggestions.length > 0 ? (
        <div className="tag-suggest">
          <span className="lbl">{t("lib.tagsExisting")}</span>
          {suggestions.map((x) => (
            <button key={x} type="button" className="mini mini-button" onClick={() => add(x)}>
              {x}
            </button>
          ))}
        </div>
      ) : null}
    </Modal>
  );
}

function RenameDialog(props: { clip: Clip; onClose: () => void }) {
  const { rename } = useApp();
  const [name, setName] = useState(props.clip.title);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    try {
      await rename(props.clip, name);
      props.onClose();
    } catch (e) {
      setError(errorText(e));
    }
  };
  return (
    <Modal
      title={t("lib.rename")}
      onClose={props.onClose}
      footer={
        <>
          <Button kind="ghost" onClick={props.onClose}>{t("common.cancel")}</Button>
          <Button kind="primary" onClick={save} disabled={name.trim() === ""}>{t("lib.rename")}</Button>
        </>
      }
    >
      <input className="input input-wide" autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && save()} onFocus={(e) => e.target.select()} />
      <p className="muted small">{t("lib.renameHelp")}</p>
      {error ? <p className="error-text">{error}</p> : null}
    </Modal>
  );
}

export function Library() {
  const app = useApp();
  const { clips, filters, setFilters, settings, saveSettings, go, updateMeta, trash, rescan, scanning } = app;
  const toast = useToast();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState<{ clip: Clip; x: number; y: number } | null>(null);
  const [tagging, setTagging] = useState<Clip[] | null>(null);
  const [renaming, setRenaming] = useState<Clip | null>(null);
  const [deleting, setDeleting] = useState<Clip[] | null>(null);
  const [viewing, setViewing] = useState<Clip | null>(null);
  const lastToggled = useRef<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const visible = useMemo(() => sortClips(clips.filter((c) => matches(c, filters)), settings.sort), [clips, filters, settings.sort]);
  const byDate = settings.sort === "date_desc" || settings.sort === "date_asc";
  const groups = useMemo(() => (byDate ? groupByDay(visible) : [{ key: "all", date: new Date(), clips: visible }]), [visible, byDate]);
  const allTags = useMemo(() => [...countBy(clips, (c) => c.tags).keys()].sort(), [clips]);
  const allEvents = useMemo(() => [...countBy(clips, (c) => [...new Set(c.events.map((e) => e.kind))]).keys()].sort(), [clips]);
  const kinds = useMemo(() => countBy(clips, (c) => [c.kind]), [clips]);
  const games = useMemo(() => [...countBy(clips, (c) => [c.game ?? ""]).entries()].sort((a, b) => b[1] - a[1]), [clips]);
  const favorites = useMemo(() => clips.filter((c) => c.favorite).length, [clips]);
  const selectedClips = useMemo(() => clips.filter((c) => selected.has(c.path)), [clips, selected]);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const selectedRef = useRef(selected);
  selectedRef.current = selected;

  // Drop selections of clips that no longer exist.
  useEffect(() => {
    setSelected((old) => {
      const paths = new Set(clips.map((c) => c.path));
      const next = new Set([...old].filter((p) => paths.has(p)));
      return next.size === old.size ? old : next;
    });
  }, [clips]);

  // Ctrl+K puts the cursor into the search field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const toggle = useCallback((clip: Clip, range: boolean) => {
    setSelected((old) => {
      const next = new Set(old);
      const list = visibleRef.current;
      const from = list.findIndex((c) => c.path === lastToggled.current);
      const to = list.findIndex((c) => c.path === clip.path);
      if (range && from >= 0 && to >= 0) {
        for (let i = Math.min(from, to); i <= Math.max(from, to); i++) next.add(list[i].path);
      } else if (next.has(clip.path)) {
        next.delete(clip.path);
      } else {
        next.add(clip.path);
      }
      return next;
    });
    lastToggled.current = clip.path;
  }, []);

  const open = useCallback(
    (clip: Clip, event: ReactMouseEvent) => {
      if (event.ctrlKey || event.shiftKey || selectedRef.current.size > 0) return toggle(clip, event.shiftKey);
      if (clip.item.media === "image") setViewing(clip);
      else go({ view: "editor", path: clip.path });
    },
    [go, toggle],
  );

  const favorite = useCallback(
    (clip: Clip) => {
      updateMeta(clip, { favorite: !clip.favorite }).catch((e) => toast(errorText(e), "error"));
    },
    [updateMeta, toast],
  );

  const reveal = useCallback((clip: Clip) => call("reveal", { path: clip.path }).catch((e) => toast(errorText(e), "error")), [toast]);

  const dragOut = useCallback(
    (clip: Clip) => {
      const chosen = selectedRef.current.has(clip.path) ? [...selectedRef.current] : [clip.path];
      requestThumb(clip)
        .then((icon) => dragFilesOut(chosen, icon))
        .catch((e) => toast(t("lib.dragFailed", errorText(e)), "error"));
    },
    [toast],
  );

  const openMenu = useCallback((clip: Clip, x: number, y: number) => setMenu({ clip, x, y }), []);

  const menuItems = (clip: Clip): MenuItem[] => {
    const group = selected.has(clip.path) && selected.size > 1 ? selectedClips : [clip];
    const many = group.length > 1;
    return [
      ...(many ? [] : [{ label: clip.item.media === "image" ? t("lib.view") : t("lib.openEditor"), icon: <Film size={15} />, onClick: () => (clip.item.media === "image" ? setViewing(clip) : go({ view: "editor", path: clip.path })) }]),
      ...(many ? [] : [{ label: t("lib.rename"), icon: <Pencil size={15} />, onClick: () => setRenaming(clip) }]),
      { label: many ? t("lib.tagsFor", group.length) : t("lib.tags"), icon: <Tag size={15} />, onClick: () => setTagging(group) },
      {
        label: group.every((c) => c.favorite) ? t("lib.unfavorite") : t("lib.favorite"),
        icon: <Star size={15} />,
        onClick: () => {
          const value = !group.every((c) => c.favorite);
          void Promise.all(group.map((c) => updateMeta(c, { favorite: value }))).catch((e) => toast(errorText(e), "error"));
        },
      },
      ...(many ? [] : [{ label: t("lib.reveal"), icon: <FolderOpen size={15} />, onClick: () => void reveal(clip), separator: true }]),
      ...(!many && !clip.hasSidecar && !clip.item.meta_error
        ? [{ label: t("lib.createSidecar"), icon: <Check size={15} />, onClick: () => void updateMeta(clip, {}).then(() => toast(t("lib.sidecarCreated")), (e) => toast(errorText(e), "error")) }]
        : []),
      { label: many ? t("lib.deleteMany", group.length) : t("common.delete"), icon: <Trash2 size={15} />, danger: true, separator: true, onClick: () => setDeleting(group) },
    ];
  };

  const filtersActive = filters.game !== null || filters.date !== "all" || filters.length !== "all" || filters.event !== null || filters.tag !== null || filters.search !== "";
  const view = VIEWS.find((v) => v.id === filters.view) ?? VIEWS[0];
  const counts: Record<Filters["view"], number> = {
    all: clips.length,
    favorites,
    clip: kinds.get("clip") ?? 0,
    auto_clip: kinds.get("auto_clip") ?? 0,
    recording: kinds.get("recording") ?? 0,
    screenshot: kinds.get("screenshot") ?? 0,
  };

  const confirmDelete = async () => {
    if (!deleting) return;
    const list = deleting;
    setDeleting(null);
    try {
      await trash(list);
      toast(tn("lib.trashed", list.length));
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <Shell
      title={t(view.title)}
      sub={
        <>
          {tn("lib.count", visible.length)}
          {filters.game !== null ? ` / ${gameLabel(filters.game)}` : ""}
        </>
      }
      hud={
        <>
          <div className="search">
            <Search size={14} />
            <input ref={searchRef} placeholder={t("lib.searchPlaceholder")} value={filters.search} onChange={(e) => setFilters({ ...filters, search: e.target.value })} aria-label={t("lib.search")} />
            {filters.search ? (
              <button type="button" className="search-clear" onClick={() => setFilters({ ...filters, search: "" })} aria-label={t("lib.searchClear")}>
                <X size={14} />
              </button>
            ) : (
              <span className="lbl">{t("lib.searchKey")}</span>
            )}
          </div>
          <Button kind="ghost" icon={<RefreshCw size={15} className={scanning ? "spin" : ""} />} title={t("lib.rescan")} onClick={() => void rescan()} />
        </>
      }
    >
      <main className="main">
        <div className="filters">
          <div className="chips">
            {VIEWS.map((v) => (
              <Chip key={v.id} on={filters.view === v.id} count={counts[v.id]} onClick={() => setFilters({ ...filters, view: v.id })}>
                {t(v.label)}
              </Chip>
            ))}
          </div>
          <div className="filterbar">
            <Select
              label={t("lib.fGame")}
              value={filters.game === null ? ALL_GAMES : filters.game}
              onChange={(game) => setFilters({ ...filters, game: game === ALL_GAMES ? null : game })}
              options={[{ value: ALL_GAMES, label: t("lib.gameAll", clips.length) }, ...games.map(([game, count]) => ({ value: game, label: `${gameLabel(game)} (${count})` }))]}
            />
            <Select<DateRange>
              label={t("lib.fDate")}
              value={filters.date}
              onChange={(date) => setFilters({ ...filters, date })}
              options={[
                { value: "all", label: t("lib.dateAll") },
                { value: "today", label: t("day.today") },
                { value: "week", label: t("lib.dateWeek") },
                { value: "month", label: t("lib.dateMonth") },
              ]}
            />
            <Select<LengthRange>
              label={t("lib.fLength")}
              value={filters.length}
              onChange={(length) => setFilters({ ...filters, length })}
              options={[
                { value: "all", label: t("lib.lenAll") },
                { value: "short", label: t("lib.lenShort") },
                { value: "medium", label: t("lib.lenMedium") },
                { value: "long", label: t("lib.lenLong") },
              ]}
            />
            <Select
              label={t("lib.fEvent")}
              value={filters.event ?? ""}
              onChange={(event) => setFilters({ ...filters, event: event || null })}
              options={[{ value: "", label: t("lib.eventAll") }, ...allEvents.map((k) => ({ value: k, label: eventLabel(k) }))]}
            />
            <Select
              label={t("lib.fTag")}
              value={filters.tag ?? ""}
              onChange={(tag) => setFilters({ ...filters, tag: tag || null })}
              options={[{ value: "", label: t("lib.tagAll") }, ...allTags.map((x) => ({ value: x, label: x }))]}
            />
            {filtersActive ? (
              <Button kind="ghost" small icon={<X size={14} />} onClick={() => setFilters({ ...NO_FILTERS, view: filters.view })}>
                {t("lib.resetFilters")}
              </Button>
            ) : null}
            <span className="spacer" />
            <Select<SortKey>
              label={t("lib.fSort")}
              value={settings.sort}
              onChange={(sort) => void saveSettings({ ...settings, sort })}
              options={[
                { value: "date_desc", label: t("sort.newest") },
                { value: "date_asc", label: t("sort.oldest") },
                { value: "length_desc", label: t("sort.longest") },
                { value: "length_asc", label: t("sort.shortest") },
                { value: "size_desc", label: t("sort.largest") },
                { value: "name", label: t("sort.name") },
              ]}
            />
          </div>
        </div>

        <div className="grid-scroll">
          {app.libraryMissing ? (
            <div className="empty">
              <h2>{t("lib.missingTitle")}</h2>
              <p className="mono">{app.libraryDir}</p>
              <p>{t("lib.missingText")}</p>
            </div>
          ) : visible.length === 0 && !scanning ? (
            <div className="empty">
              <h2>{clips.length === 0 ? t("lib.emptyTitle") : t("lib.nothingTitle")}</h2>
              <p>{clips.length === 0 ? t("lib.emptyText") : t("lib.nothingText")}</p>
              {clips.length === 0 ? <p className="mono">{app.libraryDir}</p> : null}
            </div>
          ) : (
            groups.map((group) => (
              <section key={group.key} className="day">
                {byDate ? <SectionHead title={dayLabel(group.date)} note={`${tn("lib.count", group.clips.length)} / ${formatBytes(group.clips.reduce((sum, c) => sum + c.item.size, 0))}`} /> : null}
                <div className="grid">
                  {group.clips.map((clip) => (
                    <ClipCard
                      key={clip.path}
                      clip={clip}
                      selected={selected.has(clip.path)}
                      selecting={selected.size > 0}
                      onOpen={open}
                      onToggle={toggle}
                      onFavorite={favorite}
                      onMenu={openMenu}
                      onDragOut={dragOut}
                    />
                  ))}
                </div>
              </section>
            ))
          )}
        </div>

        {selected.size > 0 ? (
          <div className="selection-bar">
            <span className="selection-count">
              <span className="mono">{selected.size}</span> {t("lib.selected")}
            </span>
            <Button small kind="ghost" onClick={() => setSelected(new Set(visible.map((c) => c.path)))}>{t("lib.selectAll", visible.length)}</Button>
            <span className="vr" />
            <Button small icon={<Star size={14} />} onClick={() => {
              const value = !selectedClips.every((c) => c.favorite);
              void Promise.all(selectedClips.map((c) => updateMeta(c, { favorite: value }))).catch((e) => toast(errorText(e), "error"));
            }}>
              {t("lib.favoriteShort")}
            </Button>
            <Button small icon={<Tag size={14} />} onClick={() => setTagging(selectedClips)}>{t("lib.tags")}</Button>
            {selectedClips.length === 1 ? <Button small icon={<FolderOpen size={14} />} onClick={() => void reveal(selectedClips[0])}>{t("lib.revealShort")}</Button> : null}
            <Button small kind="danger" icon={<Trash2 size={14} />} onClick={() => setDeleting(selectedClips)}>{t("common.delete")}</Button>
            <span className="vr" />
            <Button small kind="ghost" icon={<X size={14} />} title={t("lib.deselect")} onClick={() => setSelected(new Set())} />
          </div>
        ) : null}
      </main>

      {menu ? <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.clip)} onClose={() => setMenu(null)} /> : null}
      {tagging ? <TagsDialog clips={tagging} allTags={allTags} onClose={() => setTagging(null)} /> : null}
      {renaming ? <RenameDialog clip={renaming} onClose={() => setRenaming(null)} /> : null}
      {deleting ? (
        <Modal
          title={tn("lib.deleteTitle", deleting.length)}
          onClose={() => setDeleting(null)}
          footer={
            <>
              <Button kind="ghost" onClick={() => setDeleting(null)}>{t("common.cancel")}</Button>
              <Button kind="danger" icon={<Trash2 size={15} />} onClick={confirmDelete}>{t("common.toTrash")}</Button>
            </>
          }
        >
          <p>
            {deleting.length === 1 ? <span className="mono">{deleting[0].title}</span> : t("lib.nFiles", deleting.length)} {t("lib.deleteText", formatBytes(deleting.reduce((s, c) => s + c.item.size, 0)))}
          </p>
          {deleting.some((c) => c.favorite) ? <p className="warn-text">{t("lib.deleteFavorites")}</p> : null}
        </Modal>
      ) : null}
      {viewing ? (
        <Modal title={viewing.title} wide onClose={() => setViewing(null)} footer={<Button icon={<FolderOpen size={15} />} onClick={() => void reveal(viewing)}>{t("lib.reveal")}</Button>}>
          <img className="viewer-image" src={fileUrl(viewing.path)} alt={viewing.title} />
        </Modal>
      ) : null}
    </Shell>
  );
}
