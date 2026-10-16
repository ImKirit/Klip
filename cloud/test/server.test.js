// The whole path of a clip through Klip Cloud, against a real server on a free port, with
// Discord replaced by a fake that needs no network.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createServer } from "../src/server.js";
import { loadConfig } from "../src/config.js";

let server;
let base;
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "klip-cloud-test-"));

before(async () => {
  server = createServer(loadConfig({ KLIP_DATA_DIR: dataDir, KLIP_FAKE_DISCORD: "1", KLIP_BASE_URL: "http://127.0.0.1:0/klip/cloud", KLIP_QUOTA_GB: "1" }));
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${server.address().port}/klip/cloud`;
});

after(() => {
  server.close();
  server.db.close();
  fs.rmSync(dataDir, { recursive: true, force: true });
});

/** Signs in through the fake Discord and returns the session cookie. */
async function signIn(id = "1000", name = "Tester") {
  const start = await fetch(`${base}/auth/discord?next=/settings`, { redirect: "manual" });
  assert.equal(start.status, 302);
  const state = start.headers.get("set-cookie").split(";")[0];
  const stateValue = decodeURIComponent(state.split("=")[1]).split("|")[0];
  const back = await fetch(`${base}/auth/discord/callback?code=fake:${id}:${name}&state=${encodeURIComponent(stateValue)}`, { redirect: "manual", headers: { Cookie: state } });
  assert.equal(back.status, 302);
  assert.equal(back.headers.get("location"), "/klip/cloud/settings");
  const session = back.headers.getSetCookie().find((c) => c.startsWith("klip_session="));
  assert.ok(session, "a session cookie");
  assert.match(session, /HttpOnly/);
  return session.split(";")[0];
}

test("a signed out visitor gets the page and no data", async () => {
  const page = await fetch(`${base}/`);
  assert.equal(page.status, 200);
  assert.match(await page.text(), /\/klip\/cloud\/assets\/app\.js/);
  const me = await (await fetch(`${base}/api/me`)).json();
  assert.equal(me.user, null);
  assert.equal(me.discord, true);
  assert.equal((await fetch(`${base}/api/clips`)).status, 401);
});

test("a sign in with a forged state is refused", async () => {
  const res = await fetch(`${base}/auth/discord/callback?code=fake&state=abc`, { redirect: "manual", headers: { Cookie: "klip_state=xyz%7C%2F" } });
  assert.equal(res.status, 400);
});

test("device link, upload, list, play with ranges, delete", async () => {
  const web = await signIn();
  // Klip on the PC asks for a code, the browser confirms it, the PC picks up its token.
  const device = await (await fetch(`${base}/api/device`, { method: "POST", body: JSON.stringify({ label: "Klip on DESKTOP-1" }) })).json();
  assert.match(device.user_code, /^[0-9A-F]{4}-[0-9A-F]{4}$/);
  assert.equal((await fetch(`${base}/api/device/token`, { method: "POST", body: JSON.stringify({ device_code: device.device_code }) })).status, 202);
  // Without the header of the page, a cookie alone cannot change anything.
  const forged = await fetch(`${base}/api/device/confirm`, { method: "POST", headers: { Cookie: web }, body: JSON.stringify({ user_code: device.user_code }) });
  assert.equal(forged.status, 403);
  const ok = await fetch(`${base}/api/device/confirm`, { method: "POST", headers: { Cookie: web, "X-Klip-Web": "1" }, body: JSON.stringify({ user_code: device.user_code }) });
  assert.equal(ok.status, 200);
  const picked = await (await fetch(`${base}/api/device/token`, { method: "POST", body: JSON.stringify({ device_code: device.device_code }) })).json();
  assert.ok(picked.token);
  assert.equal(picked.user.name, "Tester");
  // The code works once.
  assert.equal((await fetch(`${base}/api/device/token`, { method: "POST", body: JSON.stringify({ device_code: device.device_code }) })).status, 410);

  const auth = { Authorization: `Bearer ${picked.token}` };
  const video = Buffer.alloc(200_000, 7);
  const meta = Buffer.from(JSON.stringify({ kind: "auto_clip", game: "Valorant", reason: "ace", duration_ms: 12_000, created: "2026-10-15T20:00:00+02:00" })).toString("base64");
  const up = await fetch(`${base}/api/clips`, { method: "PUT", headers: { ...auth, "Content-Length": String(video.length), "X-Klip-Name": "Ace on Ascent", "X-Klip-Meta": meta }, body: video });
  assert.equal(up.status, 201);
  const { id } = await up.json();
  const thumb = await fetch(`${base}/api/clips/${id}/thumb`, { method: "PUT", headers: auth, body: Buffer.from([0xff, 0xd8, 0xff]) });
  assert.equal(thumb.status, 200);

  // The phone sees it with the cookie of the web sign in.
  const list = await (await fetch(`${base}/api/clips`, { headers: { Cookie: web } })).json();
  assert.equal(list.clips.length, 1);
  assert.deepEqual([list.clips[0].name, list.clips[0].game, list.clips[0].reason, list.clips[0].has_thumb], ["Ace on Ascent", "Valorant", "ace", true]);
  const part = await fetch(`${base}/api/clips/${id}/video`, { headers: { Cookie: web, Range: "bytes=100-199" } });
  assert.equal(part.status, 206);
  assert.equal(part.headers.get("content-range"), "bytes 100-199/200000");
  assert.equal((await part.arrayBuffer()).byteLength, 100);

  // Another account sees nothing of it.
  const other = await signIn("2000", "Other");
  assert.equal((await (await fetch(`${base}/api/clips`, { headers: { Cookie: other } })).json()).clips.length, 0);
  assert.equal((await fetch(`${base}/api/clips/${id}/video`, { headers: { Cookie: other } })).status, 404);

  const me = await (await fetch(`${base}/api/me`, { headers: { Cookie: web } })).json();
  assert.equal(me.used, 200_000);
  assert.equal(me.sessions.length, 2);

  assert.equal((await fetch(`${base}/api/clips/${id}`, { method: "DELETE", headers: auth })).status, 200);
  assert.equal((await (await fetch(`${base}/api/clips`, { headers: auth })).json()).clips.length, 0);
  assert.equal(fs.readdirSync(path.join(dataDir, "clips")).flatMap((d) => fs.readdirSync(path.join(dataDir, "clips", d))).length, 0);
});

test("the quota stops an upload before it is stored", async () => {
  const web = await signIn("3000", "Full");
  const res = await fetch(`${base}/api/clips`, { method: "PUT", headers: { Cookie: web, "X-Klip-Web": "1", "Content-Length": String(2 * 1024 ** 3) }, body: Buffer.alloc(10) }).catch((e) => e);
  // The server answers before reading the body; fetch may then report the closed socket.
  if (res instanceof Error) return;
  assert.ok([413, 507].includes(res.status));
});

test("Google Drive cannot be chosen without a link", async () => {
  const web = await signIn("4000", "NoDrive");
  const res = await fetch(`${base}/api/me/storage`, { method: "PUT", headers: { Cookie: web, "X-Klip-Web": "1" }, body: JSON.stringify({ storage: "gdrive" }) });
  assert.equal(res.status, 409);
});

test("leaderboard: reports add up, hidden accounts and empty ones stay off", async () => {
  const web = { "X-Klip-Web": "1", "Content-Type": "application/json" };
  const a = await signIn("2001", "Ace");
  const b = await signIn("2002", "Bee");
  const c = await signIn("2003", "Hidden");
  await signIn("2004", "Nothing");
  const report = (cookie, clips) => fetch(`${base}/api/stats`, { method: "POST", headers: { ...web, Cookie: cookie }, body: JSON.stringify({ clips }) });
  assert.equal((await report(a, 3)).status, 200);
  assert.equal((await report(a, 1)).status, 429, "a second report within ten seconds is refused");
  assert.equal((await report(b, 5)).status, 200);
  assert.equal((await report(c, 9)).status, 200);
  assert.equal((await fetch(`${base}/api/me/board`, { method: "PUT", headers: { ...web, Cookie: c }, body: JSON.stringify({ show: false }) })).status, 200);
  // Absurd numbers are capped.
  assert.equal((await fetch(`${base}/api/stats`, { method: "POST", headers: { ...web, Cookie: b } , body: JSON.stringify({ clips: 1e9 }) })).status, 429);
  const board = (await (await fetch(`${base}/api/leaderboard`)).json()).board;
  assert.deepEqual(board.map((r) => [r.rank, r.name, r.clips]), [[1, "Bee", 5], [2, "Ace", 3]]);
  assert.ok(!JSON.stringify(board).includes("2001"), "no Discord ids on the board");
  // Signed out nobody can report.
  assert.equal((await fetch(`${base}/api/stats`, { method: "POST", headers: web, body: "{}" })).status, 401);
});
