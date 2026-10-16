// Klip Cloud. One small HTTP server: Discord sign in, a device link for the Klip app on
// the PC, the clip API, and the web page for the phone. No framework, only Node.

import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { loadConfig } from "./config.js";
import { hash, now, openDb, randomToken } from "./db.js";
import { discordLoginUrl, discordUser, googleEnabled, googleLinkUrl, googleRefreshToken } from "./oauth.js";
import { deleteFromDrive, moveToDrive, proxyDrive, receiveToFile, sendFile, usedOnServer, userDir } from "./storage.js";

const WEB_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "web");
const SESSION_COOKIE = "klip_session";
const STATE_COOKIE = "klip_state";
const DEVICE_MINUTES = 10;
const STATIC_TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml", ".png": "image/png", ".woff2": "font/woff2", ".webmanifest": "application/manifest+json" };

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

function json(res, status, body, headers = {}) {
  res.writeHead(status, { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store", ...headers });
  res.end(JSON.stringify(body));
}

function cookies(req) {
  const out = {};
  for (const part of (req.headers.cookie || "").split(";")) {
    const at = part.indexOf("=");
    if (at > 0) out[part.slice(0, at).trim()] = decodeURIComponent(part.slice(at + 1).trim());
  }
  return out;
}

function cookie(cfg, name, value, maxAgeSeconds) {
  const secure = cfg.baseUrl.startsWith("https:") ? "; Secure" : "";
  return `${name}=${encodeURIComponent(value)}; Path=${cfg.basePath || "/"}; HttpOnly; SameSite=Lax; Max-Age=${maxAgeSeconds}${secure}`;
}

async function readJson(req, limit = 64 * 1024) {
  let body = "";
  for await (const chunk of req) {
    body += chunk;
    if (body.length > limit) throw new HttpError(413, "request too large");
  }
  try {
    return body ? JSON.parse(body) : {};
  } catch {
    throw new HttpError(400, "not JSON");
  }
}

/** A clip id: short, random, safe in paths and URLs. */
function clipId() {
  return crypto.randomBytes(9).toString("base64url");
}

function publicUser(u) {
  return {
    id: u.id,
    name: u.name,
    avatar: u.avatar,
    storage: u.storage,
    google_linked: Boolean(u.google_refresh),
    clips_made: u.clips_made ?? 0,
    on_board: u.on_board !== 0,
  };
}

function publicClip(c) {
  return {
    id: c.id,
    name: c.name,
    kind: c.kind,
    game: c.game,
    reason: c.reason,
    created: c.created,
    duration_ms: c.duration_ms,
    width: c.width,
    height: c.height,
    size: c.size,
    storage: c.storage,
    has_thumb: Boolean(c.thumb),
  };
}

export function createServer(cfg = loadConfig()) {
  const db = openDb(cfg.dataDir);

  const userBySession = (req) => {
    const auth = req.headers.authorization || "";
    const bearer = auth.startsWith("Bearer ") ? auth.slice(7).trim() : "";
    const token = bearer || cookies(req)[SESSION_COOKIE] || "";
    if (!token) return null;
    const row = db.prepare("SELECT s.token_hash, s.kind, u.* FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?").get(hash(token));
    if (!row) return null;
    db.prepare("UPDATE sessions SET last_used = ? WHERE token_hash = ?").run(now(), row.token_hash);
    return { user: row, viaCookie: !bearer, sessionHash: row.token_hash };
  };

  const need = (req) => {
    const who = userBySession(req);
    if (!who) throw new HttpError(401, "not signed in");
    // A changing request from the web page must carry this header. Other sites cannot
    // send it without asking first, which is the whole protection against forged posts.
    if (who.viaCookie && req.method !== "GET" && req.method !== "HEAD" && req.headers["x-klip-web"] !== "1") {
      throw new HttpError(403, "missing header");
    }
    return who;
  };

  const newSession = (userId, kind, label) => {
    const token = randomToken();
    db.prepare("INSERT INTO sessions (token_hash, user_id, kind, label, created, last_used) VALUES (?, ?, ?, ?, ?, ?)").run(hash(token), userId, kind, label, now(), now());
    return token;
  };

  const clipOf = (userId, id) => {
    const c = db.prepare("SELECT * FROM clips WHERE id = ? AND user_id = ?").get(id, userId);
    if (!c) throw new HttpError(404, "no such clip");
    return c;
  };

  const sendStatic = (res, rel) => {
    const file = path.normalize(path.join(WEB_DIR, rel));
    if (!file.startsWith(WEB_DIR) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return false;
    const type = STATIC_TYPES[path.extname(file)] || "application/octet-stream";
    let body = fs.readFileSync(file);
    // The page learns its own base path, the server can live under any prefix.
    if (rel === "index.html") body = Buffer.from(body.toString("utf8").replaceAll("__BASE__", cfg.basePath));
    res.writeHead(200, { "Content-Type": type, "Cache-Control": rel === "index.html" ? "no-cache" : "public, max-age=3600", "X-Content-Type-Options": "nosniff" });
    res.end(body);
    return true;
  };

  async function route(req, res) {
    const url = new URL(req.url, "http://x");
    let p = url.pathname;
    if (cfg.basePath && p.startsWith(cfg.basePath)) p = p.slice(cfg.basePath.length) || "/";
    const m = req.method;
    const seg = p.split("/").filter(Boolean);

    // ---- sign in with Discord ----
    if (m === "GET" && p === "/auth/discord") {
      if (!cfg.discord.fake && !cfg.discord.clientId) throw new HttpError(503, "Discord sign in is not set up on this server");
      const next = url.searchParams.get("next") || "/";
      const state = `${randomToken(16)}|${next.startsWith("/") ? next : "/"}`;
      res.writeHead(302, { Location: discordLoginUrl(cfg, state.split("|")[0], url.searchParams.get("as") || ""), "Set-Cookie": cookie(cfg, STATE_COOKIE, state, 600) });
      return res.end();
    }
    if (m === "GET" && p === "/auth/discord/callback") {
      const [expect, next = "/"] = (cookies(req)[STATE_COOKIE] || "").split("|");
      if (!expect || url.searchParams.get("state") !== expect) throw new HttpError(400, "the sign in took too long or came from elsewhere, try again");
      const code = url.searchParams.get("code");
      if (!code) throw new HttpError(400, "Discord sent no code");
      const d = await discordUser(cfg, code);
      if (cfg.allowedUsers.length > 0 && !cfg.allowedUsers.includes(d.id)) throw new HttpError(403, "this Discord account may not use this server");
      db.prepare(
        "INSERT INTO users (discord_id, name, avatar, created) VALUES (?, ?, ?, ?) ON CONFLICT(discord_id) DO UPDATE SET name = excluded.name, avatar = excluded.avatar",
      ).run(d.id, d.name, d.avatar, now());
      const user = db.prepare("SELECT * FROM users WHERE discord_id = ?").get(d.id);
      const token = newSession(user.id, "web", req.headers["user-agent"]?.slice(0, 80) || "Browser");
      res.writeHead(302, {
        Location: `${cfg.basePath}${next}`,
        "Set-Cookie": [cookie(cfg, SESSION_COOKIE, token, 60 * 60 * 24 * 180), cookie(cfg, STATE_COOKIE, "", 0)],
      });
      return res.end();
    }
    if (m === "POST" && p === "/auth/logout") {
      const who = need(req);
      db.prepare("DELETE FROM sessions WHERE token_hash = ?").run(who.sessionHash);
      return json(res, 200, { ok: true }, { "Set-Cookie": cookie(cfg, SESSION_COOKIE, "", 0) });
    }

    // ---- link a Google Drive ----
    if (m === "GET" && p === "/auth/google") {
      const { user } = need(req);
      if (!googleEnabled(cfg)) throw new HttpError(503, "Google Drive is not set up on this server");
      const state = randomToken(16);
      res.writeHead(302, { Location: googleLinkUrl(cfg, state), "Set-Cookie": cookie(cfg, STATE_COOKIE, `${state}|g${user.id}`, 600) });
      return res.end();
    }
    if (m === "GET" && p === "/auth/google/callback") {
      const { user } = need(req);
      const [expect, tag] = (cookies(req)[STATE_COOKIE] || "").split("|");
      if (!expect || url.searchParams.get("state") !== expect || tag !== `g${user.id}`) throw new HttpError(400, "the link took too long, try again");
      const refresh = await googleRefreshToken(cfg, url.searchParams.get("code") || "");
      db.prepare("UPDATE users SET google_refresh = ?, storage = 'gdrive' WHERE id = ?").run(refresh, user.id);
      res.writeHead(302, { Location: `${cfg.basePath}/settings`, "Set-Cookie": cookie(cfg, STATE_COOKIE, "", 0) });
      return res.end();
    }

    // ---- the Klip app on a PC: device link ----
    if (m === "POST" && p === "/api/device") {
      const body = await readJson(req);
      const device = randomToken();
      const userCode = `${crypto.randomBytes(2).toString("hex")}-${crypto.randomBytes(2).toString("hex")}`.toUpperCase();
      db.prepare("DELETE FROM device_codes WHERE expires < ?").run(now());
      db.prepare("INSERT INTO device_codes (device_hash, user_code, label, expires) VALUES (?, ?, ?, ?)").run(hash(device), userCode, String(body.label || "Klip").slice(0, 60), now() + DEVICE_MINUTES * 60_000);
      return json(res, 200, { device_code: device, user_code: userCode, verify_url: `${cfg.baseUrl}/link?code=${userCode}`, interval: 3, expires_in: DEVICE_MINUTES * 60 });
    }
    if (m === "POST" && p === "/api/device/confirm") {
      const { user } = need(req);
      const body = await readJson(req);
      const row = db.prepare("SELECT * FROM device_codes WHERE user_code = ? AND expires > ?").get(String(body.user_code || "").toUpperCase(), now());
      if (!row) throw new HttpError(404, "this code is unknown or expired, start the link in Klip again");
      if (row.user_id && row.user_id !== user.id) throw new HttpError(409, "this code belongs to another account");
      const token = newSession(user.id, "device", row.label);
      db.prepare("UPDATE device_codes SET user_id = ?, token = ? WHERE device_hash = ?").run(user.id, token, row.device_hash);
      return json(res, 200, { ok: true, label: row.label });
    }
    if (m === "POST" && p === "/api/device/token") {
      const body = await readJson(req);
      const row = db.prepare("SELECT * FROM device_codes WHERE device_hash = ?").get(hash(String(body.device_code || "")));
      if (!row || row.expires < now()) throw new HttpError(410, "the link expired");
      if (!row.token) return json(res, 202, { pending: true });
      // Handed out once, then the code is gone.
      db.prepare("DELETE FROM device_codes WHERE device_hash = ?").run(row.device_hash);
      const user = db.prepare("SELECT * FROM users WHERE id = ?").get(row.user_id);
      return json(res, 200, { token: row.token, user: publicUser(user) });
    }

    // ---- account ----
    if (m === "GET" && p === "/api/me") {
      const who = userBySession(req);
      if (!who) return json(res, 200, { user: null, discord: Boolean(cfg.discord.clientId || cfg.discord.fake), google: googleEnabled(cfg) });
      const u = who.user;
      const sessions = db.prepare("SELECT token_hash, kind, label, created, last_used FROM sessions WHERE user_id = ? ORDER BY last_used DESC").all(u.id);
      return json(res, 200, {
        user: publicUser(u),
        google: googleEnabled(cfg),
        uploads: cfg.uploads,
        used: usedOnServer(db, u.id),
        quota: cfg.quotaBytes,
        clips: db.prepare("SELECT COUNT(*) AS n FROM clips WHERE user_id = ?").get(u.id).n,
        sessions: sessions.map((s) => ({ id: s.token_hash.slice(0, 12), kind: s.kind, label: s.label, created: s.created, last_used: s.last_used, current: s.token_hash === who.sessionHash })),
      });
    }
    if (m === "PUT" && p === "/api/me/storage") {
      const { user } = need(req);
      const body = await readJson(req);
      const storage = body.storage === "gdrive" ? "gdrive" : "server";
      if (storage === "gdrive" && !user.google_refresh) throw new HttpError(409, "link a Google Drive first");
      db.prepare("UPDATE users SET storage = ? WHERE id = ?").run(storage, user.id);
      return json(res, 200, { storage });
    }
    if (m === "DELETE" && seg[0] === "api" && seg[1] === "sessions" && seg[2]) {
      const { user } = need(req);
      db.prepare("DELETE FROM sessions WHERE user_id = ? AND substr(token_hash, 1, 12) = ?").run(user.id, seg[2]);
      return json(res, 200, { ok: true });
    }

    // ---- leaderboard: how many clips each signed in account made, nothing else ----
    if (m === "GET" && p === "/api/leaderboard") {
      const rows = db
        .prepare("SELECT name, avatar, clips_made FROM users WHERE on_board = 1 AND clips_made > 0 ORDER BY clips_made DESC, id ASC LIMIT 50")
        .all();
      return json(res, 200, { board: rows.map((r, i) => ({ rank: i + 1, name: r.name, avatar: r.avatar, clips: r.clips_made })) }, { "Cache-Control": "public, max-age=30" });
    }
    if (m === "POST" && p === "/api/stats") {
      const { user } = need(req);
      const body = await readJson(req);
      // A PC sends what it saved since its last report. More than this in one go is not
      // a person pressing a key.
      const clips = Math.max(0, Math.min(200, Math.floor(Number(body.clips) || 0)));
      // At most one report per ten seconds and account.
      if (now() - (user.stats_at || 0) < 10_000) throw new HttpError(429, "too many reports, try again in a moment");
      db.prepare("UPDATE users SET clips_made = clips_made + ?, stats_at = ? WHERE id = ?").run(clips, now(), user.id);
      return json(res, 200, { clips_made: (user.clips_made || 0) + clips });
    }
    if (m === "PUT" && p === "/api/me/board") {
      const { user } = need(req);
      const body = await readJson(req);
      db.prepare("UPDATE users SET on_board = ? WHERE id = ?").run(body.show === false ? 0 : 1, user.id);
      return json(res, 200, { on_board: body.show !== false });
    }

    // ---- clips ----
    if (m === "GET" && p === "/api/clips") {
      const { user } = need(req);
      const rows = db.prepare("SELECT * FROM clips WHERE user_id = ? ORDER BY created DESC").all(user.id);
      return json(res, 200, { clips: rows.map(publicClip) });
    }
    if (m === "PUT" && p === "/api/clips") {
      const { user } = need(req);
      if (!cfg.uploads) throw new HttpError(403, "uploads open soon, the cloud is not ready yet");
      const length = Number(req.headers["content-length"] || 0);
      if (!length) throw new HttpError(411, "the size of the clip is missing");
      if (length > cfg.maxUploadBytes) throw new HttpError(413, "the clip is larger than this server takes");
      let meta = {};
      try {
        meta = JSON.parse(Buffer.from(String(req.headers["x-klip-meta"] || ""), "base64").toString("utf8") || "{}");
      } catch {
        meta = {};
      }
      let name = String(req.headers["x-klip-name"] || meta.name || "Clip");
      try {
        // Klip sends letters outside ASCII as percent escapes, headers are ASCII only.
        name = decodeURIComponent(name);
      } catch {
        // A stray percent sign stays as it is.
      }
      name = name.slice(0, 160);
      const toDrive = user.storage === "gdrive" && user.google_refresh && googleEnabled(cfg);
      if (!toDrive && cfg.quotaBytes > 0 && usedOnServer(db, user.id) + length > cfg.quotaBytes) throw new HttpError(507, "your space on this server is full");
      const id = clipId();
      const file = path.join(userDir(cfg, user.id), `${id}.mp4`);
      const size = await receiveToFile(req, file, cfg.maxUploadBytes);
      let storage = "server";
      let location = file;
      if (toDrive) {
        location = await moveToDrive(cfg, db, user, file, `${name}.mp4`, "video/mp4");
        storage = "gdrive";
      }
      db.prepare(
        "INSERT INTO clips (id, user_id, name, kind, game, reason, created, duration_ms, width, height, size, storage, location, uploaded) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
      ).run(id, user.id, name, String(meta.kind || "clip"), meta.game ?? null, meta.reason ?? null, String(meta.created || new Date().toISOString()), Number(meta.duration_ms) || 0, Number(meta.width) || 0, Number(meta.height) || 0, size, storage, location, now());
      return json(res, 201, { id, storage, size });
    }
    if (seg[0] === "api" && seg[1] === "clips" && seg[2]) {
      const { user } = need(req);
      const clip = clipOf(user.id, seg[2]);
      if (m === "PUT" && seg[3] === "thumb") {
        const file = path.join(userDir(cfg, user.id), `${clip.id}.jpg`);
        await receiveToFile(req, file, 2 * 1024 * 1024);
        db.prepare("UPDATE clips SET thumb = ? WHERE id = ?").run(file, clip.id);
        return json(res, 200, { ok: true });
      }
      if ((m === "GET" || m === "HEAD") && seg[3] === "thumb") {
        if (!clip.thumb) throw new HttpError(404, "no picture");
        return sendFile(req, res, clip.thumb, "image/jpeg");
      }
      if ((m === "GET" || m === "HEAD") && seg[3] === "video") {
        if (clip.storage === "gdrive") return proxyDrive(cfg, user, clip.location, req, res, "video/mp4");
        return sendFile(req, res, clip.location, "video/mp4");
      }
      if (m === "DELETE" && !seg[3]) {
        if (clip.storage === "gdrive") await deleteFromDrive(cfg, user, clip.location).catch(() => {});
        else fs.rmSync(clip.location, { force: true });
        if (clip.thumb) fs.rmSync(clip.thumb, { force: true });
        db.prepare("DELETE FROM clips WHERE id = ?").run(clip.id);
        return json(res, 200, { ok: true });
      }
    }

    // ---- the web page ----
    if (m === "GET" || m === "HEAD") {
      if (p.startsWith("/assets/") && sendStatic(res, p.slice(1))) return;
      if (!p.startsWith("/api/") && !p.startsWith("/auth/") && sendStatic(res, "index.html")) return;
    }
    throw new HttpError(404, "not found");
  }

  const server = http.createServer((req, res) => {
    route(req, res).catch((error) => {
      const status = error instanceof HttpError ? error.status : 500;
      if (status === 500) console.error(error);
      if (res.headersSent) return res.destroy();
      json(res, status, { error: status === 500 ? "something went wrong on the server" : error.message });
    });
  });
  server.db = db;
  return server;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const cfg = loadConfig();
  createServer(cfg).listen(cfg.port, cfg.host, () => {
    console.log(`klip-cloud on http://${cfg.host}:${cfg.port}, public as ${cfg.baseUrl}`);
    if (!cfg.discord.clientId && !cfg.discord.fake) console.log("Discord sign in is not set up: DISCORD_CLIENT_ID and DISCORD_CLIENT_SECRET are missing");
  });
}
