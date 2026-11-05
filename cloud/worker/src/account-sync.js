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
//   PUT    /account/sync/blobs/<bucket>/<id>    one sealed file body (S3), named by its SHA-256
//   GET    /account/sync/blobs/<bucket>/<id>    that body back
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
// - per app, its files (S3): a second bucket of the same shape whose
//   entries each name a body -- the file's contents, sealed on the device
//   -- kept in R2 (BUNDLES) at acsync/<account>/<bucket>/<sha256 of the
//   sealed bytes>. A version names its body in the clear (`blob`) so the
//   hub can refuse an entry whose body never arrived and delete a body no
//   version names any more; the name is a digest of ciphertext and says
//   nothing about the file. What the hub learns is how many files and
//   roughly how large.
//
// Whose account it is comes from the sign-in only (authedUser): every key
// below starts with the account's id, so one account can never name
// another's bucket.
//
// KV writes (the free plan's ~1,000 a day are shared with sign-in, K-952):
// one per device registered, one per key made or passed on, and one per
// bucket PUT that changed something -- a sync's writes go up together.
// A read with nothing new costs no write and no bytes (`since`). A file's
// body is one R2 write the first time it is sent and none after; R2's
// writes are not KV's, and its free tier is a million a month.

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
// A file follows the person up to 16 MiB (crates/accountsync files.rs
// FILE_MAX); sealed, a body is that plus its nonce and tag. One app's
// bodies together are held to FILES_MAX, every version kept counted.
const BLOB_MAX = 16 * 1024 * 1024 + 64;
const FILES_MAX = 256 * 1024 * 1024;

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
const blobKey = (uid, bucket, id) => `acsync/${uid}/${bucket}/${id}`;

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
    (w.kind === "v" || w.kind === "t") && Number.isSafeInteger(w.k) && w.k >= 1 && goodBlob(w.c, SEALED_MAX) &&
    // A file's version names its body; a deletion names none.
    (w.blob === undefined || (w.kind === "v" && HEX64.test(String(w.blob))))
  );
}

/// Every body the bucket's versions name, with the size the hub measured.
function blobsOf(b) {
  const out = new Map();
  for (const versions of Object.values(b.entries)) for (const v of versions) if (v.blob) out.set(v.blob, v.bn || 0);
  return out;
}

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((x) => x.toString(16).padStart(2, "0")).join("");
}

async function blobPut(request, env, uid, bucket, id) {
  const declared = Number(request.headers.get("content-length") || "0");
  if (declared > BLOB_MAX) return refuse(413, "file-too-large", `a file body is at most ${BLOB_MAX} bytes`);
  const bytes = new Uint8Array(await request.arrayBuffer());
  if (bytes.byteLength === 0 || bytes.byteLength > BLOB_MAX) return refuse(413, "file-too-large", `a file body is at most ${BLOB_MAX} bytes`);
  if ((await sha256Hex(bytes)) !== id) return refuse(400, "bad-request", "a body goes by the SHA-256 of its bytes");
  const key = blobKey(uid, bucket, id);
  const had = await env.BUNDLES.head(key);
  if (had && had.size === bytes.byteLength) return json({ id, size: had.size, stored: false });
  await env.BUNDLES.put(key, bytes, { sha256: id, httpMetadata: { contentType: "application/octet-stream" } });
  return json({ id, size: bytes.byteLength, stored: true }, 201);
}

async function blobGet(env, uid, bucket, id) {
  const object = await env.BUNDLES.get(blobKey(uid, bucket, id));
  if (!object) return refuse(404, "blob-missing", "that file body is not on the hub");
  return new Response(object.body, {
    headers: { "content-type": "application/octet-stream", "content-length": String(object.size), "cache-control": "no-store" },
  });
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
  const before = blobsOf(b);
  // A file's body must be on the hub before a version may name it: a
  // device that lost it on the way sends it again.
  const missing = [];
  const sizes = new Map(before);
  for (const w of writes) {
    if (!w.blob || sizes.has(w.blob)) continue;
    if ((b.entries[w.name] || []).some((v) => covers(v.vv, w.vv))) continue;
    const head = await env.BUNDLES.head(blobKey(uid, bucket, w.blob));
    if (head) sizes.set(w.blob, head.size);
    else missing.push(w.blob);
  }
  if (missing.length) return json({ error: "blob-missing", message: "a file's body is not on the hub yet -- send it first", missing }, 409);
  let changed = false;
  for (const w of writes) {
    const versions = b.entries[w.name] || [];
    const v = { vv: w.vv, by: w.by, kind: w.kind, k: w.k, c: w.c };
    if (w.blob) Object.assign(v, { blob: w.blob, bn: sizes.get(w.blob) || 0 });
    if (keepVersion(versions, v)) changed = true;
    if (versions.length > VERSIONS_MAX) return refuse(413, "too-many-versions", "one entry holds too many versions nobody has decided between");
    b.entries[w.name] = versions;
  }
  const after = blobsOf(b);
  if (changed) {
    let held = 0;
    for (const n of after.values()) held += n;
    if (held > FILES_MAX) return refuse(413, "files-too-large", `this app's files are larger than sync carries (${Math.round(FILES_MAX / 1048576)} MB)`);
    b.rev += 1;
    const out = JSON.stringify(b);
    if (out.length > BUCKET_MAX) return refuse(413, "bucket-too-large", "this app's data is larger than sync carries");
    await env.APPS.put(bucketKey(uid, bucket), out);
    // Only once the bucket no longer names them: a body no version names.
    // A delete that fails leaves a body nothing names, which costs storage
    // and never data; the answer still goes back.
    for (const id of before.keys()) {
      if (after.has(id)) continue;
      try {
        await env.BUNDLES.delete(blobKey(uid, bucket, id));
      } catch {
        /* left for a later scrub */
      }
    }
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
  if (rest.length === 3 && rest[0] === "blobs" && HEX64.test(rest[1]) && HEX64.test(rest[2])) {
    if (!env.BUNDLES) return refuse(503, "no-store", "this hub keeps no files");
    if (m === "PUT") return blobPut(request, env, uid, rest[1], rest[2]);
    if (m === "GET") return blobGet(env, uid, rest[1], rest[2]);
  }
  if (rest.length === 2 && rest[0] === "buckets" && HEX64.test(rest[1])) {
    if (m === "GET") return bucketGet(url, env, uid, rest[1]);
    if (m === "PUT") return bucketPut(request, url, env, uid, rest[1]);
  }
  return refuse(404, "not-found", "no such account sync route");
}
