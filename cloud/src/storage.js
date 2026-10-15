// Where the video files of an account live: on the disk of this server, or in the
// account's own Google Drive. The database only knows the place.

import fs from "node:fs";
import path from "node:path";
import { pipeline } from "node:stream/promises";
import { googleAccessToken } from "./oauth.js";

const DRIVE_FILES = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD = "https://www.googleapis.com/upload/drive/v3/files";

export function userDir(cfg, userId) {
  return path.join(cfg.dataDir, "clips", String(userId));
}

/** Bytes this account keeps on the disk of the server. */
export function usedOnServer(db, userId) {
  const row = db.prepare("SELECT COALESCE(SUM(size), 0) AS used FROM clips WHERE user_id = ? AND storage = 'server'").get(userId);
  return Number(row.used);
}

/** Streams an upload into a file in the account's folder. Returns the bytes written. */
export async function receiveToFile(req, file, maxBytes) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  let size = 0;
  const tmp = `${file}.part`;
  const out = fs.createWriteStream(tmp);
  req.on("data", (chunk) => {
    size += chunk.length;
    if (size > maxBytes) req.destroy(new Error("too large"));
  });
  try {
    await pipeline(req, out);
  } catch (error) {
    fs.rmSync(tmp, { force: true });
    throw error;
  }
  fs.renameSync(tmp, file);
  return size;
}

/** Serves a file with support for byte ranges, so phones can seek in a video. */
export function sendFile(req, res, file, type) {
  let stat;
  try {
    stat = fs.statSync(file);
  } catch {
    res.writeHead(404).end();
    return;
  }
  const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || "");
  const headers = { "Content-Type": type, "Accept-Ranges": "bytes", "Cache-Control": "private, max-age=3600" };
  if (range && (range[1] || range[2])) {
    let start = range[1] ? Number(range[1]) : stat.size - Number(range[2]);
    let end = range[1] && range[2] ? Number(range[2]) : stat.size - 1;
    start = Math.max(0, start);
    end = Math.min(stat.size - 1, end);
    if (start > end) {
      res.writeHead(416, { "Content-Range": `bytes */${stat.size}` }).end();
      return;
    }
    res.writeHead(206, { ...headers, "Content-Range": `bytes ${start}-${end}/${stat.size}`, "Content-Length": end - start + 1 });
    fs.createReadStream(file, { start, end }).pipe(res);
    return;
  }
  res.writeHead(200, { ...headers, "Content-Length": stat.size });
  if (req.method === "HEAD") return res.end();
  fs.createReadStream(file).pipe(res);
}

// ---- Google Drive ----------------------------------------------------------------

/** The folder "Klip" in the Drive, created on first use. */
async function driveFolder(cfg, db, user, access) {
  if (user.google_folder) return user.google_folder;
  const res = await fetch(DRIVE_FILES, {
    method: "POST",
    headers: { Authorization: `Bearer ${access}`, "Content-Type": "application/json" },
    body: JSON.stringify({ name: "Klip", mimeType: "application/vnd.google-apps.folder" }),
  });
  if (!res.ok) throw new Error(`Drive did not create the folder (${res.status})`);
  const { id } = await res.json();
  db.prepare("UPDATE users SET google_folder = ? WHERE id = ?").run(id, user.id);
  return id;
}

/** Moves a file that arrived on the server into the Drive, then removes it here. */
export async function moveToDrive(cfg, db, user, file, name, type) {
  const access = await googleAccessToken(cfg, user.google_refresh);
  const folder = await driveFolder(cfg, db, user, access);
  const size = fs.statSync(file).size;
  // A resumable session: the file goes up in one request, but large files survive it.
  const start = await fetch(`${DRIVE_UPLOAD}?uploadType=resumable`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${access}`,
      "Content-Type": "application/json; charset=UTF-8",
      "X-Upload-Content-Type": type,
      "X-Upload-Content-Length": String(size),
    },
    body: JSON.stringify({ name, parents: [folder] }),
  });
  if (!start.ok) throw new Error(`Drive refused the upload (${start.status})`);
  const session = start.headers.get("location");
  const put = await fetch(session, {
    method: "PUT",
    headers: { "Content-Length": String(size), "Content-Type": type },
    body: fs.createReadStream(file),
    duplex: "half",
  });
  if (!put.ok) throw new Error(`Drive upload failed (${put.status})`);
  const { id } = await put.json();
  fs.rmSync(file, { force: true });
  return id;
}

/** Passes a file of the Drive through to the browser, ranges included. */
export async function proxyDrive(cfg, user, fileId, req, res, type) {
  const access = await googleAccessToken(cfg, user.google_refresh);
  const headers = { Authorization: `Bearer ${access}` };
  if (req.headers.range) headers.Range = req.headers.range;
  const upstream = await fetch(`${DRIVE_FILES}/${encodeURIComponent(fileId)}?alt=media`, { headers });
  const out = { "Content-Type": type, "Accept-Ranges": "bytes", "Cache-Control": "private, max-age=3600" };
  for (const h of ["content-length", "content-range"]) {
    const v = upstream.headers.get(h);
    if (v) out[h] = v;
  }
  res.writeHead(upstream.status, out);
  if (!upstream.body) return res.end();
  for await (const chunk of upstream.body) res.write(chunk);
  res.end();
}

export async function deleteFromDrive(cfg, user, fileId) {
  const access = await googleAccessToken(cfg, user.google_refresh);
  await fetch(`${DRIVE_FILES}/${encodeURIComponent(fileId)}`, { method: "DELETE", headers: { Authorization: `Bearer ${access}` } });
}
