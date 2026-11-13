// An organisation's keyring (founder decision F-316): the keys that make
// its apps' data and its connections' credentials ciphertext on this hub.
//
// The hub never holds a private key. It keeps, in the organisation's own
// store:
//
// - each member device's X25519 public key, under the member's principal;
// - each version of the organisation key's PUBLIC half, and whether the
//   current one is stale (a member was removed since it was made);
// - one envelope per device: the whole keyring, sealed on a member device
//   to that device's public key with HPKE (RFC 9180). Opaque here;
// - the recovery envelope: the keyring sealed to a key pair derived from
//   the owner's recovery code with Argon2id, beside the (public) Argon2id
//   parameters and that key pair's public half, so a device that rotates
//   the key can re-seal for recovery without the code.
//
// What the hub decides is membership: who may register a device, whose
// device a holder should seal the keyring for, and who may read an
// envelope. Every grant, rotation and recovery is an event in the
// hash-chained audit, and the device list says which devices hold the key,
// so a grant the hub should not have asked for is visible to the
// organisation. (The honest limit: a hub lying about membership could ask
// a device to seal to a device it controls. Key transparency that devices
// verify is the next step.)
//
// This module is independent of the apps' data (org-data.js) and of
// connections: both use `currentKeyVersion()` and the keyring's public key.

import { Org, extendOrg, addReasons, allows, displayOf, sha256HexSync } from "./org.js";
import "./org-lifecycle-store.js";

const HEX64 = /^[0-9a-f]{64}$/;
const DEVICE = /^[0-9a-f]{16}$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;
const ENVELOPE_MAX = 64 * 1024;
const NAME_MAX = 80;

addReasons({
  "no-org-key": {
    status: 409,
    message: (o, d) => d || `${o} has no organisation key yet -- an owner runs \`krate org keys init ${o}\``,
  },
  "key-rotated": {
    status: 409,
    message: (o, d) => d || `${o}'s key was replaced -- this device fetches the new one and tries again`,
  },
  "key-not-held": {
    status: 403,
    message: (o, d) => d || `this device does not hold ${o}'s current key`,
  },
  "device-unknown": {
    status: 404,
    message: (o, d) => d || `that device is not registered with ${o}`,
  },
  "key-exists": { status: 409, message: (o) => `${o} already has an organisation key` },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS key_devices (
     device_id TEXT PRIMARY KEY, principal TEXT NOT NULL, public_key TEXT NOT NULL,
     name TEXT, added_at INTEGER NOT NULL, state TEXT NOT NULL, removed_at INTEGER)`,
  `CREATE TABLE IF NOT EXISTS key_versions (
     version INTEGER PRIMARY KEY, public_key TEXT NOT NULL, state TEXT NOT NULL,
     created_at INTEGER NOT NULL, created_by TEXT NOT NULL, created_device TEXT NOT NULL,
     stale_since INTEGER)`,
  `CREATE TABLE IF NOT EXISTS key_envelopes (
     device_id TEXT PRIMARY KEY, version INTEGER NOT NULL, blob TEXT NOT NULL,
     granted_by TEXT NOT NULL, granted_device TEXT NOT NULL, at INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS key_recovery (
     id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL, public_key TEXT NOT NULL,
     params TEXT NOT NULL, blob TEXT NOT NULL, set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
];

/// The id the hub knows a device's public key by: the first 16 hex of the
/// SHA-256 of its lower-case hex (crates/orgkeys device_id, the same).
export function deviceIdOf(publicKeyHex) {
  return sha256HexSync(String(publicKeyHex).toLowerCase()).slice(0, 16);
}

const goodBlob = (b) => typeof b === "string" && b.length > 0 && b.length <= ENVELOPE_MAX && b.length % 4 === 0 && B64.test(b);

function goodParams(p) {
  return (
    p && typeof p === "object" && p.alg === "argon2id" && /^[0-9a-f]{32}$/.test(String(p.salt || "")) &&
    Number.isInteger(p.m_kib) && p.m_kib >= 19 * 1024 && Number.isInteger(p.t) && p.t >= 2 && Number.isInteger(p.p) && p.p >= 1
  );
}

const ops = {
  /// The current version of the organisation key, or null.
  currentKey() {
    return this.row("SELECT * FROM key_versions ORDER BY version DESC LIMIT 1");
  },

  currentKeyVersion() {
    const k = this.currentKey();
    return k ? k.version : null;
  },

  deviceRow(id) {
    return this.row("SELECT * FROM key_devices WHERE device_id = ?", id);
  },

  /// The caller's own active device `id`, or null.
  myDevice(ctx, me, id) {
    const d = this.deviceRow(String(id || ""));
    return d && d.principal === me.principal && d.state === "active" ? d : null;
  },

  envelopeOf(id) {
    return this.row("SELECT * FROM key_envelopes WHERE device_id = ?", id);
  },

  /// Active devices of active members.
  liveDevices() {
    return this.rows(
      `SELECT d.* FROM key_devices d JOIN members m ON m.principal = d.principal
       WHERE d.state = 'active' AND m.state = 'active' ORDER BY d.added_at, d.device_id`,
    );
  },

  keyStanding(ctx, action, target) {
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return { refusal };
    if (allows(me.role, "keys.hold") !== "yes") return { refusal: this.deny(ctx, action, target, "role-cannot") };
    return { me };
  },

  /// GET /orgs/<slug>/keys?device=<id>: everything one device needs to
  /// hold the key, and to pass it on.
  op_keys_view(ctx) {
    const { me, refusal } = this.keyStanding(ctx, "keys.view", `org:${ctx.slug}`);
    if (refusal) return refusal;
    const cur = this.currentKey();
    const mine = this.myDevice(ctx, me, ctx.args.device);
    const env = mine ? this.envelopeOf(mine.device_id) : null;
    const holds = Boolean(cur && env && env.version === cur.version);
    const all = allows(me.role, "org.view") === "yes";
    const out = {
      current: cur ? { version: cur.version, public_key: cur.public_key, state: cur.state, created_at: cur.created_at } : null,
      device: mine ? { device_id: mine.device_id, state: mine.state } : null,
      envelope: env ? { version: env.version, blob: env.blob, granted_by: displayOf(env.granted_by), granted_device: env.granted_device } : null,
      requests: [],
      seal_to: [],
      recovery: null,
      devices: this.rows("SELECT d.*, e.version AS holds FROM key_devices d LEFT JOIN key_envelopes e ON e.device_id = d.device_id ORDER BY d.added_at")
        .filter((d) => all || d.principal === me.principal)
        .map((d) => ({ device_id: d.device_id, principal: d.principal, display: displayOf(d.principal), name: d.name, kind: d.kind === "browser" ? "browser" : "computer", added_at: d.added_at, state: d.state, holds: d.holds ?? null })),
    };
    if (holds) {
      // Who is waiting for the key: devices of active members without the
      // current version. Only a device that can pass it on is told.
      out.requests = this.liveDevices()
        .filter((d) => {
          const e = this.envelopeOf(d.device_id);
          return !e || e.version !== cur.version;
        })
        .map((d) => ({ device_id: d.device_id, principal: d.principal, public_key: d.public_key, name: d.name }));
      if (cur.state === "stale") {
        out.seal_to = this.liveDevices().map((d) => ({ device_id: d.device_id, principal: d.principal, public_key: d.public_key }));
      }
      const r = this.row("SELECT version, public_key, params FROM key_recovery WHERE id = 1");
      if (r) out.recovery = { version: r.version, public_key: r.public_key, params: JSON.parse(r.params) };
    }
    return this.ok(out);
  },

  /// PUT /orgs/<slug>/keys/devices/<id> {public_key, name}: this device,
  /// for the signed-in member. The id is the public key's (checked at the
  /// door).
  op_keys_device_add(ctx) {
    const id = String(ctx.args.device_id || "");
    const target = `device:${id}`;
    const { me, refusal } = this.keyStanding(ctx, "keys.device", target);
    if (refusal) return refusal;
    const pub = String(ctx.args.public_key || "");
    if (!DEVICE.test(id) || !HEX64.test(pub) || deviceIdOf(pub) !== id) {
      return { status: 400, body: { error: "bad-request", message: "a device is a 64-hex X25519 public key and its id, the first 16 hex of the SHA-256 of that hex" } };
    }
    const name = String(ctx.args.name || "").slice(0, NAME_MAX);
    // A browser that is an owner's device (IC-958) says so; anything else
    // is a computer running Krate.
    const kind = ctx.args.kind === "browser" ? "browser" : "computer";
    const was = this.deviceRow(id);
    if (was && (was.principal !== me.principal || was.public_key !== pub)) {
      return this.deny(ctx, "keys.device", target, "role-cannot", "that device is registered to someone else");
    }
    if (!was) {
      this.sql.exec(
        "INSERT INTO key_devices (device_id, principal, public_key, name, added_at, state, kind) VALUES (?, ?, ?, ?, ?, 'active', ?)",
        id, me.principal, pub, name, ctx.now, kind,
      );
      this.audit(me.principal, "key.device.add", target, "allowed", name || null);
    } else if (was.state !== "active") {
      this.sql.exec("UPDATE key_devices SET state = 'active', removed_at = NULL, name = ? WHERE device_id = ?", name, id);
      this.audit(me.principal, "key.device.add", target, "allowed", "again");
    }
    return this.ok({ device_id: id, state: "active" });
  },

  /// POST /orgs/<slug>/keys/init: the first version, made on the owner's
  /// device, with its envelope and the recovery envelope.
  op_keys_init(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.keyStanding(ctx, "keys.init", target);
    if (refusal) return refusal;
    if (allows(me.role, "keys.init") !== "yes") return this.deny(ctx, "keys.init", target, "role-cannot", `only an owner makes ${ctx.slug}'s key`);
    if (this.currentKey()) return this.deny(ctx, "keys.init", target, "key-exists");
    const a = ctx.args;
    const dev = this.myDevice(ctx, me, a.device_id);
    if (!dev) return this.deny(ctx, "keys.init", target, "device-unknown", "register this device first");
    const rec = a.recovery || {};
    if (!HEX64.test(String(a.public_key || "")) || !goodBlob(a.envelope) || !HEX64.test(String(rec.public_key || "")) || !goodParams(rec.params) || !goodBlob(rec.blob)) {
      return { status: 400, body: { error: "bad-request", message: "the key, its envelope and the recovery envelope are each required" } };
    }
    this.sql.exec(
      "INSERT INTO key_versions (version, public_key, state, created_at, created_by, created_device) VALUES (1, ?, 'current', ?, ?, ?)",
      a.public_key, ctx.now, me.principal, dev.device_id,
    );
    this.sql.exec(
      "INSERT INTO key_envelopes (device_id, version, blob, granted_by, granted_device, at) VALUES (?, 1, ?, ?, ?, ?)",
      dev.device_id, a.envelope, me.principal, dev.device_id, ctx.now,
    );
    this.sql.exec(
      "INSERT INTO key_recovery (id, version, public_key, params, blob, set_at, set_by) VALUES (1, 1, ?, ?, ?, ?, ?)",
      rec.public_key, JSON.stringify(rec.params), rec.blob, ctx.now, me.principal,
    );
    this.audit(me.principal, "key.init", `key:v1`, "allowed", `on ${dev.device_id}`);
    return this.ok({ version: 1 }, 201);
  },

  /// POST /orgs/<slug>/keys/grants {device_id, version, blob, by_device}:
  /// a holder device passes the keyring to a waiting device of an active
  /// member. `by_device: "recovery"` is an owner or admin restoring their
  /// own new device from the recovery code.
  op_keys_grant(ctx) {
    const a = ctx.args;
    const id = String(a.device_id || "");
    const target = `device:${id}`;
    const { me, refusal } = this.keyStanding(ctx, "keys.grant", target);
    if (refusal) return refusal;
    const cur = this.currentKey();
    if (!cur) return this.deny(ctx, "keys.grant", target, "no-org-key");
    if (a.version !== cur.version) return this.deny(ctx, "keys.grant", target, "key-rotated", `the current key is version ${cur.version}`);
    if (!goodBlob(a.blob)) return { status: 400, body: { error: "bad-request", message: "the envelope is base64, under 64 KiB" } };
    const to = this.deviceRow(id);
    const member = to ? this.member(to.principal) : null;
    if (!to || to.state !== "active" || !member || member.state !== "active") {
      return this.deny(ctx, "keys.grant", target, "device-unknown", "that device is not an active member's");
    }
    let how;
    if (a.by_device === "recovery") {
      if (allows(me.role, "keys.recover") !== "yes") return this.deny(ctx, "keys.grant", target, "role-cannot", "only an owner or admin restores from the recovery code");
      if (to.principal !== me.principal) return this.deny(ctx, "keys.grant", target, "role-cannot", "the recovery code restores your own device");
      how = "recovery code";
    } else {
      const by = this.myDevice(ctx, me, a.by_device);
      const held = by ? this.envelopeOf(by.device_id) : null;
      if (!by || !held || held.version !== cur.version) return this.deny(ctx, "keys.grant", target, "key-not-held");
      how = by.device_id;
    }
    this.sql.exec(
      `INSERT INTO key_envelopes (device_id, version, blob, granted_by, granted_device, at) VALUES (?, ?, ?, ?, ?, ?)
       ON CONFLICT (device_id) DO UPDATE SET version = excluded.version, blob = excluded.blob,
         granted_by = excluded.granted_by, granted_device = excluded.granted_device, at = excluded.at`,
      id, cur.version, a.blob, me.principal, how, ctx.now,
    );
    // The audit says which device now holds which key, for whom, from whom.
    this.audit(me.principal, a.by_device === "recovery" ? "key.recover" : "key.grant", target, "allowed", `v${cur.version} to ${to.principal} by ${how}`);
    return this.ok({ device_id: id, version: cur.version });
  },

  /// GET /orgs/<slug>/keys/recovery (owner, admin): the recovery envelope
  /// and its parameters. Useless without the code; the read is audited.
  op_keys_recovery(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.keyStanding(ctx, "keys.recover", target);
    if (refusal) return refusal;
    if (allows(me.role, "keys.recover") !== "yes") return this.deny(ctx, "keys.recover", target, "role-cannot", "only an owner or admin restores from the recovery code");
    const r = this.row("SELECT * FROM key_recovery WHERE id = 1");
    if (!r) return this.deny(ctx, "keys.recover", target, "no-org-key");
    this.audit(me.principal, "key.recovery.read", target, "allowed");
    return this.ok({ version: r.version, public_key: r.public_key, params: JSON.parse(r.params), blob: r.blob });
  },

  /// POST /orgs/<slug>/keys/rotate: after a removal, the next version,
  /// made on a holder device and sealed to every remaining device and to
  /// the recovery key.
  op_keys_rotate(ctx) {
    const a = ctx.args;
    const target = `key:v${a.version}`;
    const { me, refusal } = this.keyStanding(ctx, "keys.rotate", target);
    if (refusal) return refusal;
    const cur = this.currentKey();
    if (!cur) return this.deny(ctx, "keys.rotate", target, "no-org-key");
    if (cur.state !== "stale") return { status: 409, body: { error: "key-rotated", message: `version ${cur.version} is current and not stale: nothing to rotate` } };
    if (a.version !== cur.version + 1 || !HEX64.test(String(a.public_key || ""))) {
      return { status: 400, body: { error: "bad-request", message: `the next version is ${cur.version + 1}, with its 64-hex public key` } };
    }
    const by = this.myDevice(ctx, me, a.by_device);
    const held = by ? this.envelopeOf(by.device_id) : null;
    if (!by || !held || held.version !== cur.version) return this.deny(ctx, "keys.rotate", target, "key-not-held");
    const envelopes = a.envelopes && typeof a.envelopes === "object" ? a.envelopes : {};
    const live = this.liveDevices();
    const holders = live.filter((d) => this.envelopeOf(d.device_id));
    const missing = holders.find((d) => !goodBlob(envelopes[d.device_id]));
    if (missing) return { status: 400, body: { error: "bad-request", message: `every device that holds the key gets the new one: ${missing.device_id} is missing` } };
    const rec = this.row("SELECT * FROM key_recovery WHERE id = 1");
    if (rec && !goodBlob(a.recovery_blob)) return { status: 400, body: { error: "bad-request", message: "the recovery envelope is re-sealed with the new version" } };
    this.sql.exec("UPDATE key_versions SET state = 'old' WHERE version = ?", cur.version);
    this.sql.exec(
      "INSERT INTO key_versions (version, public_key, state, created_at, created_by, created_device) VALUES (?, ?, 'current', ?, ?, ?)",
      a.version, a.public_key, ctx.now, me.principal, by.device_id,
    );
    // Only live devices: a removed member's devices get nothing new.
    this.sql.exec("DELETE FROM key_envelopes");
    for (const d of live) {
      const blob = envelopes[d.device_id];
      if (!goodBlob(blob)) continue;
      this.sql.exec(
        "INSERT INTO key_envelopes (device_id, version, blob, granted_by, granted_device, at) VALUES (?, ?, ?, ?, ?, ?)",
        d.device_id, a.version, blob, me.principal, by.device_id, ctx.now,
      );
    }
    if (rec) this.sql.exec("UPDATE key_recovery SET version = ?, blob = ?, set_at = ?, set_by = ? WHERE id = 1", a.version, a.recovery_blob, ctx.now, me.principal);
    this.audit(me.principal, "key.rotate", target, "allowed", `by ${by.device_id}, to ${live.filter((d) => goodBlob(envelopes[d.device_id])).length} devices`);
    return this.ok({ version: a.version });
  },
};

/// What the standard export carries of the keyring: public keys,
/// envelopes and the recovery envelope. Reading anything sealed needs a
/// member device or the recovery code.
function exportKeys(store) {
  return {
    versions: store.rows("SELECT version, public_key, state, created_at, created_by, created_device FROM key_versions ORDER BY version"),
    devices: store.rows("SELECT device_id, principal, public_key, name, added_at, state FROM key_devices ORDER BY added_at"),
    envelopes: store.rows("SELECT device_id, version, blob, granted_by, granted_device, at FROM key_envelopes ORDER BY device_id"),
    recovery: store.row("SELECT version, public_key, params, blob FROM key_recovery WHERE id = 1"),
  };
}

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({
    schema: SCHEMA,
    // Which kind of device (IC-958): a browser, or a computer running Krate.
    columns: [["key_devices", "kind", "TEXT"]],
    mutations: ["keys-device-add", "keys-init", "keys-grant", "keys-rotate"],
  });
  const P = OrgClass.prototype;
  for (const name of ["op_remove", "op_export_begin", "op_swept"]) {
    if (typeof P[name] !== "function") throw new Error(`org-keys: ${name} is not installed`);
  }

  // Removing a member ends their devices' hold: their envelopes go, and the
  // key is stale until a remaining device makes the next version. The same
  // for a SCIM deactivation (org-signing.js calls this from scimRemove,
  // which is installed after this module).
  P.keysEndMember = function (actor, who, now) {
    let ended = 0;
    for (const d of this.rows("SELECT device_id FROM key_devices WHERE principal = ? AND state = 'active'", who)) {
      this.sql.exec("UPDATE key_devices SET state = 'removed', removed_at = ? WHERE device_id = ?", now, d.device_id);
      this.sql.exec("DELETE FROM key_envelopes WHERE device_id = ?", d.device_id);
      this.audit(actor, "key.device.remove", `device:${d.device_id}`, "allowed", "member removed");
      ended += 1;
    }
    const cur = this.currentKey();
    if (cur && cur.state === "current") {
      this.sql.exec("UPDATE key_versions SET state = 'stale', stale_since = ? WHERE version = ?", now, cur.version);
      this.audit(actor, "key.stale", `key:v${cur.version}`, "allowed", `member removed: ${displayOf(who)}`);
    }
    return ended;
  };
  const remove = P.op_remove;
  P.op_remove = function (ctx) {
    const who = this.targetOf(ctx);
    const answer = remove.call(this, ctx);
    if (answer.status === 204 && who) this.keysEndMember(ctx.actor, who, ctx.now);
    return answer;
  };

  const begin = P.op_export_begin;
  P.op_export_begin = function (ctx) {
    const answer = begin.call(this, ctx);
    if (answer.status === 202 && answer.body && answer.body.data) answer.body.data.keyring = exportKeys(this);
    return answer;
  };

  const swept = P.op_swept;
  P.op_swept = function (ctx) {
    const answer = swept.call(this, ctx);
    if (answer.status === 200) {
      for (const t of ["key_envelopes", "key_recovery", "key_versions", "key_devices"]) this.sql.exec(`DELETE FROM ${t}`);
      this.audit("system:krate", "key.deleted", `org:${ctx.slug}`, "allowed");
    }
    return answer;
  };
}

install(Org);

// ---------------------------------------------------------------- routes


/// /orgs/<slug>/keys... Answers a Response, or null.
export async function orgKeysRoute(h) {
  const { url, rest, method, askRaw, mutateRaw, orgAnswer } = h;
  if (rest[0] !== "keys") return null;
  if (rest.length === 1 && method === "GET") return orgAnswer(await askRaw("keys-view", { device: url.searchParams.get("device") || "" }));
  if (rest.length === 2 && rest[1] === "recovery" && method === "GET") return orgAnswer(await askRaw("keys-recovery"));
  if (rest.length === 3 && rest[1] === "devices" && method === "PUT") {
    const id = String(rest[2]).toLowerCase();
    const answer = await mutateRaw("keys-device-add", (b) => ({ device_id: id, public_key: String(b.public_key || "").toLowerCase(), name: b.name, kind: b.kind }));
    // A computer added is a security item the channels hear (org-chat.js).
    if (answer.status === 200 && !answer.replayed && h.notify) await h.notify({ event: "security" });
    return orgAnswer(answer);
  }
  if (rest.length === 2 && rest[1] === "init" && method === "POST") return orgAnswer(await mutateRaw("keys-init", (b) => b));
  if (rest.length === 2 && rest[1] === "grants" && method === "POST") return orgAnswer(await mutateRaw("keys-grant", (b) => b));
  if (rest.length === 2 && rest[1] === "rotate" && method === "POST") return orgAnswer(await mutateRaw("keys-rotate", (b) => b));
  return null;
}
