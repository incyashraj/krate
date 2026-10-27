// Signed-in sessions, and signing out everywhere.
//
//   GET    /account/sessions                 {sessions: [{id, label, kind, created_at, current}]}
//   DELETE /account/sessions/<id>            {ok: true}
//   POST   /account/sessions/revoke-others   {revoked: n}
//
// What a session is, and why this costs no KV write per request:
//
// - A Krate session (Google, an email link, company sign-in) is a `krs_`
//   token: `session:<token>` names the account, and `usess:<user>:<token>`
//   already indexed it per person (support's sign-out uses it). That index
//   entry now holds what the list shows -- the kind, a label read from the
//   browser at sign-in ("Chrome on Mac"), when -- in the SAME write, so a
//   Krate sign-in costs nothing more than before. Revoking deletes both.
//
// - A GitHub sign-in hands the person GitHub's own token (read:user), not a
//   Krate session, because organisations know people as github:<login> only
//   when GitHub vouched for it. The hub kept no record of those, so a GitHub
//   sign-in now writes ONE index entry, `usess:<user>:<token>`, as a Krate
//   one does. Revoking one asks GitHub to revoke the token (the OAuth app's
//   own revocation endpoint) and drops the hub's six-hour identity cache, so
//   every route that checks a GitHub token refuses it from then on.
//
// Nothing is written per request: no last-used time. The list is one KV
// list call plus one read per Krate session (a session signed out another
// way is left off). `id` is the first 16 hex of the token's SHA-256: it
// names the session and cannot be turned back into the token. KV's list is
// eventually consistent, so a session made in the last minute may be
// missing from it: the session making the request is always shown, from
// its own token.

export const SESSION_TTL_SECONDS = 90 * 24 * 3600;
/// A GitHub token does not expire; its index entry is dropped after a year.
export const GITHUB_INDEX_TTL_SECONDS = 365 * 24 * 3600;
const KINDS = new Set(["github", "google", "email", "company"]);

async function sha256HexOf(text) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// The id a session is shown and revoked by.
export async function sessionId(token) {
  return (await sha256HexOf(String(token))).slice(0, 16);
}

/// What a person reads for the device a sign-in came from: "Chrome on
/// Mac", "Safari on iPhone", "Krate on Windows". `from` is the sign-in's
/// start ("app" when it is handed to the desktop app).
export function sessionLabel(userAgent, from) {
  const ua = String(userAgent || "");
  const os = /iPhone/.test(ua)
    ? "iPhone"
    : /iPad/.test(ua)
      ? "iPad"
      : /Android/.test(ua)
        ? "Android"
        : /CrOS/.test(ua)
          ? "ChromeOS"
          : /Mac OS X|Macintosh|macOS|darwin/i.test(ua)
            ? "Mac"
            : /Windows/i.test(ua)
              ? "Windows"
              : /Linux/i.test(ua)
                ? "Linux"
                : "";
  const krate = from === "app" || /^krate/i.test(ua) || /\bkrate\//i.test(ua);
  const browser = krate
    ? "Krate"
    : /Edg\//.test(ua)
      ? "Edge"
      : /OPR\/|Opera/.test(ua)
        ? "Opera"
        : /Firefox\/|FxiOS/.test(ua)
          ? "Firefox"
          : /Chrome\/|CriOS/.test(ua)
            ? "Chrome"
            : /Safari\//.test(ua)
              ? "Safari"
              : "";
  if (browser && os) return `${browser} on ${os}`;
  if (browser) return browser === "Krate" ? "Krate" : `${browser}`;
  if (os) return `A browser on ${os}`;
  return "A browser";
}

/// The index entry a sign-in writes: `usess:<user>:<token>`, holding what
/// the session list shows. For a Krate session this is the write that
/// already happened; for a GitHub sign-in it is the one write it adds.
export async function recordSession(env, userId, token, { kind, label, ttl } = {}) {
  const meta = {
    kind: KINDS.has(kind) ? kind : "email",
    label: String(label || "A browser").slice(0, 80),
    created_at: Math.floor(Date.now() / 1000),
  };
  await env.APPS.put(`usess:${userId}:${token}`, JSON.stringify(meta), {
    expirationTtl: ttl || SESSION_TTL_SECONDS,
    metadata: meta,
  });
  return meta;
}

function parsedMeta(raw) {
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? v : null;
  } catch {
    return null;
  }
}

/// Ask GitHub to revoke one of the tokens this hub's OAuth app issued.
/// True once GitHub no longer honours it (revoked now, or already gone).
async function revokeAtGitHub(env, token, clientId) {
  if (!env.GITHUB_CLIENT_SECRET || !clientId) return false;
  try {
    const res = await fetch(`https://api.github.com/applications/${clientId}/token`, {
      method: "DELETE",
      headers: {
        authorization: `Basic ${btoa(`${clientId}:${env.GITHUB_CLIENT_SECRET}`)}`,
        accept: "application/vnd.github+json",
        "content-type": "application/json",
        "user-agent": "krate-hub",
      },
      body: JSON.stringify({ access_token: token }),
    });
    // 204: revoked. 404: GitHub does not know it any more (revoked already).
    return res.status === 204 || res.status === 404;
  } catch {
    return false;
  }
}

/// End one session. True when it no longer signs anyone in.
async function endSession(env, key, token, clientId) {
  if (token.startsWith("krs_")) {
    await env.APPS.delete(`session:${token}`);
    if (key) await env.APPS.delete(key);
    return true;
  }
  if (!(await revokeAtGitHub(env, token, clientId))) return false;
  try {
    await env.APPS.delete(`token:${await sha256HexOf(token)}`);
  } catch {}
  if (key) await env.APPS.delete(key);
  return true;
}

/// Support's sign-out: every session the account's index names. Answers
/// how many ended.
export async function endEverySession(env, userId, clientId) {
  if (!userId) return 0;
  const prefix = `usess:${userId}:`;
  const listed = await env.APPS.list({ prefix, limit: 1000 });
  let ended = 0;
  for (const k of listed.keys || []) {
    const token = k.name.slice(prefix.length);
    if (token && (await endSession(env, k.name, token, clientId))) ended += 1;
  }
  return ended;
}

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "private, no-store" },
  });

/// /account/sessions... A Response, or null when the path is not one of
/// these. `deps`: verifyGitHub, testIdentity (KRATE_TEST_IDENTITIES), the
/// GitHub OAuth app's client id.
export async function accountSessionsRoute(request, url, env, deps) {
  const path = url.pathname.replace(/\/+$/, "");
  if (!path.startsWith("/account/sessions")) return null;
  const rest = path.slice("/account/sessions".length).split("/").filter(Boolean);
  const method = request.method;
  const known =
    (rest.length === 0 && method === "GET") ||
    (rest.length === 1 && rest[0] === "revoke-others" && method === "POST") ||
    (rest.length === 1 && rest[0] !== "revoke-others" && method === "DELETE");
  if (!known) return null;

  const bearer = (request.headers.get("authorization") || "").replace(/^Bearer\s+/i, "").trim();
  const identity = bearer ? await deps.verifyGitHub(request, env) : null;
  if (!identity) return json({ error: "not-signed-in", message: "sign in first, then try again" }, 401);
  const currentId = await sessionId(bearer);
  const kindNow = bearer.startsWith("krs_") ? null : "github";

  // A stubbed identity for a local proof (KRATE_TEST_IDENTITIES) is one
  // session and nothing else: there is nothing to list or revoke.
  if (deps.testIdentity(bearer, env)) {
    const synthetic = { id: currentId, label: sessionLabel(request.headers.get("user-agent")), kind: identity.github ? "github" : "email", created_at: null, current: true };
    if (method === "GET") return json({ sessions: [synthetic] });
    if (method === "POST") return json({ revoked: 0 });
    return json({ error: "not-revocable", message: "a test sign-in cannot be signed out from here" }, 409);
  }

  // Whose sessions: the account a Krate session belongs to, or the account
  // a GitHub sign-in reached (the ghlogin: record ensureUser keeps).
  let userId = identity.id || null;
  if (!userId && identity.github && identity.login) {
    try {
      userId = await env.APPS.get(`ghlogin:${String(identity.login).toLowerCase()}`);
    } catch {}
  }
  let user = null;
  if (userId) {
    try {
      user = JSON.parse((await env.APPS.get(`user:${userId}`)) || "null");
    } catch {}
  }
  const olderKind = user && Array.isArray(user.providers) && user.providers.includes("google") ? "google" : "email";

  const entries = [];
  if (userId) {
    const prefix = `usess:${userId}:`;
    const listed = await env.APPS.list({ prefix, limit: 1000 });
    for (const k of listed.keys || []) {
      const token = k.name.slice(prefix.length);
      if (!token) continue;
      // A Krate session signed out another way is gone: leave it off.
      if (token.startsWith("krs_") && !(await env.APPS.get(`session:${token}`))) continue;
      let meta = k.metadata && typeof k.metadata === "object" ? k.metadata : null;
      if (!meta) meta = parsedMeta(await env.APPS.get(k.name));
      entries.push({
        key: k.name,
        token,
        view: {
          id: await sessionId(token),
          label: (meta && meta.label) || "A browser",
          // Before sessions carried their kind, the index held "1".
          kind: meta && KINDS.has(meta.kind) ? meta.kind : token.startsWith("krs_") ? olderKind : "github",
          created_at: (meta && Number(meta.created_at)) || null,
          current: token === bearer,
        },
      });
    }
  }

  if (method === "GET") {
    const sessions = entries.map((e) => e.view);
    if (!sessions.some((s) => s.current)) {
      sessions.push({
        id: currentId,
        label: sessionLabel(request.headers.get("user-agent")),
        kind: kindNow || (identity.sso && identity.sso.length ? "company" : olderKind),
        created_at: null,
        current: true,
      });
    }
    sessions.sort((a, b) => Number(b.current) - Number(a.current) || (b.created_at || 0) - (a.created_at || 0));
    return json({ sessions });
  }

  if (method === "POST") {
    let revoked = 0;
    for (const e of entries) {
      if (e.token === bearer) continue;
      if (await endSession(env, e.key, e.token, deps.githubClientId)) revoked += 1;
    }
    return json({ revoked });
  }

  // DELETE /account/sessions/<id>
  const id = String(rest[0] || "").toLowerCase();
  let entry = entries.find((e) => e.view.id === id);
  // This session, even before the index lists it.
  if (!entry && id === currentId) entry = { key: userId ? `usess:${userId}:${bearer}` : null, token: bearer };
  if (!entry) return json({ error: "no-such-session", message: "that session is not one of yours, or it has already ended" }, 404);
  if (!(await endSession(env, entry.key, entry.token, deps.githubClientId))) {
    return json(
      { error: "not-revocable", message: "GitHub would not sign that session out just now -- try again in a minute, or revoke Krate under GitHub Settings, Applications" },
      502,
    );
  }
  return json({ ok: true });
}
