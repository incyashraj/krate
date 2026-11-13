// Telling a company API who is calling (SMART-FIXES section 5).
//
// A company connection (IC-952) holds one credential for the whole
// organisation, so on its own the API sees only "the app". With this, each
// request an app makes through a connection also carries `Krate-Identity`:
// a short-lived token, signed by this hub, saying which person, in which
// organisation, through which app and release, over which connection. The
// API can log the real person and check the token against the public keys
// at /.well-known/jwks.json. The API owner does no OAuth work.
//
// The token is a compact JWT (RFC 7519) signed with EdDSA over Ed25519
// (RFC 8037). Its key is not a new secret: it is derived from the lease
// key's seed (ORG_LEASE_KEY) with HKDF-SHA256 and its own info string, so
// the lease key itself never signs anything but leases, and a token can
// never pass as a lease or a lease as a token.
//
// The token is information, not access: the credential still decides
// whether a request gets in. A token is only issued after every check the
// credential fetch makes (org-connections.js op_connection_use), and only
// for a connection whose "Tell the API who is calling" setting is on.

/// How long a token is good for. Five minutes: short enough that a person
/// removed from the organisation stops being named on new requests within
/// minutes (their next refresh is refused), long enough that a running app
/// asks the hub about once every four minutes, not on every request. The
/// device refreshes once fewer than IDENTITY_REFRESH_SECONDS remain.
export const IDENTITY_SECONDS = 5 * 60;
export const IDENTITY_REFRESH_SECONDS = 60;

/// The HKDF info that makes the identity key from the lease key's seed.
/// Fixed forever for this key: changing it changes every `kid`.
export const IDENTITY_INFO = "krate identity token v1";

export const IDENTITY_HEADER = "Krate-Identity";

const hex = (bytes) => [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
const unhex = (text) => Uint8Array.from(String(text).match(/../g) || [], (h) => parseInt(h, 16));

export function b64url(bytes) {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let s = "";
  for (let i = 0; i < u8.length; i += 1) s += String.fromCharCode(u8[i]);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function unb64url(text) {
  const s = String(text).replace(/-/g, "+").replace(/_/g, "/");
  return Uint8Array.from(atob(s + "=".repeat((4 - (s.length % 4)) % 4)), (c) => c.charCodeAt(0));
}

const utf8 = (text) => new TextEncoder().encode(text);

/// The identity key: { privateKey, x (the public key, base64url), kid },
/// or null when the hub has no ORG_LEASE_KEY. Derived, never stored.
export async function identityKey(env) {
  const seed = String((env && env.ORG_LEASE_KEY) || "").trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(seed)) return null;
  const ikm = await crypto.subtle.importKey("raw", unhex(seed), "HKDF", false, ["deriveBits"]);
  const derived = new Uint8Array(
    await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: new Uint8Array(0), info: utf8(IDENTITY_INFO) }, ikm, 256),
  );
  // PKCS#8 for an Ed25519 seed is a fixed 16-byte prefix and the seed.
  const pkcs8 = new Uint8Array([...unhex("302e020100300506032b657004220420"), ...derived]);
  const privateKey = await crypto.subtle.importKey("pkcs8", pkcs8, { name: "Ed25519" }, true, ["sign"]);
  const { x } = await crypto.subtle.exportKey("jwk", privateKey);
  // A stable id from the public key alone: the same seed always gives the
  // same kid, and a new seed a new one.
  const kid = hex(await crypto.subtle.digest("SHA-256", unb64url(x))).slice(0, 16);
  return { privateKey, x, kid };
}

/// The public keys an API checks a token with (RFC 7517).
export async function jwks(env) {
  const key = await identityKey(env);
  return { keys: key ? [{ kty: "OKP", crv: "Ed25519", x: key.x, kid: key.kid, alg: "EdDSA", use: "sig" }] : [] };
}

/// Sign one token for what the store decided (`facts`: sub, org, app,
/// release, connection, aud, now). Answers { token, expires_in }, or null
/// when the hub has no key.
export async function issueIdentity(env, facts) {
  const key = await identityKey(env);
  if (!key) return null;
  const iat = Number(facts.now);
  for (const k of ["sub", "org", "app", "release", "connection", "aud"]) {
    if (typeof facts[k] !== "string" || !facts[k]) throw new Error(`refusing to sign an identity token without ${k}`);
  }
  const header = { alg: "EdDSA", typ: "JWT", kid: key.kid };
  const claims = {
    iss: String((env && env.PUBLIC_BASE) || "").replace(/\/$/, ""),
    sub: facts.sub,
    aud: facts.aud,
    iat,
    nbf: iat,
    exp: iat + IDENTITY_SECONDS,
    jti: hex(crypto.getRandomValues(new Uint8Array(16))),
    org: facts.org,
    app: facts.app,
    release: facts.release,
    connection: facts.connection,
  };
  const signing = `${b64url(utf8(JSON.stringify(header)))}.${b64url(utf8(JSON.stringify(claims)))}`;
  const signature = await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, utf8(signing));
  return { token: `${signing}.${b64url(signature)}`, expires_in: IDENTITY_SECONDS };
}

/// GET /.well-known/jwks.json. Public, cacheable for five minutes.
export async function jwksRoute(env) {
  return new Response(JSON.stringify(await jwks(env)), {
    headers: { "content-type": "application/json", "cache-control": "public, max-age=300" },
  });
}
