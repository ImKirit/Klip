// Settings of the server, all from the environment (see cloud/klip-cloud.env.example).

import path from "node:path";

function int(name, fallback, min = 1) {
  const raw = process.env[name];
  const v = Number(raw);
  return raw !== undefined && raw !== "" && Number.isFinite(v) && v >= min ? v : fallback;
}

export function loadConfig(env = process.env) {
  const base = (env.KLIP_BASE_URL || "http://127.0.0.1:3160").replace(/\/+$/, "");
  const basePath = new URL(base).pathname.replace(/\/+$/, "");
  return {
    port: int("KLIP_PORT", 3160),
    host: env.KLIP_HOST || "127.0.0.1",
    // Public address of the server, with path, without a slash at the end.
    baseUrl: base,
    basePath,
    dataDir: path.resolve(env.KLIP_DATA_DIR || "./data"),
    discord: {
      clientId: env.DISCORD_CLIENT_ID || "",
      clientSecret: env.DISCORD_CLIENT_SECRET || "",
      // Tests replace Discord by a fake that answers without the network.
      fake: env.KLIP_FAKE_DISCORD === "1",
    },
    google: {
      clientId: env.GOOGLE_CLIENT_ID || "",
      clientSecret: env.GOOGLE_CLIENT_SECRET || "",
    },
    // Space per account on this server, in bytes. 0 means no limit.
    quotaBytes: int("KLIP_QUOTA_GB", 20, 0) * 1024 ** 3,
    maxUploadBytes: int("KLIP_MAX_UPLOAD_MB", 2048) * 1024 ** 2,
    // "0" keeps sign in working but refuses clips, while the cloud is announced as coming soon.
    uploads: env.KLIP_UPLOADS !== "0",
    // Only these Discord accounts may sign in. Empty means everyone with a Discord account.
    allowedUsers: (env.KLIP_ALLOWED_DISCORD_IDS || "").split(",").map((s) => s.trim()).filter(Boolean),
  };
}
