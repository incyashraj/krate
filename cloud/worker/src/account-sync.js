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
// Where it is kept: one SQLite-backed Durable Object per account
// (SyncAccount, named "acct:<account id>", bound as SYNC_ACCOUNTS), the way
// organisations are kept. Every call is one transaction in it, and a read
// sees the last write from anywhere -- a second device is never shown an
// older bucket than the first one wrote. It used to be KV: a sync that
// changed something was a KV write, an open app syncs every 30 s, and the
// free plan's 1,000 KV writes a day are shared with sign-in (K-912, K-952).
// Account sync now writes nothing to KV. File bodies stay in R2; the
// worker does every R2 call itself, outside the object's transaction.
//
// Whose account it is comes from the sign-in only (authedUser): the object
// is named by the account's id, and the R2 keys start with it, so one
// account can never name another's bucket.
//
// What a call costs the object (rows written, the free plan's 100,000 a
// day): a sync that changed something writes its new versions, deletes
// what they replaced, and updates the bucket's one row; a read, and a
// sync with nothing new, writes nothing. A file's body is one R2 write the
// first time it is sent and none after.

const HEX64 = /^[0-9a-f]{64}$/;
const DEVICE = /^[0-9a-f]{16}$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;
const NAME_MAX = 80;
const ENVELOPE_MAX = 64 * 1024;
const MAX_DEVICES = 32;
// The store's own bounds (crates/adapter-common kv.rs): a value is at most
// 1 MiB and a store 16 MiB. Sealed, a value grows by its base64 twice
// (~1.8x) plus a little. One sealed version is one row of the object's
// SQLite, whose rows are at most 2,000,000 bytes: SEALED_MAX leaves room
// for the rest of the row (ROW_ROOM).
export const SEALED_MAX = Math.ceil(1.9 * 1024 * 1024);
export const ROW_MAX = 2_000_000;
export const ROW_ROOM = 64 + 64 + 64 * 22 + 200;
// One app's bucket, as the device reads it back in one answer.
const BUCKET_MAX = 24 * 1024 * 1024;
const VERSIONS_MAX = 16;
export const VV_MAX = 64;
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

/// A version vector as one string, its devices in order: the same version
/// is always the same text, so it can be part of a row's key.
function vvText(vv) {
  return JSON.stringify(Object.fromEntries(Object.entries(vv).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))));
}

const blobKey = (uid, bucket, id) => `acsync/${uid}/${bucket}/${id}`;

async function body(request) {
  try {
    return await request.json();
  } catch {
    return null;
  }
}

function goodWrite(w) {
  return (
    w && typeof w === "object" && HEX64.test(String(w.name || "")) && goodVector(w.vv) && DEVICE.test(String(w.by || "")) &&
    (w.kind === "v" || w.kind === "t") && Number.isSafeInteger(w.k) && w.k >= 1 && goodBlob(w.c, SEALED_MAX) &&
    // A file's version names its body; a deletion names none.
    (w.blob === undefined || (w.kind === "v" && HEX64.test(String(w.blob))))
  );
}

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((x) => x.toString(16).padStart(2, "0")).join("");
}

// ---------------------------------------------------------------------------
// The account's object.

const SCHEMA = [
  // The account key's public half. One row, once.
  `CREATE TABLE IF NOT EXISTS account_key (
     one INTEGER PRIMARY KEY CHECK (one = 1),
     version INTEGER NOT NULL, public_key TEXT NOT NULL,
     created_at INTEGER NOT NULL, created_device TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS devices (
     device_id TEXT PRIMARY KEY, public_key TEXT NOT NULL, name TEXT NOT NULL,
     added_at INTEGER NOT NULL, state TEXT NOT NULL) WITHOUT ROWID`,
  // The key sealed to each device that was approved.
  `CREATE TABLE IF NOT EXISTS envelopes (
     device_id TEXT PRIMARY KEY, version INTEGER NOT NULL, blob TEXT NOT NULL,
     granted_device TEXT NOT NULL, at INTEGER NOT NULL) WITHOUT ROWID`,
  `CREATE TABLE IF NOT EXISTS recovery (
     one INTEGER PRIMARY KEY CHECK (one = 1),
     version INTEGER NOT NULL, public_key TEXT NOT NULL, params TEXT NOT NULL, blob TEXT NOT NULL)`,
  // One row per app bucket: its revision, the size of its answer, and the
  // bytes of the file bodies it names.
  `CREATE TABLE IF NOT EXISTS buckets (
     bucket TEXT PRIMARY KEY, rev INTEGER NOT NULL, bytes INTEGER NOT NULL, files INTEGER NOT NULL) WITHOUT ROWID`,
  // Every version no other replaces, one row each. `seq` keeps the order
  // they arrived in; `vv` is vvText.
  `CREATE TABLE IF NOT EXISTS versions (
     bucket TEXT NOT NULL, name TEXT NOT NULL, vv TEXT NOT NULL, seq INTEGER NOT NULL,
     by_device TEXT NOT NULL, kind TEXT NOT NULL, k INTEGER NOT NULL, c TEXT NOT NULL,
     blob TEXT, bn INTEGER,
     PRIMARY KEY (bucket, name, vv)) WITHOUT ROWID`,
  // The file bodies a bucket's versions name, with the size R2 measured
  // and how many versions name each. A row with no versions is deleted,
  // and its body in R2 with it.
  `CREATE TABLE IF NOT EXISTS bodies (
     bucket TEXT NOT NULL, id TEXT NOT NULL, size INTEGER NOT NULL, refs INTEGER NOT NULL,
     PRIMARY KEY (bucket, id)) WITHOUT ROWID`,
];

/// What one version adds to its bucket's answer, roughly: its JSON and its
/// share of the entry around it. BUCKET_MAX is held against the sum.
function versionBytes(name, v) {
  return JSON.stringify(v).length + name.length + 6;
}

function wireOf(row) {
  const v = { vv: JSON.parse(row.vv), by: row.by_device, kind: row.kind, k: row.k, c: row.c };
  if (row.blob) Object.assign(v, { blob: row.blob, bn: row.bn || 0 });
  return v;
}

/// One account's sync state. Each command is one synchronous transaction:
/// no await inside, so no other request is interleaved with it.
export class SyncAccount {
  constructor(ctx, env) {
    this.ctx = ctx;
    this.env = env;
    this.sql = ctx.storage.sql;
    for (const statement of SCHEMA) this.sql.exec(statement);
  }

  rows(query, ...bindings) {
    return this.sql.exec(query, ...bindings).toArray();
  }

  row(query, ...bindings) {
    return this.rows(query, ...bindings)[0] || null;
  }

  /// The answer goes back as one line of control -- {status, need, drop} --
  /// then the body's JSON, so a bucket of many megabytes is passed through
  /// the worker as text and never parsed twice.
  async fetch(request) {
    let command;
    try {
      command = await request.json();
    } catch {
      return new Response(`${JSON.stringify({ status: 400 })}\n${JSON.stringify({ error: "bad-request", message: "not a command" })}`);
    }
    const answer = this.ctx.storage.transactionSync(() => this.run(command));
    const { body: out, ...head } = answer;
    return new Response(`${JSON.stringify(head)}\n${out === undefined ? "" : JSON.stringify(out)}`, {
      headers: { "content-type": "text/plain" },
    });
  }

  run(command) {
    const a = command.args || {};
    switch (command.op) {
      case "keys":
        return { status: 200, body: this.view(command.account, a.device || "") };
      case "device-add":
        return this.deviceAdd(a);
      case "device-remove":
        return this.deviceRemove(a.id);
      case "init":
        return this.init(a);
      case "grant":
        return this.grant(a);
      case "recovery":
        return this.recovery();
      case "bucket-get":
        return this.bucketGet(a.bucket, a.since);
      case "bucket-put":
        return this.bucketPut(a);
      default:
        return { status: 400, body: { error: "bad-request", message: "no such command" } };
    }
  }

  device(id) {
    return this.row("SELECT device_id, public_key, name, added_at, state FROM devices WHERE device_id = ?", id);
  }

  envelope(id) {
    return this.row("SELECT version, blob, granted_device FROM envelopes WHERE device_id = ?", id);
  }

  key() {
    return this.row("SELECT version, public_key, created_at FROM account_key WHERE one = 1");
  }

  view(account, device) {
    const key = this.key();
    const mineRow = device ? this.device(device) : null;
    const mine = mineRow && mineRow.state === "active" ? mineRow : null;
    const env = mine ? this.envelope(mine.device_id) : null;
    const recovery = this.row("SELECT public_key, params FROM recovery WHERE one = 1");
    const devices = this.rows(
      `SELECT d.device_id, d.name, d.public_key, d.added_at, e.version AS holds
         FROM devices d LEFT JOIN envelopes e ON e.device_id = d.device_id
        WHERE d.state = 'active' ORDER BY d.added_at, d.device_id`,
    );
    return {
      account,
      key: key ? { version: key.version, public_key: key.public_key, created_at: key.created_at } : null,
      device: mine ? { device_id: mine.device_id, name: mine.name, state: "active" } : null,
      envelope: env ? { version: env.version, blob: env.blob, granted_device: env.granted_device } : null,
      devices: devices.map((d) => ({
        device_id: d.device_id,
        name: d.name,
        public_key: d.public_key,
        added_at: d.added_at,
        holds: d.holds === null || d.holds === undefined ? null : d.holds,
      })),
      recovery: recovery ? { public_key: recovery.public_key, params: JSON.parse(recovery.params) } : null,
    };
  }

  deviceAdd({ id, public_key: pub, name }) {
    const was = this.device(id);
    if (was && was.public_key !== pub) return { status: 409, body: { error: "device-taken", message: "that device id is registered with another key" } };
    if (was && was.state === "active" && was.name === name) return { status: 200, body: { device_id: id, state: "active" } };
    const active = this.row("SELECT COUNT(*) AS n FROM devices WHERE state = 'active'").n;
    if (!was && active >= MAX_DEVICES) {
      return { status: 413, body: { error: "too-many-devices", message: `an account syncs to at most ${MAX_DEVICES} devices -- remove one first` } };
    }
    this.sql.exec(
      `INSERT INTO devices (device_id, public_key, name, added_at, state) VALUES (?, ?, ?, ?, 'active')
       ON CONFLICT (device_id) DO UPDATE SET name = excluded.name, state = 'active'`,
      id,
      pub,
      name,
      Date.now(),
    );
    return { status: 200, body: { device_id: id, state: "active" } };
  }

  deviceRemove(id) {
    if (!this.device(id)) return { status: 404, body: { error: "device-unknown", message: "that device is not one of this account's" } };
    this.sql.exec("UPDATE devices SET state = 'removed' WHERE device_id = ?", id);
    this.sql.exec("DELETE FROM envelopes WHERE device_id = ?", id);
    return { status: 200, body: { device_id: id, state: "removed" } };
  }

  init(a) {
    if (this.key()) return { status: 409, body: { error: "key-exists", message: "this account already has its data key" } };
    const id = String(a.device_id || "");
    const dev = this.device(id);
    if (!dev || dev.state !== "active") return { status: 404, body: { error: "device-unknown", message: "register this device first" } };
    const rec = a.recovery || {};
    if (a.version !== 1 || !HEX64.test(String(a.public_key || "")) || !goodBlob(a.envelope) || !HEX64.test(String(rec.public_key || "")) || !goodParams(rec.params) || !goodBlob(rec.blob)) {
      return { status: 400, body: { error: "bad-request", message: "the key (version 1), its envelope and the recovery envelope are each required" } };
    }
    const now = Date.now();
    this.sql.exec("INSERT INTO account_key (one, version, public_key, created_at, created_device) VALUES (1, 1, ?, ?, ?)", a.public_key, now, id);
    this.sql.exec(
      `INSERT INTO envelopes (device_id, version, blob, granted_device, at) VALUES (?, 1, ?, ?, ?)
       ON CONFLICT (device_id) DO UPDATE SET version = 1, blob = excluded.blob, granted_device = excluded.granted_device, at = excluded.at`,
      id,
      a.envelope,
      id,
      now,
    );
    this.sql.exec(
      `INSERT INTO recovery (one, version, public_key, params, blob) VALUES (1, 1, ?, ?, ?)
       ON CONFLICT (one) DO UPDATE SET version = 1, public_key = excluded.public_key, params = excluded.params, blob = excluded.blob`,
      rec.public_key,
      JSON.stringify(rec.params),
      rec.blob,
    );
    return { status: 201, body: { version: 1 } };
  }

  grant(a) {
    const key = this.key();
    if (!key) return { status: 409, body: { error: "no-key", message: "this account has no data key yet" } };
    if (a.version !== key.version) return { status: 409, body: { error: "key-rotated", message: `the current key is version ${key.version}` } };
    if (!goodBlob(a.blob)) return { status: 400, body: { error: "bad-request", message: "the envelope is base64, under 64 KiB" } };
    const id = String(a.device_id || "");
    const to = this.device(id);
    if (!to || to.state !== "active") return { status: 404, body: { error: "device-unknown", message: "that device is not one of this account's" } };
    let how;
    if (a.by_device === "recovery") {
      // The recovery code opened the key on that device itself; the hub
      // cannot check the code, and an envelope it cannot open is all it
      // keeps either way.
      how = "recovery";
    } else {
      const by = String(a.by_device || "");
      const byDevice = this.device(by);
      const held = byDevice && byDevice.state === "active" ? this.envelope(by) : null;
      if (!held || held.version !== key.version) return { status: 403, body: { error: "key-not-held", message: "only a device that holds the key can pass it on" } };
      how = by;
    }
    this.sql.exec(
      `INSERT INTO envelopes (device_id, version, blob, granted_device, at) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT (device_id) DO UPDATE SET version = excluded.version, blob = excluded.blob, granted_device = excluded.granted_device, at = excluded.at`,
      id,
      key.version,
      a.blob,
      how,
      Date.now(),
    );
    return { status: 200, body: { device_id: id, version: key.version } };
  }

  recovery() {
    const r = this.row("SELECT version, public_key, params, blob FROM recovery WHERE one = 1");
    if (!r) return { status: 404, body: { error: "no-key", message: "this account has no data key yet" } };
    return { status: 200, body: { version: r.version, public_key: r.public_key, params: JSON.parse(r.params), blob: r.blob } };
  }

  bucketRow(bucket) {
    return this.row("SELECT rev, bytes, files FROM buckets WHERE bucket = ?", bucket) || { rev: 0, bytes: 0, files: 0, fresh: true };
  }

  entries(bucket) {
    const out = {};
    for (const row of this.rows("SELECT name, vv, by_device, kind, k, c, blob, bn FROM versions WHERE bucket = ? ORDER BY seq, name", bucket)) {
      (out[row.name] ||= []).push(wireOf(row));
    }
    return out;
  }

  bucketGet(bucket, since) {
    const b = this.bucketRow(bucket);
    if (since !== null && since === b.rev) return { status: 200, body: { rev: b.rev, unchanged: true } };
    return { status: 200, body: { rev: b.rev, entries: this.entries(bucket) } };
  }

  /// One app's changes. `sizes` are the bodies the worker found in R2 for
  /// a call before: a version naming a body the bucket does not hold yet
  /// answers `need` -- nothing written -- and the worker looks in R2 and
  /// asks again with their sizes.
  bucketPut({ bucket, writes, since, sizes = {} }) {
    const b = this.bucketRow(bucket);
    const held = new Map();
    const body = (id) => {
      if (!held.has(id)) {
        const r = this.row("SELECT size, refs FROM bodies WHERE bucket = ? AND id = ?", bucket, id);
        held.set(id, r ? { size: r.size, refs: r.refs } : null);
      }
      return held.get(id);
    };
    // The versions now held for each entry these writes name.
    const current = new Map();
    for (const w of writes) {
      if (current.has(w.name)) continue;
      const rows = this.rows("SELECT name, vv, seq, by_device, kind, k, c, blob, bn FROM versions WHERE bucket = ? AND name = ? ORDER BY seq", bucket, w.name);
      current.set(
        w.name,
        rows.map((row) => ({ ...wireOf(row), held: row.vv })),
      );
    }
    // A file's body must be on the hub before a version may name it: a
    // device that lost it on the way sends it again.
    const need = [];
    for (const w of writes) {
      if (!w.blob || body(w.blob) || Object.hasOwn(sizes, w.blob) || need.includes(w.blob)) continue;
      if (current.get(w.name).some((v) => covers(v.vv, w.vv))) continue;
      need.push(w.blob);
    }
    if (need.length) return { status: 409, need };

    let changed = false;
    const lists = new Map();
    for (const [name, versions] of current) lists.set(name, [...versions]);
    for (const w of writes) {
      const versions = lists.get(w.name);
      const v = { vv: w.vv, by: w.by, kind: w.kind, k: w.k, c: w.c };
      if (w.blob) Object.assign(v, { blob: w.blob, bn: body(w.blob) ? body(w.blob).size : Number(sizes[w.blob]) || 0 });
      if (keepVersion(versions, v)) changed = true;
      if (versions.length > VERSIONS_MAX) {
        return { status: 413, body: { error: "too-many-versions", message: "one entry holds too many versions nobody has decided between" } };
      }
    }
    if (!changed) {
      if (since !== null && since === b.rev) return { status: 200, body: { rev: b.rev, unchanged: true } };
      return { status: 200, body: { rev: b.rev, entries: this.entries(bucket) } };
    }

    // What goes and what comes, per entry.
    const gone = [];
    const come = [];
    for (const [name, before] of current) {
      const after = lists.get(name);
      for (const v of before) if (!after.includes(v)) gone.push([name, v]);
      for (const v of after) if (!before.includes(v)) come.push([name, v]);
    }
    let bytes = b.bytes;
    let files = b.files;
    const refs = new Map();
    const bump = (id, by) => refs.set(id, (refs.get(id) || 0) + by);
    for (const [name, v] of gone) {
      bytes -= versionBytes(name, { ...v, held: undefined });
      if (v.blob) bump(v.blob, -1);
    }
    for (const [name, v] of come) {
      bytes += versionBytes(name, v);
      if (v.blob) bump(v.blob, 1);
    }
    const drop = [];
    const bodyRows = [];
    for (const [id, by] of refs) {
      if (by === 0) continue;
      const was = body(id);
      const n = (was ? was.refs : 0) + by;
      const size = was ? was.size : Number(sizes[id]) || 0;
      if (!was && n > 0) files += size;
      if (was && n <= 0) {
        files -= size;
        drop.push(id);
      }
      bodyRows.push([id, size, n]);
    }
    if (files > FILES_MAX) {
      return { status: 413, body: { error: "files-too-large", message: `this app's files are larger than sync carries (${Math.round(FILES_MAX / 1048576)} MB)` } };
    }
    if (bytes > BUCKET_MAX) return { status: 413, body: { error: "bucket-too-large", message: "this app's data is larger than sync carries" } };

    const rev = b.rev + 1;
    for (const [name, v] of gone) this.sql.exec("DELETE FROM versions WHERE bucket = ? AND name = ? AND vv = ?", bucket, name, v.held);
    let seq = rev * (WRITES_MAX + 1);
    for (const [name, v] of come) {
      this.sql.exec(
        "INSERT INTO versions (bucket, name, vv, seq, by_device, kind, k, c, blob, bn) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        bucket,
        name,
        vvText(v.vv),
        seq++,
        v.by,
        v.kind,
        v.k,
        v.c,
        v.blob || null,
        v.blob ? v.bn : null,
      );
    }
    for (const [id, size, n] of bodyRows) {
      if (n <= 0) this.sql.exec("DELETE FROM bodies WHERE bucket = ? AND id = ?", bucket, id);
      else {
        this.sql.exec(
          `INSERT INTO bodies (bucket, id, size, refs) VALUES (?, ?, ?, ?)
           ON CONFLICT (bucket, id) DO UPDATE SET refs = excluded.refs`,
          bucket,
          id,
          size,
          n,
        );
      }
    }
    this.sql.exec(
      `INSERT INTO buckets (bucket, rev, bytes, files) VALUES (?, ?, ?, ?)
       ON CONFLICT (bucket) DO UPDATE SET rev = excluded.rev, bytes = excluded.bytes, files = excluded.files`,
      bucket,
      rev,
      bytes,
      files,
    );
    return { status: 200, drop, body: { rev, entries: this.entries(bucket) } };
  }
}

// ---------------------------------------------------------------------------
// The worker's side: the sign-in, the shapes, and R2.

/// Ask the account's object one command. Answers `{status, need, drop}`
/// and the body as JSON text, passed on as it came.
async function ask(env, uid, op, args = {}) {
  const ns = env.SYNC_ACCOUNTS;
  const stub = ns.get(ns.idFromName(`acct:${uid}`));
  const res = await stub.fetch("https://sync-account/command", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ op, account: uid, args }),
  });
  const text = await res.text();
  const cut = text.indexOf("\n");
  const head = JSON.parse(cut < 0 ? text : text.slice(0, cut));
  return { ...head, text: cut < 0 ? "" : text.slice(cut + 1) };
}

function answer(a) {
  return new Response(a.text, {
    status: a.status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

async function deviceAdd(request, env, uid, id) {
  const b = await body(request);
  const pub = String((b && b.public_key) || "").toLowerCase();
  if (!DEVICE.test(id) || !HEX64.test(pub) || (await deviceIdOf(pub)) !== id) {
    return refuse(400, "bad-request", "a device is a 64-hex X25519 public key and its id, the first 16 hex of the SHA-256 of that hex");
  }
  const name = String((b && b.name) || "").replace(/[\u0000-\u001f]/g, " ").slice(0, NAME_MAX);
  return answer(await ask(env, uid, "device-add", { id, public_key: pub, name }));
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

function sinceOf(url) {
  const s = url.searchParams.get("since");
  return s !== null && /^\d{1,15}$/.test(s) ? Number(s) : null;
}

async function bucketPut(request, url, env, uid, bucket) {
  const a = await body(request);
  const writes = a && Array.isArray(a.writes) ? a.writes : null;
  if (!writes || writes.length > WRITES_MAX) return refuse(400, "bad-request", `writes is a list of at most ${WRITES_MAX}`);
  const bad = writes.findIndex((w) => !goodWrite(w));
  if (bad >= 0) return refuse(400, "bad-request", `write ${bad} is not a sealed entry: {name, vv, by, kind, k, c}`);
  const clean = writes.map((w) => {
    const out = { name: w.name, vv: w.vv, by: w.by, kind: w.kind, k: w.k, c: w.c };
    if (w.blob !== undefined) out.blob = w.blob;
    return out;
  });
  const since = sinceOf(url);
  const sizes = {};
  // A body named for the first time is looked for in R2, outside the
  // object's transaction, and the object asked again with what was found.
  // Another device's sync landing between the two only changes what is
  // needed, so the object says so again; three rounds is past any race.
  for (let round = 0; round < 3; round++) {
    const got = await ask(env, uid, "bucket-put", { bucket, writes: clean, since, sizes });
    if (got.need) {
      if (!env.BUNDLES) return refuse(503, "no-store", "this hub keeps no files");
      const missing = [];
      for (const id of got.need) {
        const head = await env.BUNDLES.head(blobKey(uid, bucket, id));
        if (head) sizes[id] = head.size;
        else missing.push(id);
      }
      if (missing.length) return json({ error: "blob-missing", message: "a file's body is not on the hub yet -- send it first", missing }, 409);
      continue;
    }
    // Only once the bucket no longer names them: a body no version names.
    // A delete that fails leaves a body nothing names, which costs storage
    // and never data; the answer still goes back.
    for (const id of got.drop || []) {
      try {
        await env.BUNDLES.delete(blobKey(uid, bucket, id));
      } catch {
        /* left for a later scrub */
      }
    }
    return answer(got);
  }
  return refuse(503, "busy", "this app's data changed on another device meanwhile -- sync again");
}

/// /account/sync/... Answers a Response, or null for any other path.
export async function accountSyncRoute(request, url, env, { authedUser }) {
  const { pathname } = url;
  if (pathname !== "/account/sync" && !pathname.startsWith("/account/sync/")) return null;
  const user = await authedUser(request, env);
  if (!user || !user.id) return refuse(401, "sign-in", "sign in to Krate to keep your apps' data on all your devices");
  if (!env.SYNC_ACCOUNTS) return refuse(503, "sync-off", "keeping apps' data on every device is not switched on on this hub");
  const uid = String(user.id);
  const rest = pathname.slice("/account/sync/".length).split("/");
  const m = request.method;
  if (rest.length === 1 && rest[0] === "keys" && m === "GET") return answer(await ask(env, uid, "keys", { device: url.searchParams.get("device") || "" }));
  if (rest.length === 2 && rest[0] === "devices" && m === "PUT") return deviceAdd(request, env, uid, rest[1].toLowerCase());
  if (rest.length === 2 && rest[0] === "devices" && m === "DELETE") return answer(await ask(env, uid, "device-remove", { id: rest[1].toLowerCase() }));
  if (rest.length === 1 && rest[0] === "init" && m === "POST") return answer(await ask(env, uid, "init", (await body(request)) || {}));
  if (rest.length === 1 && rest[0] === "grants" && m === "POST") return answer(await ask(env, uid, "grant", (await body(request)) || {}));
  if (rest.length === 1 && rest[0] === "recovery" && m === "GET") return answer(await ask(env, uid, "recovery"));
  if (rest.length === 3 && rest[0] === "blobs" && HEX64.test(rest[1]) && HEX64.test(rest[2])) {
    if (!env.BUNDLES) return refuse(503, "no-store", "this hub keeps no files");
    if (m === "PUT") return blobPut(request, env, uid, rest[1], rest[2]);
    if (m === "GET") return blobGet(env, uid, rest[1], rest[2]);
  }
  if (rest.length === 2 && rest[0] === "buckets" && HEX64.test(rest[1])) {
    if (m === "GET") return answer(await ask(env, uid, "bucket-get", { bucket: rest[1], since: sinceOf(url) }));
    if (m === "PUT") return bucketPut(request, url, env, uid, rest[1]);
  }
  return refuse(404, "not-found", "no such account sync route");
}
