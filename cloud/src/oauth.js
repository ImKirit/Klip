// Sign in with Discord, and the link to a Google Drive. Both are plain OAuth 2 code
// flows against the official endpoints. Only the scopes named here are asked for.

const DISCORD_AUTHORIZE = "https://discord.com/oauth2/authorize";
const DISCORD_TOKEN = "https://discord.com/api/oauth2/token";
const DISCORD_ME = "https://discord.com/api/users/@me";

const GOOGLE_AUTHORIZE = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN = "https://oauth2.googleapis.com/token";
/** Only files Klip itself created, nothing else in the Drive. */
const GOOGLE_SCOPE = "https://www.googleapis.com/auth/drive.file";

export function discordLoginUrl(cfg, state, fakeUser = "") {
  // Tests only: `fakeUser` is "<id>:<name>" of the account the fake Discord signs in.
  if (cfg.discord.fake) return `${cfg.baseUrl}/auth/discord/callback?code=${encodeURIComponent(`fake:${fakeUser}`)}&state=${encodeURIComponent(state)}`;
  const q = new URLSearchParams({
    client_id: cfg.discord.clientId,
    response_type: "code",
    redirect_uri: `${cfg.baseUrl}/auth/discord/callback`,
    scope: "identify",
    state,
  });
  return `${DISCORD_AUTHORIZE}?${q}`;
}

/** The Discord account behind a code: id, name and avatar. */
export async function discordUser(cfg, code) {
  if (cfg.discord.fake) {
    // Tests: "fake" or "fake:<id>:<name>".
    const [, id = "", name = ""] = code.split(":");
    return { id: id || "1000", name: name || "Tester", avatar: null };
  }
  const tokenRes = await fetch(DISCORD_TOKEN, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: cfg.discord.clientId,
      client_secret: cfg.discord.clientSecret,
      grant_type: "authorization_code",
      code,
      redirect_uri: `${cfg.baseUrl}/auth/discord/callback`,
    }),
  });
  if (!tokenRes.ok) throw new Error(`Discord refused the sign in (${tokenRes.status})`);
  const { access_token } = await tokenRes.json();
  const meRes = await fetch(DISCORD_ME, { headers: { Authorization: `Bearer ${access_token}` } });
  if (!meRes.ok) throw new Error(`Discord did not name the account (${meRes.status})`);
  const me = await meRes.json();
  // Without an own picture Discord shows one of its six default ones, so does Klip.
  const avatar = me.avatar
    ? `https://cdn.discordapp.com/avatars/${me.id}/${me.avatar}.png?size=128`
    : `https://cdn.discordapp.com/embed/avatars/${Number((BigInt(me.id) >> 22n) % 6n)}.png`;
  return { id: String(me.id), name: me.global_name || me.username, avatar };
}

export function googleEnabled(cfg) {
  return Boolean(cfg.google.clientId && cfg.google.clientSecret);
}

export function googleLinkUrl(cfg, state) {
  const q = new URLSearchParams({
    client_id: cfg.google.clientId,
    response_type: "code",
    redirect_uri: `${cfg.baseUrl}/auth/google/callback`,
    scope: GOOGLE_SCOPE,
    access_type: "offline",
    prompt: "consent",
    state,
  });
  return `${GOOGLE_AUTHORIZE}?${q}`;
}

/** Exchanges the code for a refresh token, which lets the server upload later. */
export async function googleRefreshToken(cfg, code) {
  const res = await fetch(GOOGLE_TOKEN, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: cfg.google.clientId,
      client_secret: cfg.google.clientSecret,
      grant_type: "authorization_code",
      code,
      redirect_uri: `${cfg.baseUrl}/auth/google/callback`,
    }),
  });
  if (!res.ok) throw new Error(`Google refused the link (${res.status})`);
  const body = await res.json();
  if (!body.refresh_token) throw new Error("Google gave no lasting access, try linking again");
  return body.refresh_token;
}

/** A short lived access token from the stored refresh token. */
export async function googleAccessToken(cfg, refreshToken) {
  const res = await fetch(GOOGLE_TOKEN, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: cfg.google.clientId,
      client_secret: cfg.google.clientSecret,
      grant_type: "refresh_token",
      refresh_token: refreshToken,
    }),
  });
  if (!res.ok) throw new Error(`Google access ended (${res.status}), link the Drive again`);
  return (await res.json()).access_token;
}
