// Klip Cloud in the browser: the clips of the signed in Discord account, a player, the
// confirmation for a new PC, and the settings. Everything is built with DOM calls, names
// of clips never go through innerHTML.

const BASE = window.KLIP_BASE || "";
const main = document.getElementById("main");
const nav = document.getElementById("nav");

function el(tag, attrs = {}, ...kids) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid !== null && kid !== undefined && kid !== false) node.append(kid);
  return node;
}

async function api(path, options = {}) {
  const res = await fetch(BASE + path, { credentials: "same-origin", ...options, headers: { "X-Klip-Web": "1", ...(options.body ? { "Content-Type": "application/json" } : {}), ...(options.headers || {}) } });
  const body = res.status === 204 ? {} : await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error || `Error ${res.status}`);
  return body;
}

function toast(text, error = false) {
  const t = el("div", { class: `toast${error ? " err" : ""}`, role: "status" }, text);
  document.body.append(t);
  setTimeout(() => t.remove(), 3500);
}

function duration(ms) {
  const s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${r}` : `${m}:${r}`;
}

function bytes(n) {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(0)} MB`;
  return `${Math.max(1, Math.round(n / 1024))} KB`;
}

function dayOf(iso) {
  const d = new Date(iso);
  const today = new Date();
  const y = new Date(today);
  y.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === y.toDateString()) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
}

const KIND = { clip: "Clip", auto_clip: "Automatic clip", recording: "Recording", screenshot: "Screenshot" };

function go(path) {
  history.pushState(null, "", BASE + path);
  render();
}

document.addEventListener("click", (e) => {
  const a = e.target.closest("a[data-go]");
  if (!a || e.ctrlKey || e.metaKey) return;
  e.preventDefault();
  go(a.getAttribute("data-go"));
});
window.addEventListener("popstate", () => render());

function signIn(me, reason) {
  const next = location.pathname.slice(BASE.length) + location.search;
  main.replaceChildren(
    el(
      "section",
      { class: "hero" },
      el("img", { src: `${BASE}/assets/logo.svg`, width: 72, height: 72, alt: "", style: "border-radius:18px" }),
      el("h1", {}, "Your clips, everywhere"),
      el("p", {}, reason || "Sign in with the Discord account you use in Klip on your PC, and every clip you upload shows up here."),
      me.discord
        ? el("a", { class: "btn pri", href: `${BASE}/auth/discord?next=${encodeURIComponent(next || "/")}` }, "Sign in with Discord")
        : el("p", { class: "muted" }, "Discord sign in is not set up on this server yet."),
    ),
  );
}

async function listPage() {
  const { clips } = await api("/api/clips");
  if (clips.length === 0) {
    main.replaceChildren(el("section", { class: "hero" }, el("h1", {}, "No clips yet"), el("p", {}, "In Klip on your PC, open Settings, Cloud, and choose which clips go up.")));
    return;
  }
  const out = [];
  let day = null;
  let grid = null;
  for (const c of clips) {
    const d = dayOf(c.created);
    if (d !== day) {
      day = d;
      grid = el("div", { class: "grid" });
      out.push(el("h2", { class: "day" }, d), grid);
    }
    const label = c.kind === "auto_clip" ? (c.reason ? `Auto / ${c.reason.replace(/_/g, " ")}` : "Auto") : null;
    grid.append(
      el(
        "a",
        { class: "card", href: `${BASE}/clip/${c.id}`, "data-go": `/clip/${c.id}` },
        el("div", { class: "thumb" }, c.has_thumb ? el("img", { src: `${BASE}/api/clips/${c.id}/thumb`, alt: "", loading: "lazy" }) : null, label ? el("span", { class: "tag" }, label) : null, c.duration_ms ? el("span", { class: "dur" }, duration(c.duration_ms)) : null),
        el("h3", {}, c.name),
        el("p", {}, el("b", {}, c.game || "Desktop"), ` / ${KIND[c.kind] || "Clip"}`),
      ),
    );
  }
  main.replaceChildren(...out);
}

async function clipPage(id) {
  const { clips } = await api("/api/clips");
  const c = clips.find((x) => x.id === id);
  if (!c) {
    main.replaceChildren(el("p", { class: "muted center" }, "This clip is gone."));
    return;
  }
  const src = `${BASE}/api/clips/${c.id}/video`;
  main.replaceChildren(
    el(
      "section",
      { class: "player" },
      el("a", { class: "back", href: `${BASE}/`, "data-go": "/" }, "All clips"),
      el("video", { src, controls: true, playsinline: true, preload: "metadata", poster: c.has_thumb ? `${BASE}/api/clips/${c.id}/thumb` : null }),
      el("h1", {}, c.name),
      el("p", { class: "muted" }, `${c.game || "Desktop"} / ${KIND[c.kind] || "Clip"} / ${new Date(c.created).toLocaleString()} / ${bytes(c.size)}`),
      el(
        "div",
        { class: "actions" },
        el("a", { class: "btn pri", href: src, download: `${c.name}.mp4` }, "Download"),
        navigator.share ? el("button", { class: "btn", onclick: () => navigator.share({ title: c.name, url: location.href }).catch(() => {}) }, "Share") : null,
        el(
          "button",
          {
            class: "btn danger",
            onclick: async () => {
              if (!confirm(`Delete "${c.name}" from the cloud? The copy on your PC stays.`)) return;
              try {
                await api(`/api/clips/${c.id}`, { method: "DELETE" });
                go("/");
              } catch (e) {
                toast(e.message, true);
              }
            },
          },
          "Delete",
        ),
      ),
    ),
  );
}

async function linkPage() {
  const code = new URLSearchParams(location.search).get("code") || "";
  const button = el("button", { class: "btn pri" }, "Connect this PC");
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      const r = await api("/api/device/confirm", { method: "POST", body: JSON.stringify({ user_code: code }) });
      main.replaceChildren(el("section", { class: "hero" }, el("h1", {}, "Connected"), el("p", {}, `${r.label} can upload clips now. You can close this page.`)));
    } catch (e) {
      button.disabled = false;
      toast(e.message, true);
    }
  });
  main.replaceChildren(el("section", { class: "hero" }, el("h1", {}, "Connect Klip"), el("p", {}, "Klip on a PC asks to upload clips to your account. Check that it shows the same code:"), el("div", { class: "code" }, code || "?"), button));
}

async function settingsPage(me) {
  const share = me.quota > 0 ? Math.min(100, (me.used / me.quota) * 100) : 0;
  const storage = (value) =>
    el(
      "button",
      {
        class: me.user.storage === value ? "on" : "",
        onclick: async () => {
          if (value === "gdrive" && !me.user.google_linked) {
            location.href = `${BASE}/auth/google`;
            return;
          }
          try {
            await api("/api/me/storage", { method: "PUT", body: JSON.stringify({ storage: value }) });
            render();
          } catch (e) {
            toast(e.message, true);
          }
        },
      },
      value === "gdrive" ? "Google Drive" : "This server",
    );
  main.replaceChildren(
    el("h2", { class: "section" }, "Account"),
    el(
      "div",
      { class: "panel" },
      el("div", { class: "row" }, el("div", {}, me.user.name, el("small", {}, "Signed in with Discord")), el("button", { class: "btn small", onclick: async () => { await api("/auth/logout", { method: "POST" }).catch(() => {}); go("/"); } }, "Sign out")),
      el("div", { class: "row" }, el("div", {}, "Clips", el("small", {}, `${me.clips} in the cloud`)), el("div", {}, el("small", {}, me.quota > 0 ? `${bytes(me.used)} of ${bytes(me.quota)} on this server` : `${bytes(me.used)} on this server`), me.quota > 0 ? el("div", { class: "bar" }, el("i", { style: `width:${share}%` })) : null)),
    ),
    el("h2", { class: "section" }, "Where new clips are kept"),
    el(
      "div",
      { class: "panel" },
      el("div", { class: "row" }, el("div", {}, "Storage", el("small", {}, me.google ? "Google Drive keeps them in a folder named Klip, this server only passes them on." : "Google Drive is not set up on this server.")), el("div", { class: "choice" }, storage("server"), me.google ? storage("gdrive") : null)),
    ),
    el("h2", { class: "section" }, "Signed in devices"),
    el(
      "div",
      { class: "panel" },
      ...me.sessions.map((s) =>
        el(
          "div",
          { class: "row" },
          el("div", {}, s.kind === "device" ? s.label || "Klip on a PC" : "Browser", el("small", {}, `${s.current ? "This one, " : ""}last used ${new Date(s.last_used).toLocaleString()}`)),
          s.current ? null : el("button", { class: "btn small danger", onclick: async () => { await api(`/api/sessions/${s.id}`, { method: "DELETE" }); render(); } }, "Remove"),
        ),
      ),
    ),
  );
}

async function render() {
  const path = location.pathname.slice(BASE.length) || "/";
  try {
    const me = await api("/api/me");
    nav.replaceChildren();
    if (!me.user) return signIn(me, path === "/link" ? "Sign in with Discord to connect Klip on your PC." : null);
    nav.append(el("a", { href: `${BASE}/settings`, "data-go": "/settings", title: "Settings" }, me.user.avatar ? el("img", { src: me.user.avatar, alt: me.user.name }) : el("span", { class: "btn small" }, me.user.name)));
    if (path === "/link") return linkPage();
    if (path === "/settings") return settingsPage(me);
    const clip = /^\/clip\/([\w-]+)$/.exec(path);
    if (clip) return clipPage(clip[1]);
    return listPage();
  } catch (e) {
    main.replaceChildren(el("p", { class: "muted center" }, e.message));
  }
}

render();
