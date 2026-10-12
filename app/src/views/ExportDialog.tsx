// Export and "save as new clip". Both build an ffmpeg call, run it as a job with
// progress and cancel, and offer the result afterwards.

import { useEffect, useMemo, useRef, useState } from "react";
import { AlertTriangle, Check, ClipboardCopy, FolderOpen, X } from "lucide-react";
import { call, errorText, lastLines, startJob } from "../lib/api";
import type { JobHandle } from "../lib/api";
import type { AudioTrack } from "../lib/audio";
import { effectiveGain, sourceToOutput } from "../lib/edit";
import type { Range } from "../lib/edit";
import { buildPlan, copyBlocker, familyFor, FILTER_SCRIPT_FROM, FORMATS, isSoftware, isVideoFormat, outputSize, pickEncoder, planArgs } from "../lib/export";
import type { AudioMode, ExportOptions, Format, OverlayInput, Plan, Quality, Source } from "../lib/export";
import { dirName, formatBytes, formatDuration, formatNumber, stem } from "../lib/format";
import { t, tn } from "../lib/i18n";
import { renderForExport } from "../lib/inputs";
import type { InputTrack } from "../lib/inputs";
import type { Clip } from "../lib/library";
import { cleanAccent, onAccent } from "../lib/settings";
import type { ClipMeta, EditDoc, JobStatus } from "../lib/types";
import { metaFromProbe, useApp } from "../state";
import { Button, Chip, Modal, NumberField, PathField, ProgressBar, Select, Toggle, useToast } from "../ui";

interface Props {
  mode: "export" | "newclip";
  clip: Clip;
  source: Source;
  ranges: Range[];
  edit: EditDoc;
  tracks: AudioTrack[];
  inputs: InputTrack | null;
  /** `created` is true when a new clip landed in the library. */
  onClose: (created: boolean) => void;
}

type Phase =
  | { step: "options" }
  | { step: "running"; status: JobStatus | null; plan: Plan }
  | { step: "done"; plan: Plan; sizes: number[]; seconds: number }
  | { step: "failed"; message: string };

const SIZE_PRESETS = [10, 25, 50, 100, 500];

export function ExportDialog(props: Props) {
  const { clip, source, ranges, edit, mode } = props;
  const { env, config, settings, saveSettings } = useApp();
  const toast = useToast();
  const totalMs = ranges.reduce((sum, r) => sum + r.end_ms - r.start_ms, 0);
  const gains = useMemo(() => effectiveGain(edit.tracks), [edit.tracks]);
  const overlayWanted = edit.input_overlay?.enabled === true && props.inputs !== null;

  const [format, setFormat] = useState<Format>("mp4");
  const [height, setHeight] = useState<number | null>(null);
  const [fps, setFps] = useState<number | null>(null);
  const [quality, setQuality] = useState<Quality>("high");
  const [codec, setCodec] = useState<"h264" | "hevc">("h264");
  const [audioMode, setAudioMode] = useState<AudioMode>("mix");
  // Start with what is audible in the editor.
  const [chosen, setChosen] = useState<Set<number>>(() => new Set(props.tracks.filter((tr) => (gains.get(tr.stream) ?? 1) > 0).map((tr) => tr.stream)));
  const [sized, setSized] = useState(false);
  const [targetMb, setTargetMb] = useState(10);
  const [allowCopy, setAllowCopy] = useState(true);
  const [withOverlay, setWithOverlay] = useState(overlayWanted);
  const [name, setName] = useState(stem(clip.path));
  const [dir, setDir] = useState(settings.export_dir || env.export_dir);
  const [encoder, setEncoder] = useState<string | null>(null);
  const [encoderError, setEncoderError] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>({ step: "options" });
  const job = useRef<JobHandle | null>(null);
  const started = useRef(false);

  const options: ExportOptions = useMemo(() => {
    if (mode === "newclip") {
      // A new clip keeps every stream of the original as it is, the mix included.
      return {
        format: clip.item.ext === "mkv" ? "mkv" : "mp4",
        height: null,
        fps: null,
        quality: "high",
        codec: source.videoCodec === "hevc" ? "hevc" : "h264",
        audioMode: source.audio.length > 0 ? "separate" : "none",
        tracks: source.audio.map((a) => ({ stream: a.stream, name: a.name, volume: 1 })),
        targetMb: null,
        allowCopy: true,
      };
    }
    return {
      format,
      height,
      fps,
      quality,
      codec,
      audioMode: props.tracks.length === 0 ? "none" : audioMode,
      tracks: props.tracks.filter((tr) => chosen.has(tr.stream)).map((tr) => ({ stream: tr.stream, name: tr.name, volume: edit.tracks.find((e) => e.stream === tr.stream)?.volume ?? 1 })),
      targetMb: sized && format !== "gif" && format !== "wav" ? targetMb : null,
      allowCopy,
    };
  }, [mode, clip.item.ext, source, format, height, fps, quality, codec, audioMode, props.tracks, chosen, edit.tracks, sized, targetMb, allowCopy]);

  const overlayActive = mode === "export" && withOverlay && overlayWanted && isVideoFormat(options.format);
  const blocker = copyBlocker(source, ranges, options, overlayActive);
  const copies = blocker === null && options.allowCopy;
  const family = copies ? null : familyFor(options.format, options.codec);

  // Find an encoder that really runs here: hardware first, software as the fallback.
  useEffect(() => {
    let alive = true;
    setEncoder(null);
    setEncoderError(null);
    if (!family) return;
    pickEncoder(family).then((e) => alive && setEncoder(e), (e) => alive && setEncoderError(errorText(e)));
    return () => {
      alive = false;
    };
  }, [family]);

  // A dry plan for the summary. The real one is built again at start with real paths.
  const preview = useMemo(() => {
    try {
      const fake: OverlayInput[] = overlayActive ? [{ path: "overlay.png", x: 0, y: 0, spans: null }] : [];
      return { plan: buildPlan(source, ranges, options, encoder ?? "libx264", `out.${options.format}`, fake), error: null };
    } catch (error) {
      return { plan: null, error: errorText(error) };
    }
  }, [source, ranges, options, encoder, overlayActive]);

  const run = async () => {
    try {
      setPhase({ step: "running", status: null, plan: preview.plan! });
      const began = performance.now();
      const targetDir = mode === "newclip" ? dirName(clip.path) : dir.trim();
      const targetStem = mode === "newclip" ? `${stem(clip.path)}_${t("ex.cutSuffix")}` : name.trim() || stem(clip.path);
      const out = await call<{ path: string }>("unique_path", { dir: targetDir, stem: targetStem.replace(/[\\/:*?"<>|]/g, "_"), ext: options.format });
      const enc = family ? await pickEncoder(family) : null;

      // The overlay goes in as pictures: one at rest, one per element that lights up.
      const overlay: OverlayInput[] = [];
      if (overlayActive && edit.input_overlay && props.inputs) {
        const size = outputSize(source, options);
        const accent = cleanAccent(config.accent);
        const colors = { accent, onAccent: onAccent(accent) };
        const pictures = await renderForExport(edit.input_overlay, props.inputs, ranges, size.width, size.height, colors);
        for (const picture of pictures) {
          const stored = await call<{ path: string }>("cache_write", { source: source.path, name: picture.name, base64: picture.png });
          overlay.push({ path: stored.path, x: picture.x, y: picture.y, spans: picture.spans });
        }
      }

      const plan = buildPlan(source, ranges, options, enc, out.path, overlay);
      let script: string | null = null;
      if (plan.filter.length > FILTER_SCRIPT_FROM) {
        script = (await call<{ path: string }>("cache_write", { source: source.path, name: "export_filter.txt", text: plan.filter })).path;
      }
      const handle = await startJob({ args: ["-loglevel", "error", ...planArgs(plan, script)], total_ms: plan.totalMs, outputs: plan.outputs }, (status) =>
        setPhase((p) => (p.step === "running" ? { ...p, status, plan } : p)),
      );
      job.current = handle;
      const status = await handle.done;
      job.current = null;
      if (status.state === "cancelled") return setPhase({ step: "options" });
      if (status.state !== "done") return setPhase({ step: "failed", message: lastLines(status.log, 12) || t("api.ffmpegCode", String(status.code)) });

      const sizes = await Promise.all(plan.outputs.map(async (p) => (await call<{ size: number }>("file_info", { path: p })).size));
      if (mode === "newclip") {
        // The new clip gets its own sidecar: new length, events moved to the new timeline.
        const base: ClipMeta = clip.item.meta ?? (await metaFromProbe(clip.item));
        const events = base.events
          .map((e) => ({ event: e, at: sourceToOutput(ranges, e.at_ms) }))
          .filter((x) => x.at !== null)
          .map((x) => ({ ...x.event, at_ms: Math.round(x.at!) }));
        const meta: ClipMeta = { ...base, duration_ms: plan.totalMs, events, favorite: false };
        if (plan.video === "encode" && plan.encoder) {
          meta.encoder = `ffmpeg ${plan.encoder}`;
          meta.codec = options.codec;
        }
        await call("meta_write", { path: out.path, meta });
      } else if (dir.trim() !== (settings.export_dir || env.export_dir)) {
        void saveSettings({ ...settings, export_dir: dir.trim() });
      }
      setPhase({ step: "done", plan, sizes, seconds: (performance.now() - began) / 1000 });
    } catch (error) {
      job.current = null;
      setPhase({ step: "failed", message: errorText(error) });
    }
  };

  // "Save as new clip" has nothing to choose, it starts right away.
  useEffect(() => {
    if (mode === "newclip" && !started.current && preview.plan) {
      started.current = true;
      void run();
    }
    // Starts exactly once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, preview.plan]);

  const close = () => {
    if (phase.step === "running") return;
    props.onClose(mode === "newclip" && phase.step === "done");
  };
  const title = mode === "newclip" ? t("ex.saveAsNew") : t("ex.export");

  if (phase.step === "running") {
    const status = phase.status;
    return (
      <Modal title={title} onClose={close} locked footer={<Button icon={<X size={15} />} onClick={() => void job.current?.cancel()}>{t("common.cancel")}</Button>}>
        <div className="export-progress">
          <span className="big">{Math.round((status?.progress ?? 0) * 100)} %</span>
          <ProgressBar value={status?.progress ?? 0} />
          <span className="muted mono small">
            {t("ex.progressOf", formatDuration(status?.out_ms ?? 0), formatDuration(phase.plan.totalMs))}
            {status?.speed && status.speed !== "N/A" ? ` / ${t("ex.speed", status.speed)}` : ""}
            {phase.plan.video === "copy" ? ` / ${t("ex.noReencode")}` : phase.plan.encoder ? ` / ${phase.plan.encoder}` : ""}
          </span>
        </div>
      </Modal>
    );
  }

  if (phase.step === "failed") {
    return (
      <Modal title={t("ex.failedTitle", title)} onClose={() => props.onClose(false)} footer={<>{mode === "export" ? <Button onClick={() => setPhase({ step: "options" })}>{t("common.back")}</Button> : null}<Button kind="primary" onClick={() => props.onClose(false)}>{t("common.close")}</Button></>}>
        <p>{t("ex.failedText")}</p>
        <div className="export-log">{phase.message}</div>
      </Modal>
    );
  }

  if (phase.step === "done") {
    const outputs = phase.plan.outputs;
    const total = phase.sizes.reduce((a, b) => a + b, 0);
    const limit = options.targetMb !== null ? options.targetMb * 1024 * 1024 : null;
    const copy = () =>
      call("clipboard_files", { paths: outputs }).then(
        () => toast(tn("ex.copied", outputs.length)),
        (e) => toast(errorText(e), "error"),
      );
    return (
      <Modal
        title={mode === "newclip" ? t("ex.newClipSaved") : t("ex.done")}
        onClose={close}
        footer={
          <>
            <Button icon={<ClipboardCopy size={15} />} onClick={() => void copy()}>{t("ex.toClipboard")}</Button>
            <Button icon={<FolderOpen size={15} />} onClick={() => void call("reveal", { path: outputs[0] }).catch((e) => toast(errorText(e), "error"))}>{t("ex.openFolder")}</Button>
            <Button kind="primary" onClick={close}>{t("common.done")}</Button>
          </>
        }
      >
        {outputs.map((p, i) => (
          <div key={p}>
            <div className="result-path">{p}</div>
            <div className="muted small mono">{formatBytes(phase.sizes[i])}</div>
          </div>
        ))}
        <div className="summary">
          <span>
            <Check size={14} className="ok" /> {t("ex.doneIn", formatDuration(phase.plan.totalMs), formatNumber(Math.round(phase.seconds * 10) / 10))}
            {phase.plan.video === "copy" ? `, ${t("ex.noReencode")}` : phase.plan.encoder ? `, ${t("ex.encoderName", phase.plan.encoder)}` : ""}
          </span>
          {limit !== null ? (
            <span className={total <= limit ? "" : "warn-text"}>
              {total <= limit ? t("ex.sizeMet", options.targetMb ?? 0, formatBytes(total)) : t("ex.sizeMissed", options.targetMb ?? 0, formatBytes(total))}
            </span>
          ) : null}
        </div>
      </Modal>
    );
  }

  if (mode === "newclip") {
    // Only reached when the plan could not even be built.
    return (
      <Modal title={title} onClose={() => props.onClose(false)}>
        <p className="error-text">{preview.error ?? t("ex.preparing")}</p>
      </Modal>
    );
  }

  const video = isVideoFormat(format);
  const plan = preview.plan;
  const heights = [2160, 1440, 1080, 720, 480, 360].filter((h) => h < source.height);
  const rates = [60, 30, 24, 15].filter((r) => r < source.fps);
  const canStart = plan !== null && dir.trim() !== "" && (copies || !family || encoder !== null);

  return (
    <Modal
      title={t("ex.export")}
      wide
      onClose={close}
      footer={
        <>
          <Button kind="ghost" onClick={close}>{t("common.cancel")}</Button>
          <Button kind="primary" disabled={!canStart} onClick={() => void run()}>{t("ex.start")}</Button>
        </>
      }
    >
      <div className="format-grid" role="radiogroup" aria-label={t("ex.format")}>
        {FORMATS.map((f) => (
          <button key={f.id} type="button" role="radio" aria-checked={format === f.id} className={`format-option${format === f.id ? " is-active" : ""}`} onClick={() => setFormat(f.id)}>
            <strong>{f.label}</strong>
            <span>{t(f.note)}</span>
          </button>
        ))}
      </div>

      <div className="export-cols">
        <div>
          <h3>{t("tl.picture")}</h3>
          {video ? (
            <>
              <div className="field">
                <span>{t("rec.resolution")}</span>
                <Select<number> label={t("rec.resolution")} value={height ?? 0} onChange={(h) => setHeight(h === 0 ? null : h)} options={[{ value: 0, label: format === "gif" ? t("ex.gifHeight") : t("ex.original", `${source.height}p`) }, ...heights.map((h) => ({ value: h, label: `${h}p` }))]} />
              </div>
              <div className="field">
                <span>{t("rec.fps")}</span>
                <Select<number> label={t("rec.fps")} value={fps ?? 0} onChange={(r) => setFps(r === 0 ? null : r)} options={[{ value: 0, label: format === "gif" ? t("ex.gifFps") : t("ex.original", `${source.fps} fps`) }, ...rates.map((r) => ({ value: r, label: `${r} fps` }))]} />
              </div>
              {format === "mp4" || format === "mkv" ? (
                <div className="field">
                  <span>Codec</span>
                  <Select<"h264" | "hevc"> label="Codec" value={codec} disabled={copies} onChange={setCodec} options={[{ value: "h264", label: t("ex.h264") }, { value: "hevc", label: t("ex.hevc") }]} />
                </div>
              ) : null}
              {format !== "gif" ? (
                <div className="field">
                  <span>{t("ex.quality")}</span>
                  <Select<Quality> label={t("ex.quality")} value={quality} disabled={copies || sized} onChange={setQuality} options={[{ value: "high", label: t("ex.qHigh") }, { value: "medium", label: t("ex.qMedium") }, { value: "low", label: t("ex.qLow") }]} />
                </div>
              ) : null}
              {overlayWanted ? (
                <div className="field">
                  <span>{t("ex.burnOverlay")}</span>
                  <Toggle checked={withOverlay} label={t("ex.burnOverlay")} onChange={setWithOverlay} />
                </div>
              ) : null}
              {format !== "gif" ? (
                <div className="field">
                  <span title={blocker ?? ""}>{t("ex.copyIfPossible")}</span>
                  <Toggle checked={allowCopy} label={t("ex.copyIfPossible")} onChange={setAllowCopy} />
                </div>
              ) : null}
            </>
          ) : (
            <p className="muted small">{t("ex.audioOnly")}</p>
          )}
        </div>

        <div>
          <h3>{t("ex.sound")}</h3>
          {format === "gif" ? (
            <p className="muted small">{t("ex.gifNoSound")}</p>
          ) : props.tracks.length === 0 ? (
            <p className="muted small">{t("ex.noTrack")}</p>
          ) : (
            <>
              <div className="field">
                <span>{t("audio.tracks")}</span>
                <Select<AudioMode>
                  label={t("audio.tracks")}
                  value={audioMode}
                  onChange={setAudioMode}
                  options={[
                    { value: "mix", label: t("ex.aMix") },
                    { value: "separate", label: video ? (format === "webm" || format === "mp4" || format === "mkv" ? t("ex.aSeparateInFile") : t("ex.aSeparate")) : t("ex.aSeparateFiles") },
                    ...(video ? [{ value: "none" as AudioMode, label: t("ex.aNone") }] : []),
                  ]}
                />
              </div>
              {audioMode !== "none"
                ? props.tracks.map((tr) => {
                    const volume = edit.tracks.find((e) => e.stream === tr.stream)?.volume ?? 1;
                    return (
                      <label key={tr.stream} className="check">
                        <input
                          type="checkbox"
                          checked={chosen.has(tr.stream)}
                          onChange={(e) => {
                            const next = new Set(chosen);
                            if (e.target.checked) next.add(tr.stream);
                            else next.delete(tr.stream);
                            setChosen(next);
                          }}
                        />
                        <span>{tr.name}</span>
                        <span className="vol mono">{Math.round(volume * 100)} %</span>
                      </label>
                    );
                  })
                : null}
            </>
          )}

          <h3>{t("ex.targetSize")}</h3>
          {format === "gif" || format === "wav" ? (
            <p className="muted small">{t("ex.noSizeFor", format.toUpperCase())}</p>
          ) : (
            <>
              <div className="field">
                <span>{t("ex.sizeLimit")}</span>
                <Toggle checked={sized} label={t("ex.targetSize")} onChange={setSized} />
              </div>
              {sized ? (
                <div className="field">
                  <span className="swatches">
                    {SIZE_PRESETS.map((mb) => (
                      <Chip key={mb} on={mb === targetMb} onClick={() => setTargetMb(mb)}>
                        {mb} MB
                      </Chip>
                    ))}
                  </span>
                  <NumberField label={t("ex.targetSizeLabel")} value={targetMb} min={1} max={100000} unit="MB" onChange={setTargetMb} />
                </div>
              ) : null}
            </>
          )}
        </div>
      </div>

      <div className="export-section">{t("ex.target")}</div>
      <div className="field">
        <input className="input export-name" aria-label={t("ex.fileName")} value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
        <PathField label={t("ex.targetDir")} value={dir} pick="folder" onChange={setDir} />
      </div>

      <div className="summary">
        {preview.error ? (
          <span className="warn-text">
            <AlertTriangle size={14} /> {preview.error}
          </span>
        ) : plan ? (
          <>
            <span>
              <strong>{formatDuration(totalMs)}</strong>
              {video ? `, ${outputSize(source, options).width}x${outputSize(source, options).height}` : ""}
              {ranges.length > 1 ? `, ${t("ex.nSections", ranges.length)}` : ""}
            </span>
            {video ? (
              <span>
                {t("tl.picture")}:{" "}
                {plan.video === "copy"
                  ? t("ex.sCopy")
                  : format === "gif"
                    ? t("ex.sGif")
                    : encoderError
                      ? encoderError
                      : encoder
                        ? `${encoder} (${isSoftware(encoder) ? t("ex.sSoftware") : t("ex.sHardware")})${plan.videoKbps ? `, ${plan.videoKbps} kbit/s` : ""}`
                        : t("ex.sTesting")}
              </span>
            ) : null}
            {plan.video !== "copy" && allowCopy && blocker && video && format !== "gif" ? <span className="muted">{t("ex.sReencode", blocker)}</span> : null}
            <span>
              {t("ex.sound")}: {plan.audio === "none" ? t("ex.sNone") : plan.audio === "copy" ? tn("ex.sCopied", options.tracks.length) : audioMode === "mix" || options.tracks.length === 1 ? tn("ex.sMix", options.tracks.length) : t("ex.sSeparate", options.tracks.length)}
              {plan.audioKbps && plan.audio === "encode" && format !== "wav" ? `, ${plan.audioKbps} kbit/s` : ""}
            </span>
            {plan.warnings.map((w) => (
              <span key={w} className="warn-text">
                <AlertTriangle size={14} /> {w}
              </span>
            ))}
          </>
        ) : null}
      </div>
    </Modal>
  );
}
