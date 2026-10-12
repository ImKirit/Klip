// The only place that knows how the UI reaches the backend. Inside the Tauri window it
// uses the IPC command `klip`, in a plain browser it posts to the test backend. Both end
// in the same Rust function, so the browser preview exercises the real code.

import { t } from "./i18n";
import type { JobStatus } from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
let invokePromise: Promise<Invoke> | null = null;

function tauriInvoke(): Promise<Invoke> {
  invokePromise ??= import("@tauri-apps/api/core").then((m) => m.invoke as Invoke);
  return invokePromise;
}

let convert: ((path: string) => string) | null = null;
if (isTauri) {
  void import("@tauri-apps/api/core").then((m) => {
    convert = m.convertFileSrc;
  });
}

export async function call<T = unknown>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isTauri) {
    const invoke = await tauriInvoke();
    return (await invoke("klip", { cmd, args })) as T;
  }
  const response = await fetch("/__klip/invoke", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ cmd, args }),
  });
  if (!response.ok) throw new Error(t("api.noBackend", response.status));
  const payload = (await response.json()) as { ok?: T; error?: string };
  if (payload.error !== undefined) throw new Error(payload.error);
  return payload.ok as T;
}

/** URL the webview can load a local file from. */
export function fileUrl(path: string): string {
  if (isTauri) {
    if (convert) return convert(path);
    // Same result as convertFileSrc on Windows, used before the import has settled.
    return `http://asset.localhost/${encodeURIComponent(path)}`;
  }
  return `/__klip/file?p=${encodeURIComponent(path)}`;
}

export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : JSON.stringify(error);
}

export interface JobHandle {
  id: number;
  cancel: () => Promise<void>;
  done: Promise<JobStatus>;
}

interface JobRequest {
  args: string[];
  total_ms?: number;
  /** Removed again if the job fails or is cancelled. */
  outputs?: string[];
  /** Pairs of [from, to], renamed once the job has succeeded. */
  renames?: [string, string][];
}

/** Starts ffmpeg as a job and polls its progress four times a second. */
export async function startJob(request: JobRequest, onProgress?: (status: JobStatus) => void): Promise<JobHandle> {
  const { id } = await call<{ id: number }>("job_start", { ...request });
  const done = new Promise<JobStatus>((resolve, reject) => {
    const poll = async () => {
      try {
        const status = await call<JobStatus>("job_status", { id });
        onProgress?.(status);
        if (status.state === "running") {
          window.setTimeout(poll, 250);
        } else {
          resolve(status);
        }
      } catch (error) {
        reject(error);
      }
    };
    void poll();
  });
  return { id, done, cancel: () => call<void>("job_cancel", { id }) };
}

/** Runs a job to the end and throws with the ffmpeg messages if it did not succeed. */
export async function runJob(request: JobRequest, onProgress?: (status: JobStatus) => void): Promise<void> {
  const job = await startJob(request, onProgress);
  const status = await job.done;
  if (status.state !== "done") {
    throw new Error(lastLines(status.log, 6) || t("api.ffmpegCode", String(status.code)));
  }
}

export function lastLines(text: string, count: number): string {
  return text.trim().split("\n").slice(-count).join("\n");
}

/** Native folder picker. Only exists in the real app, the browser preview types paths. */
export async function pickFolder(start?: string): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false, defaultPath: start || undefined });
  return typeof picked === "string" ? picked : null;
}

export async function pickFile(start: string | undefined, name: string, extensions: string[]): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, defaultPath: start || undefined, filters: [{ name, extensions }] });
  return typeof picked === "string" ? picked : null;
}

export async function pickSaveFile(start: string, name: string, extensions: string[]): Promise<string | null> {
  if (!isTauri) return null;
  const { save } = await import("@tauri-apps/plugin-dialog");
  return await save({ defaultPath: start, filters: [{ name, extensions }] });
}

/** Hands files to the system drag and drop, so they can be dropped into other programs. */
export async function dragFilesOut(paths: string[], icon: string): Promise<void> {
  if (!isTauri) return;
  const { startDrag } = await import("@crabnebula/tauri-plugin-drag");
  await startDrag({ item: paths, icon });
}

export async function quitApp(): Promise<void> {
  if (!isTauri) return;
  const invoke = await tauriInvoke();
  await invoke("quit");
}
