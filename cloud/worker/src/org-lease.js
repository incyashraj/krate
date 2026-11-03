// Offline access to an organisation's private release (IC-948, the
// contract's Addendum 3 section G and Addendum 4 G1-G2).
//
// A lease is the hub's signed statement that one principal may open one
// release digest of one organisation without a connection, until a time.
// Three classes exist and no others: `online` (no lease; every open checks
// in), `days-7` (the default) and `days-30` (the most there is). An owner
// picks one per app name; the organisation's default covers the rest.
//
// Issuing is decided and recorded inside the organisation's store, in one
// transaction that also checks the person's standing: a removal and an
// issue racing each other happen in some order, and a removed member is
// never issued one (R1). The Worker then signs exactly what the store
// recorded with the hub's lease key (Ed25519, ORG_LEASE_KEY), whose public
// half is served at /lease-key and pinned in the CLI and runtime.
//
// The honest limit: a lease cannot be withdrawn from a device that stays
// offline (R9). It stops at its expiry, or when the device next connects
// and its check-in is refused.

import { Org, extendOrg, allows, canonicalJson } from "./org.js";

const DAY = 24 * 60 * 60;

/// Every class there is, and how long a lease of it lasts. Nothing longer
/// than LEASE_MAX_SECONDS may ever be in this table; the tests hold it to
/// that, and the signer refuses anything longer whatever the table says.
export const LEASE_CLASSES = Object.freeze({ online: 0, "days-7": 7 * DAY, "days-30": 30 * DAY });
export const LEASE_MAX_SECONDS = 30 * DAY;
export const LEASE_DEFAULT_CLASS = "days-7";

/// A lease asked for again within this long, for the same release and the
/// same class, is the lease already issued: a client refreshing too eagerly
/// costs no new audit event.
export const LEASE_REUSE_SECONDS = 60 * 60;

/// The bytes a lease signature covers. Fixed forever for `v: 1`.
export const LEASE_DOMAIN = "krate-lease-v1\n";

export const LEASE_SCHEMA = [
  `CREATE TABLE IF NOT EXISTS offline_settings (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     default_class TEXT NOT NULL, set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS offline_classes (
     name TEXT PRIMARY KEY, shown TEXT NOT NULL, class TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS leases (
     lease_id TEXT PRIMARY KEY, digest TEXT NOT NULL, principal TEXT NOT NULL,
     class TEXT NOT NULL, issued_at INTEGER NOT NULL, expires_at INTEGER NOT NULL)`,
];

export function validClass(value) {
  return typeof value === "string" && Object.prototype.hasOwnProperty.call(LEASE_CLASSES, value);
}

const appKey = (name) => String(name || "").trim().toLowerCase();

const isoOf = (seconds) => new Date(seconds * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");

function leaseOps() {
  return {
    offlineDefault() {
      const r = this.row("SELECT default_class FROM offline_settings WHERE id = 1");
      return r && validClass(r.default_class) ? r.default_class : LEASE_DEFAULT_CLASS;
    },

    /// The class that governs an app name: the owner's choice for it, or
    /// the organisation's default.
    offlineClassFor(name) {
      const r = this.row("SELECT class FROM offline_classes WHERE name = ?", appKey(name));
      return r && validClass(r.class) ? r.class : this.offlineDefault();
    },

    offlineRefusal(ctx, action, target) {
      const { me, refusal } = this.standing(ctx, action, target);
      if (refusal) return refusal;
      if (me.role !== "owner") {
        return this.deny(ctx, action, target, "role-cannot", `only an owner chooses how long ${ctx.slug}'s apps work offline`);
      }
      return null;
    },

    /// PUT /orgs/<slug>/apps/<name>/offline {class} (owner).
    op_offline_set(ctx) {
      const name = String(ctx.args.name || "").trim();
      const target = `app:${name.slice(0, 80)}`;
      const refusal = this.offlineRefusal(ctx, "offline.set", target);
      if (refusal) return refusal;
      if (!name || name.length > 80) return { status: 400, body: { error: "bad-request", message: "an app name is 1 to 80 characters" } };
      const klass = ctx.args.class;
      if (!validClass(klass)) {
        return { status: 400, body: { error: "bad-request", message: `the offline class is one of ${Object.keys(LEASE_CLASSES).join(", ")}` } };
      }
      const before = this.row("SELECT class FROM offline_classes WHERE name = ?", appKey(name));
      if (!before || before.class !== klass) {
        this.sql.exec(
          `INSERT INTO offline_classes (name, shown, class, set_at, set_by) VALUES (?, ?, ?, ?, ?)
           ON CONFLICT (name) DO UPDATE SET shown = excluded.shown, class = excluded.class, set_at = excluded.set_at, set_by = excluded.set_by`,
          appKey(name),
          name,
          klass,
          ctx.now,
          ctx.actor,
        );
        this.audit(ctx.actor, "offline.set", target, "allowed", `${before ? before.class : "default"}->${klass}`);
      }
      return this.ok({ name, class: klass });
    },

    /// PUT /orgs/<slug>/offline-default {class} (owner).
    op_offline_default_set(ctx) {
      const refusal = this.offlineRefusal(ctx, "offline.default", `org:${ctx.slug}`);
      if (refusal) return refusal;
      const klass = ctx.args.class;
      if (!validClass(klass)) {
        return { status: 400, body: { error: "bad-request", message: `the offline class is one of ${Object.keys(LEASE_CLASSES).join(", ")}` } };
      }
      const before = this.offlineDefault();
      if (before !== klass || !this.row("SELECT id FROM offline_settings WHERE id = 1")) {
        this.sql.exec(
          `INSERT INTO offline_settings (id, default_class, set_at, set_by) VALUES (1, ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET default_class = excluded.default_class, set_at = excluded.set_at, set_by = excluded.set_by`,
          klass,
          ctx.now,
          ctx.actor,
        );
        if (before !== klass) this.audit(ctx.actor, "offline.default", `org:${ctx.slug}`, "allowed", `${before}->${klass}`);
      }
      return this.ok({ offline_default: klass });
    },

    /// GET /orgs/<slug>/leases/<digest>: decide, record and audit one lease,
    /// in this one transaction. The Worker signs what this returns.
    op_lease(ctx) {
      const digest = String(ctx.args.digest || "");
      const target = `release:${digest}`;
      const { me, refusal } = this.standing(ctx, "lease.issue", target);
      if (refusal) return refusal;
      if (allows(me.role, "release.download") !== "yes") return this.deny(ctx, "lease.issue", target, "role-cannot");
      const candidate = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
      if (!candidate || candidate.state !== "released") {
        return this.deny(ctx, "lease.issue", target, "digest-mismatch", "that digest is not a release");
      }
      const klass = this.offlineClassFor(candidate.name);
      if (klass === "online") return this.ok({ class: "online", lease: null });
      const seconds = LEASE_CLASSES[klass];
      // The bound, again, where the decision is made.
      if (!(seconds > 0 && seconds <= LEASE_MAX_SECONDS)) throw new Error(`offline class ${klass} is outside the bound`);
      const recent = this.row(
        "SELECT * FROM leases WHERE digest = ? AND principal = ? AND class = ? AND issued_at > ? ORDER BY issued_at DESC LIMIT 1",
        digest,
        me.principal,
        klass,
        ctx.now - LEASE_REUSE_SECONDS,
      );
      const row =
        recent ||
        (() => {
          const lease = {
            lease_id: [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join(""),
            digest,
            principal: me.principal,
            class: klass,
            issued_at: ctx.now,
            expires_at: ctx.now + seconds,
          };
          this.sql.exec(
            "INSERT INTO leases (lease_id, digest, principal, class, issued_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
            lease.lease_id,
            lease.digest,
            lease.principal,
            lease.class,
            lease.issued_at,
            lease.expires_at,
          );
          // "offline access issued, until <date>" -- the audit's words.
          this.audit(me.principal, "lease.issue", target, "allowed", `${klass} until ${isoOf(lease.expires_at)}`);
          return lease;
        })();
      return this.ok({
        class: klass,
        lease: {
          v: 1,
          org: this.org().slug,
          digest: row.digest,
          principal: row.principal,
          class: row.class,
          issued_at: row.issued_at,
          expires_at: row.expires_at,
          lease_id: row.lease_id,
        },
      });
    },
  };
}

/// Teach the Org class offline access: its commands (only an owner sets a
/// class, R25; the contract's role table stays as it is), the default on the organisation's view, the class on
/// each release and on the private-release answer.
function installLeases(OrgClass) {
  Object.assign(OrgClass.prototype, leaseOps());
  extendOrg({
    schema: LEASE_SCHEMA,
    mutations: ["offline-set", "offline-default-set"],
    // G1: GET /orgs/<slug> carries the organisation's default class.
    view(org) {
      org.offline_default = this.offlineDefault();
    },
  });

  const releases = OrgClass.prototype.op_releases;
  OrgClass.prototype.op_releases = function (ctx) {
    const answer = releases.call(this, ctx);
    if (answer.status === 200 && Array.isArray(answer.body)) {
      answer.body = answer.body.map((r) => ({ ...r, offline: this.offlineClassFor(r.name) }));
    }
    return answer;
  };

  // The people list, for the owners and admins who remove people: until
  // when each member's offline access already handed out still opens apps
  // on a computer that stays offline (R9). Removal stops everything online
  // at once; this is what it cannot call back, said before anyone confirms.
  const show = OrgClass.prototype.op_show;
  OrgClass.prototype.op_show = function (ctx) {
    const answer = show.call(this, ctx);
    if (answer.status !== 200 || !answer.body || !Array.isArray(answer.body.members)) return answer;
    const me = answer.body.me;
    if (!me || (me.role !== "owner" && me.role !== "admin")) return answer;
    const held = new Map(
      this.rows("SELECT principal, MAX(expires_at) AS until FROM leases WHERE expires_at > ? GROUP BY principal", ctx.now).map((r) => [r.principal, r.until]),
    );
    for (const m of answer.body.members) if (held.has(m.principal)) m.offline_until = held.get(m.principal);
    return answer;
  };

  const view = OrgClass.prototype.op_view;
  OrgClass.prototype.op_view = function (ctx) {
    const answer = view.call(this, ctx);
    if (answer.status === 200 && answer.body) {
      const klass = this.offlineClassFor(answer.body.name);
      answer.body.offline = { class: klass, until: klass === "online" ? null : ctx.now + LEASE_CLASSES[klass] };
    }
    return answer;
  };
}

// Importing this module teaches the store offline access.
installLeases(Org);

// ------------------------------------------------------------- signing

const hex = (bytes) => [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");
const unhex = (text) => Uint8Array.from(String(text).match(/../g) || [], (h) => parseInt(h, 16));

/// The exact bytes an Ed25519 lease signature covers: "krate-lease-v1\n"
/// then the lease as canonical JSON (keys sorted, no whitespace). Every
/// value in a v1 lease is a short ASCII string or an integer, so the JSON
/// is plain ASCII with nothing to escape.
export function leaseSigningBytes(lease) {
  return new TextEncoder().encode(LEASE_DOMAIN + canonicalJson(lease));
}

/// The hub's lease key from ORG_LEASE_KEY, the 32-byte Ed25519 seed as 64
/// hex characters. Answers { privateKey, publicKey (hex), keyId }, or null
/// when it is not set.
export async function leaseKey(env) {
  const seed = String(env.ORG_LEASE_KEY || "").trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(seed)) return null;
  // PKCS#8 for an Ed25519 seed is a fixed 16-byte prefix and the seed.
  const pkcs8 = new Uint8Array([...unhex("302e020100300506032b657004220420"), ...unhex(seed)]);
  const privateKey = await crypto.subtle.importKey("pkcs8", pkcs8, { name: "Ed25519" }, true, ["sign"]);
  const jwk = await crypto.subtle.exportKey("jwk", privateKey);
  const s = jwk.x.replace(/-/g, "+").replace(/_/g, "/");
  const pub = Uint8Array.from(atob(s + "=".repeat((4 - (s.length % 4)) % 4)), (c) => c.charCodeAt(0));
  const keyId = hex(await crypto.subtle.digest("SHA-256", pub)).slice(0, 16);
  return { privateKey, publicKey: hex(pub), keyId };
}

/// Sign one lease. Refuses anything that is not a v1 lease inside the
/// bound, whatever produced it.
export async function signLease(env, lease) {
  const key = await leaseKey(env);
  if (!key) return null;
  const life = lease.expires_at - lease.issued_at;
  if (lease.v !== 1 || !validClass(lease.class) || !(life > 0 && life <= LEASE_MAX_SECONDS) || life !== LEASE_CLASSES[lease.class]) {
    throw new Error("refusing to sign a lease outside the bound");
  }
  const signature = await crypto.subtle.sign({ name: "Ed25519" }, key.privateKey, leaseSigningBytes(lease));
  return { lease, signature: hex(signature), key_id: key.keyId, alg: "Ed25519" };
}

/// Check a signed lease against a public key (hex). What the CLI and the
/// runtime do in Rust; here for the tests and as the reference.
export async function verifyLease(signed, publicKeyHex) {
  const pub = await crypto.subtle.importKey("raw", unhex(publicKeyHex), { name: "Ed25519" }, false, ["verify"]);
  return crypto.subtle.verify({ name: "Ed25519" }, pub, unhex(signed.signature), leaseSigningBytes(signed.lease));
}

const jsonAnswer = (value, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", "cache-control": "private, no-store" } });

const leasesOff = () =>
  jsonAnswer({ error: "leases-off", message: "offline access is not switched on on this hub (it has no ORG_LEASE_KEY)" }, 503);

/// GET /lease-key: the public half of the hub's lease key.
export async function leaseKeyRoute(env) {
  const key = await leaseKey(env);
  if (!key) return leasesOff();
  return new Response(JSON.stringify({ alg: "Ed25519", format: "krate-lease-v1", public_key: key.publicKey, key_id: key.keyId }), {
    headers: { "content-type": "application/json", "cache-control": "public, max-age=300" },
  });
}

/// /orgs/<slug>/leases/<digest>, /orgs/<slug>/apps/<name>/offline and
/// /orgs/<slug>/offline-default. Answers a Response, or null.
export async function leaseRoute({ request, env, rest, method, askRaw, readBody, idempotencyOf, path, answer }) {
  if (rest[0] === "leases" && rest.length === 2 && method === "GET") {
    const digest = String(rest[1]).toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(digest)) {
      return answer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the release's 64-hex sha256" } });
    }
    // No key, no lease: refused before the store records one it could
    // never sign.
    if (!(await leaseKey(env))) return leasesOff();
    const decided = await askRaw("lease", { digest });
    if (decided.status !== 200) return answer(decided);
    if (!decided.body.lease) return jsonAnswer({ class: "online", lease: null });
    return jsonAnswer(await signLease(env, decided.body.lease));
  }
  const put = async (op, argsOf) => {
    const body = await readBody();
    if (body.tooLarge || body.bad) return answer({ status: 400, body: { error: "bad-request", message: "the body is JSON, under 64 KiB" } });
    return answer(await askRaw(op, argsOf(body.value), await idempotencyOf(request, path, body.text)));
  };
  if (rest[0] === "apps" && rest.length === 3 && rest[2] === "offline" && method === "PUT") {
    return put("offline-set", (b) => ({ name: rest[1], class: b.class }));
  }
  if (rest[0] === "offline-default" && rest.length === 1 && method === "PUT") {
    return put("offline-default-set", (b) => ({ class: b.class }));
  }
  return null;
}
