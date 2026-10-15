// Accounts, sign ins, device links and clips in one SQLite file. Video files live next to
// it on disk or in the Google Drive of the account, never in the database.

import { DatabaseSync } from "node:sqlite";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

export function hash(token) {
  return crypto.createHash("sha256").update(token).digest("hex");
}

export function randomToken(bytes = 32) {
  return crypto.randomBytes(bytes).toString("base64url");
}

export function openDb(dataDir) {
  fs.mkdirSync(dataDir, { recursive: true });
  const db = new DatabaseSync(path.join(dataDir, "klip-cloud.sqlite"));
  db.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    CREATE TABLE IF NOT EXISTS users (
      id INTEGER PRIMARY KEY,
      discord_id TEXT UNIQUE NOT NULL,
      name TEXT NOT NULL,
      avatar TEXT,
      created INTEGER NOT NULL,
      storage TEXT NOT NULL DEFAULT 'server',
      google_refresh TEXT,
      google_folder TEXT
    );
    CREATE TABLE IF NOT EXISTS sessions (
      token_hash TEXT PRIMARY KEY,
      user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      kind TEXT NOT NULL,
      label TEXT NOT NULL DEFAULT '',
      created INTEGER NOT NULL,
      last_used INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS device_codes (
      device_hash TEXT PRIMARY KEY,
      user_code TEXT UNIQUE NOT NULL,
      label TEXT NOT NULL DEFAULT '',
      user_id INTEGER REFERENCES users(id) ON DELETE CASCADE,
      token TEXT,
      expires INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS clips (
      id TEXT PRIMARY KEY,
      user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
      name TEXT NOT NULL,
      kind TEXT NOT NULL DEFAULT 'clip',
      game TEXT,
      reason TEXT,
      created TEXT NOT NULL,
      duration_ms INTEGER NOT NULL DEFAULT 0,
      width INTEGER NOT NULL DEFAULT 0,
      height INTEGER NOT NULL DEFAULT 0,
      size INTEGER NOT NULL DEFAULT 0,
      storage TEXT NOT NULL,
      location TEXT NOT NULL,
      thumb TEXT,
      uploaded INTEGER NOT NULL,
      sha256 TEXT
    );
    CREATE INDEX IF NOT EXISTS clips_by_user ON clips(user_id, created);
  `);
  return db;
}

export function now() {
  return Date.now();
}
