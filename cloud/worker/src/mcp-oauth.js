// Sign-in for Krate in Claude (IC-989 phase 2): OAuth 2.1 as the MCP
// authorization spec asks it, with the hub as both the authorization server
// and the protected resource (/mcp).
//
//   GET  /.well-known/oauth-protected-resource[/mcp]   RFC 9728
//   GET  /.well-known/oauth-authorization-server       RFC 8414
//   POST /oauth/register                                RFC 7591, open
//   (a client_id that is an https address)              Client ID Metadata Documents
//   GET  /oauth/authorize                               the consent page
//   GET  /oauth/authorize/done                          where sign-in lands
//   POST /oauth/approve                                 the person's yes/no
//   POST /oauth/token                                   code + PKCE, refresh
//   POST /oauth/revoke                                  RFC 7009
//
// The person signs in with the hub's own sign-in (GitHub, Google, email),
// which hands its session to /oauth/authorize/done the way it hands one to
// krate.tech/login/done, nonce and all (K-909). The page shows what the
// connector may do; the yes makes an authorization code.
//
// Almost nothing is stored. A client registration, an authorization request,
// a code and every token are sealed (AES-GCM, the token kind bound in as
// associated data) with a key only the hub holds, so registering costs no
// KV write and anyone may register. KV holds only what must change: a
// grant's refresh generation (one write when a code is exchanged, one per
// refresh), a revocation mark, and the account's list of grants. That also
// keeps a code exchanged from Claude's servers from depending on a record
// written a moment earlier from the person's browser, which KV may not yet
// show in another location.
//
// Access tokens last an hour and say whose they are; refresh tokens last 30
// days and rotate, and a refresh token used twice ends the grant (OAuth 2.1).

const ACCESS_TTL = 60 * 60;
const REFRESH_TTL = 30 * 24 * 60 * 60;
const CODE_TTL = 5 * 60;
const REQUEST_TTL = 30 * 60;
export const MCP_SCOPE = "krate.apps";
/** What the person is asked to allow, in the words the page shows. */
export const CONSENT_WORDS = "build apps you ask for and publish them to your account";

const enc = new TextEncoder();
const dec = new TextDecoder();
const b64url = (bytes) => {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};
const unb64url = (text) => {
  const s = atob(String(text).replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(s, (c) => c.charCodeAt(0));
};
const randomHex = (n) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");
const now = () => Math.floor(Date.now() / 1000);
async function sha256(text) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(text)));
}

/** The sealing key: MCP_OAUTH_SECRET, or one derived from KEY_WRAP_SECRET under its own label. */
const keyCache = new WeakMap();
async function sealKey(env) {
  if (keyCache.has(env)) return keyCache.get(env);
  const secret = env.MCP_OAUTH_SECRET || (env.KEY_WRAP_SECRET ? `derived:${env.KEY_WRAP_SECRET}` : "");
  if (!secret) return null;
  const base = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const raw = await crypto.subtle.sign("HMAC", base, enc.encode("krate mcp oauth v1"));
  const key = await crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
  keyCache.set(env, key);
  return key;
}

/** A sealed token of one kind: `kmcp<kind>_<iv+ciphertext>`. */
async function seal(env, kind, payload) {
  const key = await sealKey(env);
  if (!key) throw new Error("no sealing key");
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const body = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: enc.encode(`kmcp:${kind}`) }, key, enc.encode(JSON.stringify(payload))),
  );
  const out = new Uint8Array(12 + body.length);
  out.set(iv);
  out.set(body, 12);
  return `kmcp${kind}_${b64url(out)}`;
}

/** Open a sealed token of `kind`, or null: wrong kind, tampered, unreadable or expired. */
async function open(env, kind, token) {
  const prefix = `kmcp${kind}_`;
  if (typeof token !== "string" || !token.startsWith(prefix) || token.length > 8192) return null;
  const key = await sealKey(env);
  if (!key) return null;
  try {
    const raw = unb64url(token.slice(prefix.length));
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: raw.subarray(0, 12), additionalData: enc.encode(`kmcp:${kind}`) },
      key,
      raw.subarray(12),
    );
    const payload = JSON.parse(dec.decode(plain));
    if (!payload || typeof payload !== "object") return null;
    if (typeof payload.e === "number" && payload.e < now()) return null;
    return payload;
  } catch (_) {
    return null;
  }
}

// ----------------------------------------------------------------- answers

const CORS = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type, authorization, mcp-protocol-version",
};
const jsonOut = (body, status = 200, extra = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", ...CORS, ...extra },
  });
const oauthError = (error, description, status = 400) => jsonOut({ error, error_description: description }, status);
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

export const resourceOf = (base) => `${base}/mcp`;
export const resourceMetadataUrl = (base) => `${base}/.well-known/oauth-protected-resource/mcp`;

/** The 401 that starts a sign-in: the header points at the resource metadata. */
export function signInNeeded(base, { invalid = false, words = "Sign in to Krate to build and publish apps." } = {}) {
  const parts = [`Bearer resource_metadata="${resourceMetadataUrl(base)}"`, `scope="${MCP_SCOPE}"`];
  if (invalid) parts.push('error="invalid_token"', 'error_description="The sign-in has ended; sign in again"');
  return new Response(JSON.stringify({ error: invalid ? "invalid_token" : "unauthorized", error_description: words }), {
    status: 401,
    headers: {
      "content-type": "application/json",
      "www-authenticate": parts.join(", "),
      ...CORS,
      "access-control-expose-headers": "www-authenticate, mcp-session-id",
    },
  });
}

function protectedResource(base) {
  return jsonOut(
    {
      resource: resourceOf(base),
      authorization_servers: [base],
      scopes_supported: [MCP_SCOPE],
      bearer_methods_supported: ["header"],
      resource_name: "Krate",
      resource_documentation: "https://krate.tech",
    },
    200,
    { "cache-control": "public, max-age=3600" },
  );
}

function authorizationServer(base) {
  return jsonOut(
    {
      issuer: base,
      authorization_endpoint: `${base}/oauth/authorize`,
      token_endpoint: `${base}/oauth/token`,
      registration_endpoint: `${base}/oauth/register`,
      revocation_endpoint: `${base}/oauth/revoke`,
      client_id_metadata_document_supported: true,
      // Every redirect back to a client -- a code, a refusal, an error --
      // carries iss (RFC 9207). ChatGPT then uses its one fixed redirect,
      // https://chatgpt.com/connector_platform_oauth_redirect.
      authorization_response_iss_parameter_supported: true,
      response_types_supported: ["code"],
      response_modes_supported: ["query"],
      grant_types_supported: ["authorization_code", "refresh_token"],
      code_challenge_methods_supported: ["S256"],
      token_endpoint_auth_methods_supported: ["none", "client_secret_post", "client_secret_basic"],
      revocation_endpoint_auth_methods_supported: ["none", "client_secret_post", "client_secret_basic"],
      scopes_supported: [MCP_SCOPE],
      service_documentation: "https://krate.tech",
    },
    200,
    { "cache-control": "public, max-age=3600" },
  );
}

// ------------------------------------------------------------ registration

/** A redirect a client may register: https, or a loopback address for a local client. */
function redirectOk(uri) {
  try {
    const u = new URL(uri);
    if (u.hash) return false;
    if (u.protocol === "https:") return true;
    return u.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(u.hostname);
  } catch (_) {
    return false;
  }
}

/** A client's name as a page may show it: no control characters, at most 80 characters. */
const cleanName = (v) => String(v || "").replace(/[\u0000-\u001f\u007f]/g, "").trim().slice(0, 80);

/** An address on this computer: a redirect there can be claimed by any program running on it. */
function loopback(uri) {
  try {
    return ["localhost", "127.0.0.1", "[::1]"].includes(new URL(uri).hostname);
  } catch (_) {
    return false;
  }
}

async function readBody(request) {
  const raw = await request.text();
  if (raw.length > 16384) return null;
  const type = request.headers.get("content-type") || "";
  if (type.includes("application/json")) {
    try {
      const v = JSON.parse(raw || "{}");
      return v && typeof v === "object" && !Array.isArray(v) ? v : null;
    } catch (_) {
      return null;
    }
  }
  return Object.fromEntries(new URLSearchParams(raw));
}

const clientSecretOf = async (env, clientId) => b64url((await sha256(`secret:${clientId}:${(await sealKeyId(env))}`)).subarray(0, 24));
/** A value that changes when the sealing secret does, so a secret outlives no rotation. */
async function sealKeyId(env) {
  return b64url((await sha256(env.MCP_OAUTH_SECRET || `derived:${env.KEY_WRAP_SECRET || ""}`)).subarray(0, 12));
}

async function register(request, env) {
  const body = await readBody(request);
  if (!body) return oauthError("invalid_client_metadata", "The registration is not a JSON object.");
  const uris = body.redirect_uris;
  if (!Array.isArray(uris) || !uris.length || uris.length > 10 || !uris.every((u) => typeof u === "string" && u.length <= 512 && redirectOk(u))) {
    return oauthError("invalid_redirect_uri", "redirect_uris: one to ten https addresses (or a loopback http address).");
  }
  const method = ["none", "client_secret_post", "client_secret_basic"].includes(body.token_endpoint_auth_method)
    ? body.token_endpoint_auth_method
    : "none";
  const name = cleanName(body.client_name) || "An MCP client";
  const issued = now();
  const clientId = await seal(env, "c", { r: uris, n: name, m: method, t: issued });
  const answer = {
    client_id: clientId,
    client_id_issued_at: issued,
    client_name: name,
    redirect_uris: uris,
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
    token_endpoint_auth_method: method,
    scope: MCP_SCOPE,
  };
  if (method !== "none") {
    answer.client_secret = await clientSecretOf(env, clientId);
    answer.client_secret_expires_at = 0;
  }
  return jsonOut(answer, 201);
}

// ------------------------------------------ client ID metadata documents
//
// A client may name itself by an https address instead of registering:
// OAuth Client ID Metadata Documents (draft-ietf-oauth-client-id-metadata-
// document), which MCP 2025-11-25 recommends and claude.ai offers as "Use
// Claude's published identity". The hub fetches that address and trusts
// the JSON there only when its client_id is the address byte for byte, so
// a document can speak only for the host that serves it.
//
// Only public clients: PKCE is the proof, there is no secret to hold, and
// a document that carries a secret or asks for any other way of signing in
// at the token endpoint is refused. A good document is kept for a while
// (its own Cache-Control, five minutes when it says nothing, a day at most)
// in this isolate and in the Cache API -- never KV, whose writes are
// counted. A failed fetch or a bad document is never kept.

const CIMD_MAX_BYTES = 5 * 1024;
const CIMD_TIMEOUT_MS = 5000;
const CIMD_DEFAULT_TTL = 5 * 60;
const CIMD_MAX_TTL = 24 * 60 * 60;
const CIMD_MEMORY_MAX = 200;
/** address -> { client, until (ms) } */
const cimdMemory = new Map();

/** A client_id that is an address rather than a sealed registration. */
const isAddressId = (id) => typeof id === "string" && /^[a-z][a-z0-9+.-]*:\/\//i.test(id);

/** Why an address cannot be a client_id, in plain words, or null when it can. */
export function clientIdAddressProblem(id) {
  if (id.length > 1024) return "the address is too long";
  let u;
  try {
    u = new URL(id);
  } catch (_) {
    return "the address is not a valid URL";
  }
  if (u.protocol !== "https:") return "the address must start with https://";
  if (u.username || u.password) return "the address must not carry a user name or password";
  if (id.includes("#")) return "the address must not have a #fragment";
  if (u.pathname === "/") return "the address needs a path, like https://example.com/client.json";
  // The plain form, compared byte for byte: no dot segments, a lower-case
  // host, no default port. The document must name exactly this string.
  if (u.href !== id) return `the address is not in its plain form (${u.href})`;
  const host = u.hostname;
  if (host.startsWith("[") || /^[\d.]+$/.test(host) || !host.includes(".") || /\.(local|localhost|internal|lan|home|arpa|invalid|test)$/i.test(host)) {
    return "the address must be on a public host name, not an IP address or a local name";
  }
  return null;
}

/** How long a document may be kept, from its Cache-Control: 0 means not at all. */
function keepSeconds(header) {
  const cc = String(header || "").toLowerCase();
  if (/(^|[\s,])(no-store|no-cache)([\s,=]|$)/.test(cc)) return 0;
  const m = /(^|[\s,])max-age\s*=\s*"?(\d+)/.exec(cc);
  if (m) return Math.min(Number(m[2]), CIMD_MAX_TTL);
  return CIMD_DEFAULT_TTL;
}

/** At most `max` bytes of a response body, or null when it is longer. */
async function readCapped(res, max) {
  if (!res.body) return new Uint8Array(0);
  const reader = res.body.getReader();
  const parts = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      try {
        await reader.cancel();
      } catch (_) {}
      return null;
    }
    parts.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.byteLength;
  }
  return out;
}

/** The client a fetched document describes, or { error } in plain words. */
function clientOfDocument(id, doc) {
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) return { error: "its document is not a JSON object" };
  if (doc.client_id !== id) return { error: "its document's client_id is not the address it was fetched from" };
  if ("client_secret" in doc || "client_secret_expires_at" in doc) {
    return { error: "its document carries a client secret, which a published identity must never do" };
  }
  const method = doc.token_endpoint_auth_method === undefined ? "none" : doc.token_endpoint_auth_method;
  if (method !== "none") {
    return {
      error: `it asks to sign in at the token endpoint with "${String(method).slice(0, 40)}"; Krate takes only public clients ("none"), which prove themselves with PKCE`,
    };
  }
  const uris = doc.redirect_uris;
  if (!Array.isArray(uris) || !uris.length || uris.length > 10 || !uris.every((u) => typeof u === "string" && u.length <= 512 && redirectOk(u))) {
    return { error: "its document's redirect_uris must be one to ten https addresses (or a loopback http address)" };
  }
  if (doc.grant_types !== undefined && !(Array.isArray(doc.grant_types) && doc.grant_types.includes("authorization_code"))) {
    return { error: "its document's grant_types do not include authorization_code" };
  }
  if (doc.response_types !== undefined && !(Array.isArray(doc.response_types) && doc.response_types.includes("code"))) {
    return { error: "its document's response_types do not include code" };
  }
  const name = typeof doc.client_name === "string" ? cleanName(doc.client_name) : "";
  if (!name) return { error: "its document has no client_name" };
  let logo = null;
  if (typeof doc.logo_uri === "string" && doc.logo_uri.length <= 512) {
    try {
      const l = new URL(doc.logo_uri);
      if (l.protocol === "https:" && !l.username && !l.password) logo = l.href;
    } catch (_) {}
  }
  return { client: { id, r: uris, n: name, m: "none", h: new URL(id).host, l: logo, cimd: true } };
}

/** Fetch and check the document at `id`: { client, ttl } or { error, transient }. */
async function fetchClientDocument(id) {
  let res;
  let bytes;
  try {
    res = await fetch(id, {
      method: "GET",
      redirect: "manual",
      headers: { accept: "application/json" },
      signal: AbortSignal.timeout(CIMD_TIMEOUT_MS),
    });
    if (res.status >= 300 && res.status < 400) {
      return { error: "its address redirects somewhere else; the document must be served at the address itself" };
    }
    if (res.status >= 500 || res.status === 429) return { error: `its address answered ${res.status}; try again shortly`, transient: true };
    if (res.status !== 200) return { error: `its address answered ${res.status} instead of the document` };
    const type = (res.headers.get("content-type") || "").toLowerCase();
    if (!/^application\/([a-z0-9.-]+\+)?json(\s*;|$)/.test(type)) return { error: "its address did not answer with JSON" };
    if (Number(res.headers.get("content-length")) > CIMD_MAX_BYTES) return { error: "its document is larger than 5 KB" };
    bytes = await readCapped(res, CIMD_MAX_BYTES);
  } catch (_) {
    return { error: "its address did not answer in time", transient: true };
  }
  if (!bytes) return { error: "its document is larger than 5 KB" };
  let doc;
  try {
    doc = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch (_) {
    return { error: "its document is not valid JSON" };
  }
  const got = clientOfDocument(id, doc);
  if (got.error) return got;
  return { client: got.client, ttl: keepSeconds(res.headers.get("cache-control")) };
}

const cimdCacheKey = (base, id) => `${base || "https://hub.invalid"}/__client-metadata/${encodeURIComponent(id)}`;
const edgeCache = () => (typeof caches !== "undefined" && caches.default ? caches.default : null);

/** The client a metadata document at `id` describes: kept copy, or a fresh fetch. */
async function clientOfAddress(id, base) {
  const t = Date.now();
  const held = cimdMemory.get(id);
  if (held && held.until > t) return { client: held.client };
  if (held) cimdMemory.delete(id);
  const cache = edgeCache();
  if (cache) {
    try {
      const hit = await cache.match(cimdCacheKey(base, id));
      if (hit) {
        const kept = await hit.json();
        if (kept && kept.client && kept.client.id === id && kept.until > t) {
          remember(id, kept.client, kept.until);
          return { client: kept.client };
        }
      }
    } catch (_) {}
  }
  const got = await fetchClientDocument(id);
  if (!got.client) return got;
  if (got.ttl > 0) {
    const until = t + got.ttl * 1000;
    remember(id, got.client, until);
    if (cache) {
      try {
        await cache.put(
          cimdCacheKey(base, id),
          new Response(JSON.stringify({ client: got.client, until }), {
            headers: { "content-type": "application/json", "cache-control": `max-age=${got.ttl}` },
          }),
        );
      } catch (_) {}
    }
  }
  return { client: got.client };
}

function remember(id, client, until) {
  if (cimdMemory.size >= CIMD_MEMORY_MAX) cimdMemory.delete(cimdMemory.keys().next().value);
  cimdMemory.set(id, { client, until });
}

/** Tests: forget every kept document. */
export function __forgetClientDocuments() {
  cimdMemory.clear();
}

/**
 * The client an id names: a sealed registration, or the metadata document at
 * an https address. { client } or { error, transient } -- error is null for
 * an id that is simply not ours.
 */
async function clientById(env, base, id) {
  if (isAddressId(id)) {
    const problem = clientIdAddressProblem(id);
    if (problem) return { error: problem };
    return clientOfAddress(id, base);
  }
  const c = await open(env, "c", id);
  return c ? { client: { id, ...c } } : { error: null };
}

/**
 * The client a token or revoke request names, checking its secret when it
 * registered one: { client } or { error, transient }.
 */
async function clientFrom(env, base, request, body) {
  let id = body.client_id || "";
  let secret = body.client_secret || "";
  const basic = /^Basic\s+(.+)$/i.exec(request.headers.get("authorization") || "");
  if (basic) {
    try {
      const [u, p] = atob(basic[1]).split(":");
      id = decodeURIComponent(u || "");
      secret = decodeURIComponent(p || "");
    } catch (_) {
      return { error: null };
    }
  }
  const found = await clientById(env, base, id);
  if (!found.client) return found;
  const client = found.client;
  // A published identity is a public client; nothing it sends as a secret
  // means anything, and the sealed registrations below hold the only secrets.
  if (client.m !== "none") {
    const want = await clientSecretOf(env, id);
    if (!secret || secret !== want) return { error: null };
  }
  return { client };
}

/** The token endpoint's answer for a client it cannot use. */
function clientRefused(found, fallback) {
  if (found.transient) {
    return oauthError("temporarily_unavailable", `The app's published identity could not be read: ${found.error}.`, 503);
  }
  if (found.error) return oauthError("invalid_client", `The app's published identity cannot be used: ${found.error}.`, 401);
  return oauthError("invalid_client", fallback, 401);
}

const clientTag = async (clientId) => b64url((await sha256(clientId)).subarray(0, 12));

// --------------------------------------------------------------- authorize

function page(title, body, script = "") {
  return new Response(
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)}</title><meta name="robots" content="noindex">
<style>
:root{--bg:#fafafa;--fg:#111;--muted:#666;--line:#e5e5e5;--card:#fff;--accent:#111;--on-accent:#fff}
@media (prefers-color-scheme: dark){:root{--bg:#0b0b0c;--fg:#f2f2f2;--muted:#a0a0a0;--line:#26262a;--card:#141416;--accent:#f2f2f2;--on-accent:#111}}
*{box-sizing:border-box}body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,system-ui,sans-serif;padding:16px}
main{width:100%;max-width:400px;background:var(--card);border:1px solid var(--line);border-radius:16px;padding:28px 24px}
h1{font-size:20px;margin:0 0 6px;letter-spacing:-.01em}p{margin:0 0 14px;color:var(--muted)}.may{color:var(--fg);border:1px solid var(--line);border-radius:12px;padding:12px 14px;margin:0 0 18px}
button,.btn{display:block;width:100%;padding:12px;border-radius:999px;border:1px solid var(--line);background:var(--card);color:var(--fg);font:600 15px system-ui,sans-serif;cursor:pointer;margin:0 0 10px;text-align:center;text-decoration:none}
.primary{background:var(--accent);color:var(--on-accent);border-color:var(--accent)}input{width:100%;padding:11px 12px;border-radius:10px;border:1px solid var(--line);background:var(--bg);color:var(--fg);font:15px system-ui,sans-serif;margin:0 0 10px}
small{color:var(--muted);display:block;margin-top:6px;overflow-wrap:anywhere}b{overflow-wrap:anywhere}
.warn{color:var(--fg);border:1px solid #d97706;border-radius:12px;padding:10px 14px}.logo{width:40px;height:40px;border-radius:8px;display:block;margin:0 0 10px;object-fit:contain}
</style></head><body><main>${body}</main>${script ? `<script>${script}</script>` : ""}</body></html>`,
    {
      status: 200,
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
        "referrer-policy": "no-referrer",
        "x-frame-options": "DENY",
        "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src https:; connect-src 'self'; form-action 'self'; frame-ancestors 'none'",
      },
    },
  );
}

const errorPage = (words) => {
  const res = page("Krate sign-in", `<h1>This sign-in cannot go on</h1><p>${esc(words)}</p>`);
  return new Response(res.body, { status: 400, headers: res.headers });
};

function redirectWith(uri, params) {
  const u = new URL(uri);
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== "") u.searchParams.set(k, v);
  return u.toString();
}

async function authorize(request, url, env, base, deps) {
  const q = url.searchParams;
  const clientId = q.get("client_id") || "";
  // An address makes the hub fetch something; that has its own ceiling.
  if (isAddressId(clientId)) {
    const over = await deps.rateLimited(request, "oauth");
    if (over) return over;
  }
  const found = await clientById(env, base, clientId);
  const client = found.client;
  // A client or a redirect that cannot be trusted gets a page, never a
  // redirect: sending the browser to an address nobody registered is how
  // an authorization server becomes an open redirector.
  if (!client) {
    if (found.transient) return errorPage(`The app's published identity at ${clientId} could not be read: ${found.error}. Try again in a moment.`);
    if (found.error) return errorPage(`The app's published identity at ${clientId} cannot be used: ${found.error}.`);
    return errorPage("The app asking to sign you in is not registered with Krate. Connect it again from the app.");
  }
  let redirect = q.get("redirect_uri") || "";
  if (!redirect && client.r.length === 1) redirect = client.r[0];
  if (!client.r.includes(redirect)) {
    return errorPage(
      client.cimd
        ? `The app asked to come back to ${redirect || "an address it did not name"}, which its published identity at ${client.h} does not list.`
        : "The app asked to come back to an address it did not register.",
    );
  }
  const state = q.get("state") || "";
  const fail = (error, description) =>
    Response.redirect(redirectWith(redirect, { error, error_description: description, state, iss: base }), 302);
  if (q.get("response_type") !== "code") return fail("unsupported_response_type", "only response_type=code");
  const challenge = q.get("code_challenge") || "";
  if (q.get("code_challenge_method") !== "S256" || !/^[A-Za-z0-9_-]{43,128}$/.test(challenge)) {
    return fail("invalid_request", "PKCE with code_challenge_method=S256 is required");
  }
  const resource = q.get("resource");
  if (resource && resource.replace(/\/$/, "") !== resourceOf(base)) return fail("invalid_target", `the resource is ${resourceOf(base)}`);
  const scope = q.get("scope");
  if (scope && !scope.split(/\s+/).every((s) => s === MCP_SCOPE)) return fail("invalid_scope", `the scope is ${MCP_SCOPE}`);
  const host = new URL(redirect).host;
  // Who is asking, as the person must see it: a name is only a claim, so a
  // published identity shows the host that vouches for it beside the name,
  // and the host the browser goes back to whenever that is a different one.
  const who = { n: client.n, h: client.cimd ? client.h : null, back: host, local: loopback(redirect) };
  const req = await seal(env, "r", {
    c: clientId, u: redirect, ch: challenge, s: state, n: client.n, h: who.h, e: now() + REQUEST_TTL,
  });
  const lines = [];
  if (client.cimd) {
    lines.push(
      `<p>${client.l ? `<img class="logo" src="${esc(client.l)}" alt="" width="40" height="40">` : ""}<b>${esc(client.n)}</b>, from <b>${esc(client.h)}</b>, wants to use your Krate account.</p>`,
    );
    if (host !== client.h) lines.push(`<p>It will send you back to <b>${esc(host)}</b>.</p>`);
  } else {
    lines.push(`<p><b>${esc(client.n)}</b> (${esc(host)}) wants to use your Krate account.</p>`);
  }
  if (who.local) {
    lines.push(`<p class="warn">It sends you back to a program on this computer (${esc(host)}). Go on only if you started this from an app on this computer.</p>`);
  }
  const body = `
<h1>Sign in to Krate</h1>
${lines.join("\n")}
<p class="may">It may: ${esc(CONSENT_WORDS)}.</p>
<a class="btn primary" id="gh" href="#">Continue with GitHub</a>
<a class="btn" id="gg" href="#">Continue with Google</a>
<form id="em"><input type="email" id="addr" placeholder="you@example.com" autocomplete="email" required><button type="submit">Email me a sign-in link</button></form>
<p id="sent" hidden>Check your inbox and open the link on this device.</p>
<small>You will see this again and can say no before anything is allowed.</small>`;
  const script = `
const REQ=${JSON.stringify(req)};const WHO=${JSON.stringify(who).replace(/</g, "\\u003c")};
function begin(){const n=[...crypto.getRandomValues(new Uint8Array(16))].map(b=>b.toString(16).padStart(2,"0")).join("");
try{localStorage.setItem("krate-oauth:"+n,JSON.stringify({req:REQ,who:WHO,at:Date.now()}));}catch(e){}return n;}
document.getElementById("gh").onclick=e=>{e.preventDefault();location.href="/login/start?from=oauth&nonce="+begin();};
document.getElementById("gg").onclick=e=>{e.preventDefault();location.href="/login/google/start?from=oauth&nonce="+begin();};
document.getElementById("em").onsubmit=async e=>{e.preventDefault();const n=begin();
const r=await fetch("/login/email",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({email:document.getElementById("addr").value,from:"oauth",nonce:n})});
if(r.ok){document.getElementById("em").hidden=true;document.getElementById("sent").hidden=false;}else{alert(await r.text());}};`;
  return page("Sign in to Krate", body, script);
}

/** Where a finished sign-in lands: the page checks its nonce, then asks yes or no. */
function authorizeDone() {
  const body = `
<h1 id="h">Allow access?</h1>
<p id="app" hidden></p>
<p class="warn" id="local" hidden></p>
<p id="who"></p>
<p class="may" id="may" hidden>It may: ${esc(CONSENT_WORDS)}.</p>
<button class="primary" id="yes" hidden>Allow</button>
<button id="no" hidden>Cancel</button>`;
  const script = `
const f=new URLSearchParams(location.hash.slice(1));history.replaceState(null,"",location.pathname);
const n=f.get("nonce")||"";let held=null;try{held=JSON.parse(localStorage.getItem("krate-oauth:"+n)||"null");}catch(e){}
const h=document.getElementById("h"),who=document.getElementById("who");
if(!n||!held||!f.get("token")||Date.now()-held.at>30*60*1000){h.textContent="This sign-in did not start here";who.textContent="Go back to the app and connect Krate again.";}
else{who.textContent="Signed in as "+(f.get("login")||"you")+".";for(const id of ["may","yes","no"])document.getElementById(id).hidden=false;
const w=held.who;if(w&&w.n){const app=document.getElementById("app");app.textContent=w.n+(w.h?", from "+w.h+",":" ("+w.back+")")+" wants to use your Krate account."+(w.h&&w.back!==w.h?" It will send you back to "+w.back+".":"");app.hidden=false;
if(w.local){const l=document.getElementById("local");l.textContent="It sends you back to a program on this computer ("+w.back+"). Allow it only if you started this from an app on this computer.";l.hidden=false;}}
const go=async ok=>{for(const id of ["yes","no"])document.getElementById(id).disabled=true;
const r=await fetch("/oauth/approve",{method:"POST",headers:{"content-type":"application/json",authorization:"Bearer "+f.get("token")},body:JSON.stringify({req:held.req,approve:ok})});
try{localStorage.removeItem("krate-oauth:"+n);}catch(e){}
const a=await r.json().catch(()=>({}));if(a.redirect)location.replace(a.redirect);else{h.textContent="That did not work";who.textContent=a.error_description||"Connect Krate again from the app.";}};
document.getElementById("yes").onclick=()=>go(true);document.getElementById("no").onclick=()=>go(false);}`;
  return page("Allow Krate", body, script);
}

/**
 * The person's yes or no. `deps.whoIs(request)` reads their hub session and
 * answers the identity the hub publishes as: { owner, uid, login, name,
 * avatar_url, github, iid } or null.
 */
async function approve(request, env, base, deps) {
  const body = await readBody(request);
  if (!body) return oauthError("invalid_request", "expected a JSON object");
  const req = await open(env, "r", body.req);
  if (!req) return oauthError("invalid_request", "This sign-in has expired. Connect Krate again from the app.");
  if (body.approve !== true) {
    return jsonOut({ redirect: redirectWith(req.u, { error: "access_denied", error_description: "the person said no", state: req.s, iss: base }) });
  }
  const who = await deps.whoIs(request);
  if (!who || !who.owner || !who.uid) return oauthError("access_denied", "Sign in first.", 401);
  const grant = randomHex(16);
  const code = await seal(env, "k", {
    g: grant, c: await clientTag(req.c), u: req.u, ch: req.ch, n: req.n, h: req.h || null, id: who, e: now() + CODE_TTL,
  });
  return jsonOut({ redirect: redirectWith(req.u, { code, state: req.s, iss: base }) });
}

// ------------------------------------------------------------------- tokens

async function issue(env, base, { grant, gen, clientTag: tag, who }) {
  const access = await seal(env, "a", { g: grant, c: tag, id: who, aud: resourceOf(base), e: now() + ACCESS_TTL });
  const refresh = await seal(env, "f", { g: grant, gen, c: tag, id: who, e: now() + REFRESH_TTL });
  return jsonOut({ access_token: access, token_type: "Bearer", expires_in: ACCESS_TTL, refresh_token: refresh, scope: MCP_SCOPE });
}

const grantKey = (g) => `mcpgrant:${g}`;
const revokedKey = (g) => `mcprevoked:${g}`;

async function revokeGrant(env, g) {
  await env.APPS.put(revokedKey(g), "1", { expirationTtl: REFRESH_TTL + 86400 });
  await env.APPS.delete(grantKey(g));
}

/**
 * End every connector an account allowed, for its deletion (K-1304). A
 * refresh token carries the identity it was made for, so without this a
 * deleted account's Claude connector kept minting access tokens for up to
 * 30 days. Answers how many grants were ended.
 */
export async function endConnectorGrants(env, uid) {
  const listKey = `mcpgrants:${uid}`;
  const held = JSON.parse((await env.APPS.get(listKey)) || "[]");
  let ended = 0;
  for (const g of held) {
    if (!g || !g.g) continue;
    await revokeGrant(env, g.g);
    ended += 1;
  }
  await env.APPS.delete(listKey);
  return ended;
}

async function token(request, env, base) {
  const body = await readBody(request);
  if (!body) return oauthError("invalid_request", "expected a form body");
  const found = await clientFrom(env, base, request, body);
  if (!found.client) return clientRefused(found, "unknown client, or a wrong client secret");
  const client = found.client;
  const tag = await clientTag(client.id);

  if (body.grant_type === "authorization_code") {
    const code = await open(env, "k", body.code);
    if (!code || code.c !== tag) return oauthError("invalid_grant", "the code is wrong, expired or not this client's");
    if ((body.redirect_uri || code.u) !== code.u) return oauthError("invalid_grant", "redirect_uri does not match the authorization request");
    const verifier = String(body.code_verifier || "");
    if (!/^[A-Za-z0-9._~-]{43,128}$/.test(verifier) || b64url(await sha256(verifier)) !== code.ch) {
      return oauthError("invalid_grant", "code_verifier does not match the code_challenge");
    }
    // A code is good once. A second use is a stolen code: the grant ends.
    if ((await env.APPS.get(grantKey(code.g))) || (await env.APPS.get(revokedKey(code.g)))) {
      await revokeGrant(env, code.g);
      return oauthError("invalid_grant", "this code was already used");
    }
    const record = { gen: 1, owner: code.id.owner, uid: code.id.uid, client: code.n, at: now() };
    await env.APPS.put(grantKey(code.g), JSON.stringify(record), { expirationTtl: REFRESH_TTL });
    // The account's own list of what it allowed, for its sign-in page.
    try {
      const listKey = `mcpgrants:${code.id.uid}`;
      const held = JSON.parse((await env.APPS.get(listKey)) || "[]").filter((g) => g && g.at > now() - REFRESH_TTL);
      held.unshift({ g: code.g, client: code.n, ...(code.h ? { from: code.h } : {}), at: record.at });
      await env.APPS.put(listKey, JSON.stringify(held.slice(0, 50)));
    } catch (_) {}
    return issue(env, base, { grant: code.g, gen: 1, clientTag: tag, who: code.id });
  }

  if (body.grant_type === "refresh_token") {
    const rt = await open(env, "f", body.refresh_token);
    if (!rt || rt.c !== tag) return oauthError("invalid_grant", "the refresh token is wrong, expired or not this client's");
    if (await env.APPS.get(revokedKey(rt.g))) return oauthError("invalid_grant", "access was taken back");
    let record = null;
    try {
      record = JSON.parse((await env.APPS.get(grantKey(rt.g))) || "null");
    } catch (_) {
      record = null;
    }
    // Rotation: only the newest refresh token works; an older one used
    // again means two holders, and the grant ends for both.
    if (record && rt.gen !== record.gen) {
      await revokeGrant(env, rt.g);
      return oauthError("invalid_grant", "this refresh token was already used");
    }
    const gen = (record ? record.gen : rt.gen) + 1;
    await env.APPS.put(
      grantKey(rt.g),
      JSON.stringify({ ...(record || { owner: rt.id.owner, uid: rt.id.uid, at: now() }), gen }),
      { expirationTtl: REFRESH_TTL },
    );
    return issue(env, base, { grant: rt.g, gen, clientTag: tag, who: rt.id });
  }
  return oauthError("unsupported_grant_type", "authorization_code or refresh_token");
}

async function revoke(request, env, base) {
  const body = await readBody(request);
  if (!body) return oauthError("invalid_request", "expected a form body");
  const found = await clientFrom(env, base, request, body);
  if (!found.client) return clientRefused(found, "unknown client");
  const tag = await clientTag(found.client.id);
  const t = (await open(env, "a", body.token)) || (await open(env, "f", body.token));
  // RFC 7009: an unknown token is not an error.
  if (t && t.c === tag) await revokeGrant(env, t.g);
  return new Response(null, { status: 200, headers: { ...CORS, "cache-control": "no-store" } });
}

/**
 * Who a /mcp request is signed in as. null when it carries no token;
 * { invalid: true } for a token that is not a live access token for this
 * resource; otherwise { grant, who }.
 */
export async function bearerOf(request, env, base) {
  const header = request.headers.get("authorization") || "";
  if (!header) return null;
  const m = /^Bearer\s+(\S+)$/i.exec(header);
  const t = m ? await open(env, "a", m[1]) : null;
  if (!t || t.aud !== resourceOf(base) || !t.id || !t.id.owner) return { invalid: true };
  if (await env.APPS.get(revokedKey(t.g))) return { invalid: true };
  return { grant: t.g, who: t.id };
}

/** The connectors an account allowed, and taking one back (signed in to the hub). */
export async function connectorsRoute(request, env, user, grant) {
  const listKey = `mcpgrants:${user.id}`;
  const held = JSON.parse((await env.APPS.get(listKey)) || "[]");
  if (request.method === "GET" && !grant) {
    const live = [];
    for (const g of held) {
      if (!g || g.at <= now() - REFRESH_TTL) continue;
      if (await env.APPS.get(revokedKey(g.g))) continue;
      live.push({ id: g.g, client: g.client, ...(g.from ? { from: g.from } : {}), allowed_at: g.at, may: CONSENT_WORDS });
    }
    return jsonOut({ connectors: live });
  }
  if (request.method === "DELETE" && grant) {
    if (!held.some((g) => g && g.g === grant)) return oauthError("not_found", "no such connector on this account", 404);
    await revokeGrant(env, grant);
    await env.APPS.put(listKey, JSON.stringify(held.filter((g) => g && g.g !== grant)));
    return jsonOut({ ok: true, revoked: grant });
  }
  return oauthError("invalid_request", "GET the list, or DELETE one", 405);
}

/** Every OAuth route, or null when the path is not one. */
export async function oauthRoute(request, url, env, deps) {
  const base = deps.base;
  const { pathname } = url;
  const method = request.method;
  if (method === "OPTIONS" && (pathname.startsWith("/oauth/") || pathname.startsWith("/.well-known/oauth-"))) {
    return new Response(null, { status: 204, headers: CORS });
  }
  if (method === "GET" && (pathname === "/.well-known/oauth-protected-resource" || pathname === "/.well-known/oauth-protected-resource/mcp")) {
    return protectedResource(base);
  }
  if (method === "GET" && (pathname === "/.well-known/oauth-authorization-server" || pathname === "/.well-known/oauth-authorization-server/mcp")) {
    return authorizationServer(base);
  }
  if (!pathname.startsWith("/oauth/")) return null;
  if (!(await sealKey(env))) return oauthError("temporarily_unavailable", "Sign-in for connectors is not set up on this hub yet.", 503);
  if (method === "GET" && pathname === "/oauth/authorize") return authorize(request, url, env, base, deps);
  if (method === "GET" && pathname === "/oauth/authorize/done") return authorizeDone();
  if (method === "POST") {
    const over = await deps.rateLimited(request, "oauth");
    if (over) return over;
    if (pathname === "/oauth/register") return register(request, env);
    if (pathname === "/oauth/approve") return approve(request, env, base, deps);
    if (pathname === "/oauth/token") return token(request, env, base);
    if (pathname === "/oauth/revoke") return revoke(request, env, base);
  }
  return oauthError("invalid_request", "not an OAuth endpoint", 404);
}
