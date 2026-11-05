// A signed-in person's own app data on every device they have (IC-981,
// F-321): the same reminders on the Mac and the phone.
//
//   GET    /account/sync/keys[?device=<id>]     the account's key, devices, this device's envelope
//   PUT    /account/sync/devices/<id>           register a device {public_key, name}
//   DELETE /account/sync/devices/<id>           forget a device and its envelope
//   POST   /account/sync/init                   the account's first key, made on a device
//   POST   /account/sync/grants                 a device approved: the key sealed to it
//   GET    /account/sync/recovery               the recovery envelope and its parameters
//   GET    /account/sync/buckets/<id>[?since=n] one app's entries
//   PUT    /account/sync/buckets/<id>[?since=n] {writes} -- one app's changes, then its entries
//
// The hub never holds a key that opens anything. It keeps, per account:
//
// - each device's X25519 public key and name;
// - the account key's PUBLIC half, and one envelope per device that was
//   approved: the key sealed on a device to that device's public key with
//   HPKE (crates/orgkeys). A new device gets one only when a device that
//   holds the key seals it -- the person said yes there -- or when the
//   recovery code opened it on the new device itself;
// - the recovery envelope, beside its public Argon2id parameters;
// - per app, a bucket named by an HMAC of (publisher, app id) the hub
//   cannot reverse, holding per entry name (an HMAC of the key) every
//   version no other version replaces: a version vector, its writer, and
//   ciphertext. Two versions made apart are both kept; a version made
//   knowing another replaces it. The hub never decides a conflict, and a
//   person deciding one sends a version that has seen both.
//
// Whose account it is comes from the sign-in only (authedUser): every key
// below starts with the account's id, so one account can never name
// another's bucket.
//
// KV writes (the free plan's ~1,000 a day are shared with sign-in, K-952):
// one per device registered, one per key made or passed on, and one per
// bucket PUT that changed something -- a sync's writes go up together.
// A read with nothing new costs no write and no bytes (`since`).

const HEX64 = /^[0-9a-f]{64}$/;
const DEVICE = /^[0-9a-f]{16}$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;
const NAME_MAX = 80;
const ENVELOPE_MAX = 64 * 1024;
const MAX_DEVICES = 32;
// The store's own bounds (crates/adapter-common kv.rs): a value is at most
// 1 MiB and a store 16 MiB. Sealed, a value grows by its base64 twice
// (~1.8x) plus a little; a bucket is one KV value, which may be 25 MiB.
const SEALED_MAX = Math.ceil(1.9 * 1024 * 1024);
const BUCKET_MAX = 24 * 1024 * 1024;
const VERSIONS_MAX = 16;
const VV_MAX = 64;
const WRITES_MAX = 4096;

function json(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

function refuse(status, error, message) {
  return json({ error, message }, status);
}

const goodBlob = (b, max = ENVELOPE_MAX) => typeof b === "string" && b.length > 0 && b.length <= max && b.length % 4 === 0 && B64.test(b);

function goodParams(p) {
  return (
    p && typeof p === "object" && p.alg === "argon2id" && /^[0-9a-f]{32}$/.test(String(p.salt || "")) &&
    Number.isInteger(p.m_kib) && p.m_kib >= 19 * 1024 && Number.isInteger(p.t) && p.t >= 2 && Number.isInteger(p.p) && p.p >= 1
  );
}

/// The id of a device: the first 16 hex of the SHA-256 of its public key's
/// lower-case hex (crates/orgkeys device_id).
async function deviceIdOf(publicKeyHex) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(String(publicKeyHex).toLowerCase()));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("").slice(0, 16);
}

/// A version vector as a device writes it: device ids to positive counts.
function goodVector(vv) {
  if (!vv || typeof vv !== "object" || Array.isArray(vv)) return false;
  const entries = Object.entries(vv);
  if (entries.length === 0 || entries.length > VV_MAX) return false;
  return entries.every(([d, n]) => DEVICE.test(d) && Number.isSafeInteger(n) && n > 0);
}

/// `a` has seen everything `b` has: `b` is the same version or older.
export function covers(a, b) {
  for (const [d, n] of Object.entries(b)) if ((a[d] || 0) < n) return false;
  return true;
}

/// Keep `w` among `versions` unless one already covers it; drop what it
/// covers. Answers whether anything changed.
export function keepVersion(versions, w) {
  if (versions.some((v) => covers(v.vv, w.vv))) return false;
  for (let i = versions.length - 1; i >= 0; i--) if (covers(w.vv, versions[i].vv)) versions.splice(i, 1);
  versions.push(w);
  return true;
}

const keysKey = (uid) => `acsync:${uid}:keys`;
const bucketKey = (uid, bucket) => `acsync:${uid}:b:${bucket}`;

async function readKeys(env, uid) {
  const raw = await env.APPS.get(keysKey(uid));
  const k = raw ? JSON.parse(raw) : {};
  return { key: k.key || null, devices: k.devices || {}, envelopes: k.envelopes || {}, recovery: k.recovery || null };
}

async function writeKeys(env, uid, keys) {
  await env.APPS.put(keysKey(uid), JSON.stringify(keys));
}

async function body(request) {
  try {
    return await request.json();
  } catch {
    return null;
  }
}

function view(uid, keys, device) {
  const mine = device && keys.devices[device] && keys.devices[device].state === "active" ? device : null;
  const env = mine ? keys.envelopes[mine] : null;
  return {
    account: uid,
    key: keys.key ? { version: keys.key.version, public_key: keys.key.public_key, created_at: keys.key.created_at } : null,
    device: mine ? { device_id: mine, name: keys.devices[mine].name, state: "active" } : null,
    envelope: env ? { version: env.version, blob: env.blob, granted_device: env.granted_device } : null,
    devices: Object.entries(keys.devices)
      .filter(([, d]) => d.state === "active")
      .sort(([, a], [, b]) => a.added_at - b.added_at)
      .map(([id, d]) => ({
        device_id: id,
        name: d.name,
        public_key: d.public_key,
        added_at: d.added_at,
        holds: keys.envelopes[id] ? keys.envelopes[id].version : null,
      })),
    recovery: keys.recovery ? { public_key: keys.recovery.public_key, params: keys.recovery.params } : null,
  };
}

async function deviceAdd(request, env, uid, id) {
  const b = await body(request);
  const pub = String((b && b.public_key) || "").toLowerCase();
  if (!DEVICE.test(id) || !HEX64.test(pub) || (await deviceIdOf(pub)) !== id) {
    return refuse(400, "bad-request", "a device is a 64-hex X25519 public key and its id, the first 16 hex of the SHA-256 of that hex");
  }
  const name = String((b && b.name) || "").replace(/[\u0000-\u001f]/g, " ").slice(0, NAME_MAX);
  const keys = await readKeys(env, uid);
  const was = keys.devices[id];
  if (was && was.public_key !== pub) return refuse(409, "device-taken", "that device id is registered with another key");
  if (was && was.state === "active" && was.name === name) return json({ device_id: id, state: "active" });
  const active = Object.values(keys.devices).filter((d) => d.state === "active").length;
  if (!was && active >= MAX_DEVICES) return refuse(413, "too-many-devices", `an account syncs to at most ${MAX_DEVICES} devices -- remove one first`);
  keys.devices[id] = { public_key: pub, name, added_at: was ? was.added_at : Date.now(), state: "active" };
  await writeKeys(env, uid, keys);
  return json({ device_id: id, state: "active" });
}

async function deviceRemove(env, uid, id) {
  const keys = await readKeys(env, uid);
  if (!keys.devices[id]) return refuse(404, "device-unknown", "that device is not one of this account's");
  keys.devices[id].state = "removed";
  delete keys.envelopes[id];
  await writeKeys(env, uid, keys);
  return json({ device_id: id, state: "removed" });
}

async function init(request, env, uid) {
  const a = (await body(request)) || {};
  const keys = await readKeys(env, uid);
  if (keys.key) return refuse(409, "key-exists", "this account already has its data key");
  const id = String(a.device_id || "");
  const dev = keys.devices[id];
  if (!dev || dev.state !== "active") return refuse(404, "device-unknown", "register this device first");
  const rec = a.recovery || {};
  if (a.version !== 1 || !HEX64.test(String(a.public_key || "")) || !goodBlob(a.envelope) || !HEX64.test(String(rec.public_key || "")) || !goodParams(rec.params) || !goodBlob(rec.blob)) {
    return refuse(400, "bad-request", "the key (version 1), its envelope and the recovery envelope are each required");
  }
  const now = Date.now();
  keys.key = { version: 1, public_key: a.public_key, created_at: now, created_device: id };
  keys.envelopes[id] = { version: 1, blob: a.envelope, granted_device: id, at: now };
  keys.recovery = { version: 1, public_key: rec.public_key, params: rec.params, blob: rec.blob };
  await writeKeys(env, uid, keys);
  return json({ version: 1 }, 201);
}

async function grant(request, env, uid) {
  const a = (await body(request)) || {};
  const keys = await readKeys(env, uid);
  if (!keys.key) return refuse(409, "no-key", "this account has no data key yet");
  if (a.version !== keys.key.version) return refuse(409, "key-rotated", `the current key is version ${keys.key.version}`);
  if (!goodBlob(a.blob)) return refuse(400, "bad-request", "the envelope is base64, under 64 KiB");
  const id = String(a.device_id || "");
  const to = keys.devices[id];
  if (!to || to.state !== "active") return refuse(404, "device-unknown", "that device is not one of this account's");
  let how;
  if (a.by_device === "recovery") {
    // The recovery code opened the key on that device itself; the hub
    // cannot check the code, and an envelope it cannot open is all it
    // keeps either way.
    how = "recovery";
  } else {
    const by = String(a.by_device || "");
    const held = keys.devices[by] && keys.devices[by].state === "active" ? keys.envelopes[by] : null;
    if (!held || held.version !== keys.key.version) return refuse(403, "key-not-held", "only a device that holds the key can pass it on");
    how = by;
  }
  keys.envelopes[id] = { version: keys.key.version, blob: a.blob, granted_device: how, at: Date.now() };
  await writeKeys(env, uid, keys);
  return json({ device_id: id, version: keys.key.version });
}

async function recovery(env, uid) {
  const keys = await readKeys(env, uid);
  if (!keys.recovery) return refuse(404, "no-key", "this account has no data key yet");
  const r = keys.recovery;
  return json({ version: r.version, public_key: r.public_key, params: r.params, blob: r.blob });
}

function goodWrite(w) {
  return (
    w && typeof w === "object" && HEX64.test(String(w.name || "")) && goodVector(w.vv) && DEVICE.test(String(w.by || "")) &&
    (w.kind === "v" || w.kind === "t") && Number.isSafeInteger(w.k) && w.k >= 1 && goodBlob(w.c, SEALED_MAX)
  );
}

async function bucketRead(env, uid, bucket) {
  const raw = await env.APPS.get(bucketKey(uid, bucket));
  return raw ? JSON.parse(raw) : { rev: 0, entries: {} };
}

function sinceOf(url) {
  const s = url.searchParams.get("since");
  return s !== null && /^\d{1,15}$/.test(s) ? Number(s) : null;
}

async function bucketGet(url, env, uid, bucket) {
  const b = await bucketRead(env, uid, bucket);
  const since = sinceOf(url);
  if (since !== null && since === b.rev) return json({ rev: b.rev, unchanged: true });
  return json({ rev: b.rev, entries: b.entries });
}

async function bucketPut(request, url, env, uid, bucket) {
  const a = await body(request);
  const writes = a && Array.isArray(a.writes) ? a.writes : null;
  if (!writes || writes.length > WRITES_MAX) return refuse(400, "bad-request", `writes is a list of at most ${WRITES_MAX}`);
  const bad = writes.findIndex((w) => !goodWrite(w));
  if (bad >= 0) return refuse(400, "bad-request", `write ${bad} is not a sealed entry: {name, vv, by, kind, k, c}`);
  const b = await bucketRead(env, uid, bucket);
  let changed = false;
  for (const w of writes) {
    const versions = b.entries[w.name] || [];
    if (keepVersion(versions, { vv: w.vv, by: w.by, kind: w.kind, k: w.k, c: w.c })) changed = true;
    if (versions.length > VERSIONS_MAX) return refuse(413, "too-many-versions", "one entry holds too many versions nobody has decided between");
    b.entries[w.name] = versions;
  }
  if (changed) {
    b.rev += 1;
    const out = JSON.stringify(b);
    if (out.length > BUCKET_MAX) return refuse(413, "bucket-too-large", "this app's data is larger than sync carries");
    await env.APPS.put(bucketKey(uid, bucket), out);
  }
  const since = sinceOf(url);
  if (!changed && since !== null && since === b.rev) return json({ rev: b.rev, unchanged: true });
  return json({ rev: b.rev, entries: b.entries });
}

/// /account/sync/... Answers a Response, or null for any other path.
export async function accountSyncRoute(request, url, env, { authedUser }) {
  const { pathname } = url;
  if (pathname !== "/account/sync" && !pathname.startsWith("/account/sync/")) return null;
  const user = await authedUser(request, env);
  if (!user || !user.id) return refuse(401, "sign-in", "sign in to Krate to keep your apps' data on all your devices");
  const uid = String(user.id);
  const rest = pathname.slice("/account/sync/".length).split("/");
  const m = request.method;
  if (rest.length === 1 && rest[0] === "keys" && m === "GET") {
    const keys = await readKeys(env, uid);
    return json(view(uid, keys, url.searchParams.get("device") || ""));
  }
  if (rest.length === 2 && rest[0] === "devices" && m === "PUT") return deviceAdd(request, env, uid, rest[1].toLowerCase());
  if (rest.length === 2 && rest[0] === "devices" && m === "DELETE") return deviceRemove(env, uid, rest[1].toLowerCase());
  if (rest.length === 1 && rest[0] === "init" && m === "POST") return init(request, env, uid);
  if (rest.length === 1 && rest[0] === "grants" && m === "POST") return grant(request, env, uid);
  if (rest.length === 1 && rest[0] === "recovery" && m === "GET") return recovery(env, uid);
  if (rest.length === 2 && rest[0] === "buckets" && HEX64.test(rest[1])) {
    if (m === "GET") return bucketGet(url, env, uid, rest[1]);
    if (m === "PUT") return bucketPut(request, url, env, uid, rest[1]);
  }
  return refuse(404, "not-found", "no such account sync route");
}
