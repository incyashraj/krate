// One account, every way in (IC-996, K-1328).
//
// A person signs in to Krate with GitHub on the command line, Google in a
// browser, Apple on a phone, a link sent to an address. Each way is an
// `ident:<kind>:<id>` pointer to the one account, and the account keeps the
// list (`ways`) so Settings can show them, add one on purpose and remove
// one. Before this an account gained a second way only by accident -- the
// same verified address -- and nothing could show, add or remove one.
//
// Adding a way is the dangerous part. A link that finishes in someone
// else's browser would attach THEIR GitHub to the account that started it,
// and every later GitHub sign-in of theirs would land in a stranger's
// account. So a link is only ever confirmed by the account that started it,
// from the browser that started it: the start records the account and a
// nonce the page keeps; the provider's answer is parked under a one-time
// code; the page that holds the nonce confirms the code with its own
// session, and the hub checks that session is the starting account.
//
//   GET    /me/sign-ins                  the account's ways in, and which can be added
//   POST   /me/sign-ins/link             {kind, nonce} -> {go}: the provider's page
//   POST   /me/sign-ins/confirm          {code} -> the way, added (or why not)
//   POST   /me/sign-ins/email            {email} -> a link sent to that address
//   DELETE /me/sign-ins/<kind>/<id>      remove one, never the last

export const WAY_KINDS = ["github", "google", "apple", "email"];

const EMAIL_RE = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;
const LINK_TTL = 600;

/// The way a sign-in proved, as the account lists it.
export function wayFor(provider, stableId, profile = {}) {
  const at = Math.floor(Date.now() / 1000);
  if (provider === "github") return { kind: "github", id: String(stableId), label: String(profile.login || ""), at };
  if (provider === "google") return { kind: "google", id: String(stableId), label: String(profile.email || profile.unverified_email || profile.name || ""), at };
  if (provider === "apple") return { kind: "apple", id: String(stableId), label: String(profile.email || ""), at };
  if (provider === "email") return { kind: "email", id: String(stableId).toLowerCase(), label: String(stableId).toLowerCase(), at };
  return null;
}

/// Add a way to an account record in place. True when it changed.
export function addWay(record, way) {
  if (!way) return false;
  const ways = Array.isArray(record.ways) ? record.ways : [];
  const same = ways.find((w) => w.kind === way.kind && w.id === way.id);
  if (same) {
    // A GitHub login can be renamed; keep the label GitHub gives today.
    if (way.label && same.label !== way.label) {
      same.label = way.label;
      record.ways = ways;
      return true;
    }
    return false;
  }
  record.ways = [...ways, way];
  return true;
}

/// Every way in an account has, newest knowledge first: what the account
/// recorded, plus what an account from before `ways` existed can be shown
/// to hold (its providers, its GitHub login, its proved addresses).
export function waysOf(record) {
  if (!record) return [];
  const out = (Array.isArray(record.ways) ? record.ways : []).map((w) => ({ ...w }));
  const has = (kind, id) => out.some((w) => w.kind === kind && (id === undefined || w.id === id));
  const providers = Array.isArray(record.providers) ? record.providers : [];
  if (providers.includes("github") && !has("github")) {
    out.push({ kind: "github", id: "", label: String(record.github_login || ""), at: null });
  }
  if (providers.includes("google") && !has("google")) {
    out.push({ kind: "google", id: "", label: String(record.email || ""), at: null });
  }
  if (providers.includes("apple") && !has("apple")) {
    out.push({ kind: "apple", id: "", label: "", at: null });
  }
  // Every proved address is a way in: an email link to it lands here.
  const addresses = new Set(Array.isArray(record.verified_emails) ? record.verified_emails : []);
  if (providers.includes("email") && record.email) addresses.add(String(record.email).toLowerCase());
  for (const address of addresses) {
    if (!has("email", address)) out.push({ kind: "email", id: address, label: address, at: null });
  }
  const order = { github: 0, google: 1, apple: 2, email: 3 };
  return out.sort((a, b) => order[a.kind] - order[b.kind] || String(a.label).localeCompare(String(b.label)));
}

/// Which providers this hub can add, from its configuration.
export function addable(env) {
  return {
    github: Boolean(env.GITHUB_CLIENT_SECRET),
    google: Boolean(env.GOOGLE_CLIENT_ID && env.GOOGLE_CLIENT_SECRET),
    apple: Boolean(env.APPLE_CLIENT_ID),
    email: Boolean(env.RESEND_API_KEY),
  };
}

const WORDS = {
  github: "GitHub",
  google: "Google",
  apple: "Apple",
  email: "email",
};

/// The /me/sign-ins routes. `deps` brings the hub's own: who is signed in
/// (`accountOf(request)` -> the account record with `id`, or null), the
/// provider start URLs, linking, mail, and sessions.
export async function signInsRoute(request, url, env, deps) {
  const { json } = deps;
  const account = await deps.accountOf(request);
  if (!account || !account.id) return json({ error: "sign-in", message: "Sign in first." }, 401);
  const rest = url.pathname.replace(/^\/me\/sign-ins\/?/, "").split("/").filter(Boolean);
  const method = request.method;

  if (method === "GET" && rest.length === 0) {
    const ways = waysOf(account);
    return json({ ways: ways.map((w) => ({ ...w, removable: ways.length > 1 })), can_add: addable(env) });
  }

  if (method === "POST" && rest[0] === "link") {
    const body = await request.json().catch(() => ({}));
    const kind = String(body.kind || "");
    if (!["github", "google", "apple"].includes(kind)) return json({ error: "kind", message: "Say which: github, google or apple." }, 400);
    if (!addable(env)[kind]) return json({ error: "not-configured", message: `${WORDS[kind]} sign-in is not switched on here yet.` }, 503);
    const nonce = /^[A-Za-z0-9_-]{16,64}$/.test(body.nonce || "") ? body.nonce : "";
    if (!nonce) return json({ error: "nonce", message: "This page is out of date. Reload it and try again." }, 400);
    const state = crypto.randomUUID();
    await env.APPS.put(`login:${state}`, JSON.stringify({ from: "link", link: account.id, nonce, kind }), { expirationTtl: LINK_TTL });
    return json({ go: deps.providerUrl(kind, state) });
  }

  if (method === "POST" && rest[0] === "confirm") {
    const body = await request.json().catch(() => ({}));
    const code = String(body.code || "");
    if (!/^[0-9a-f-]{36}$/.test(code)) return json({ error: "code", message: "That link is not one Krate made." }, 400);
    const parked = JSON.parse((await env.APPS.get(`linkdone:${code}`)) || "null");
    if (!parked) return json({ error: "expired", message: "That link has expired or was already used. Start again from Ways to sign in." }, 410);
    // Only the account that started the link may finish it: a link that
    // reached anyone else's browser adds nothing to anyone.
    if (parked.link !== account.id) {
      return json({ error: "not-yours", message: "This link was started from a different Krate account, so nothing was added." }, 403);
    }
    await env.APPS.delete(`linkdone:${code}`);
    const done = await deps.linkWay(parked.provider, parked.stableId, parked.profile, account.id);
    if (done.taken) {
      return json(
        { error: "taken", message: `That ${WORDS[parked.provider]} account already signs in to a different Krate account. Nothing changed. Sign in with it and remove it there first, or keep using the account it opens.` },
        409,
      );
    }
    return json({ ok: true, kind: parked.provider, ways: waysOf(done.record) });
  }

  if (method === "POST" && rest[0] === "email") {
    const body = await request.json().catch(() => ({}));
    const email = String(body.email || "").trim().toLowerCase();
    if (!EMAIL_RE.test(email) || email.length > 254) return json({ error: "email", message: "That does not look like an email address." }, 400);
    if (!addable(env).email) return json({ error: "not-configured", message: "Email sign-in is not switched on here yet." }, 503);
    const held = await env.APPS.get(`ident:email:${email}`);
    if (held === account.id) return json({ error: "already", message: "That address already signs in to this account." }, 409);
    if (held) return json({ error: "taken", message: "That address already signs in to a different Krate account. Nothing changed." }, 409);
    const sent = await deps.sendLinkMail(email, account.id);
    if (sent.problem) return json({ error: "mail", message: sent.problem }, 502);
    return json({ sent: true, email });
  }

  if (method === "DELETE" && rest.length === 2 && WAY_KINDS.includes(rest[0])) {
    const kind = rest[0];
    const id = decodeURIComponent(rest[1]);
    const ways = waysOf(account);
    const way = ways.find((w) => w.kind === kind && w.id === id) || (id === "-" ? ways.find((w) => w.kind === kind && !w.id) : null);
    if (!way) return json({ error: "no-such-way", message: "That is not one of this account's ways to sign in." }, 404);
    if (ways.length <= 1) return json({ error: "last", message: "This is the only way into your account, so it stays. Add another first." }, 409);
    const record = await deps.removeWay(account, way);
    return json({ ok: true, ways: waysOf(record) });
  }

  return json({ error: "not-found" }, 404);
}

/// Take a way off an account: its `ident:` pointer, and what made the
/// account that GitHub login or that address. Sessions made that way end,
/// unless another way of the same kind remains. A pointer from before
/// `ways` recorded ids is found by its value.
export async function removeWayFrom(env, record, way, { endSessionsOfKind }) {
  const userId = record.id;
  const pointsHere = async (key) => (await env.APPS.get(key)) === userId;
  if (way.kind === "email") {
    if (await pointsHere(`ident:email:${way.id}`)) await env.APPS.delete(`ident:email:${way.id}`);
    record.verified_emails = (record.verified_emails || []).filter((e) => e !== way.id);
    if (String(record.email || "").toLowerCase() === way.id) record.email = record.verified_emails[0] || "";
  } else if (way.id) {
    if (await pointsHere(`ident:${way.kind}:${way.id}`)) await env.APPS.delete(`ident:${way.kind}:${way.id}`);
  } else {
    // Recorded before ids were: find this account's pointers of that kind.
    const prefix = `ident:${way.kind}:`;
    let cursor;
    for (let page = 0; page < 50; page += 1) {
      const listed = await env.APPS.list({ prefix, limit: 1000, ...(cursor ? { cursor } : {}) });
      for (const k of listed.keys || []) {
        if (await pointsHere(k.name)) await env.APPS.delete(k.name);
      }
      if (listed.list_complete !== false || !listed.cursor) break;
      cursor = listed.cursor;
    }
  }
  if (way.kind === "github") {
    const login = String(way.label || record.github_login || "").toLowerCase();
    if (login && (await pointsHere(`ghlogin:${login}`))) await env.APPS.delete(`ghlogin:${login}`);
    if (String(record.github_login || "").toLowerCase() === login) delete record.github_login;
  }
  record.ways = (Array.isArray(record.ways) ? record.ways : []).filter((w) => !(w.kind === way.kind && w.id === way.id));
  // The provider goes from the account's list when no recorded way of that
  // kind is left -- first, or the list would show the removed way again
  // as one the account held before ways were recorded.
  if (way.kind !== "email" && !record.ways.some((w) => w.kind === way.kind)) {
    record.providers = (record.providers || []).filter((p) => p !== way.kind);
  }
  const stillThisKind = waysOf(record).some((w) => w.kind === way.kind);
  if (way.kind === "email" && !stillThisKind) {
    record.providers = (record.providers || []).filter((p) => p !== "email");
  }
  await env.APPS.put(`user:${userId}`, JSON.stringify(record));
  if (!stillThisKind) await endSessionsOfKind(env, userId, way.kind).catch(() => 0);
  return record;
}

// ------------------------------------------------------------------ apple
//
// Sign in with Apple, by its ID token alone: Apple posts it back to the
// callback (response_mode=form_post), and the token is checked against
// Apple's published keys -- issuer, audience (this hub's Services ID),
// expiry and the nonce this hub sent. No client secret is needed for that,
// so the hub's whole configuration is APPLE_CLIENT_ID.

const APPLE_ISSUER = "https://appleid.apple.com";

export function appleAuthorizeUrl(env, state, nonce) {
  const auth = new URL("https://appleid.apple.com/auth/authorize");
  auth.searchParams.set("client_id", env.APPLE_CLIENT_ID);
  auth.searchParams.set("redirect_uri", `${env.PUBLIC_BASE}/login/apple/callback`);
  auth.searchParams.set("response_type", "code id_token");
  auth.searchParams.set("response_mode", "form_post");
  auth.searchParams.set("scope", "name email");
  auth.searchParams.set("state", state);
  auth.searchParams.set("nonce", nonce);
  return auth.toString();
}

function b64urlToBytes(s) {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const bin = atob((s + pad).replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

/// Check an Apple ID token. Answers its claims, or null.
export async function verifyAppleIdToken(env, idToken, expectedNonce, fetchKeys = defaultAppleKeys) {
  const parts = String(idToken || "").split(".");
  if (parts.length !== 3) return null;
  let header;
  let claims;
  try {
    header = JSON.parse(new TextDecoder().decode(b64urlToBytes(parts[0])));
    claims = JSON.parse(new TextDecoder().decode(b64urlToBytes(parts[1])));
  } catch {
    return null;
  }
  if (header.alg !== "RS256" || !header.kid) return null;
  const keys = await fetchKeys();
  const jwk = (keys || []).find((k) => k.kid === header.kid && k.kty === "RSA");
  if (!jwk) return null;
  const key = await crypto.subtle.importKey("jwk", { kty: "RSA", n: jwk.n, e: jwk.e, alg: "RS256", ext: true }, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
  const ok = await crypto.subtle.verify(
    "RSASSA-PKCS1-v1_5",
    key,
    b64urlToBytes(parts[2]),
    new TextEncoder().encode(`${parts[0]}.${parts[1]}`),
  );
  if (!ok) return null;
  const now = Math.floor(Date.now() / 1000);
  if (claims.iss !== APPLE_ISSUER) return null;
  if (claims.aud !== env.APPLE_CLIENT_ID) return null;
  if (!Number.isFinite(claims.exp) || claims.exp < now) return null;
  if (!expectedNonce || claims.nonce !== expectedNonce) return null;
  if (!claims.sub) return null;
  return claims;
}

async function defaultAppleKeys() {
  const r = await fetch("https://appleid.apple.com/auth/keys");
  if (!r.ok) return [];
  const body = await r.json();
  return Array.isArray(body.keys) ? body.keys : [];
}
