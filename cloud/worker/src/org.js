// Organisations (IC-946): members, private releases, two-person release,
// removal, and a hash-chained audit.
//
// One SQLite-backed Durable Object per organisation, named "org:<slug>". It
// is the only authority for membership, roles, invitations, the root key,
// candidates, releases and the audit (chapter 14 rules R5, R6): every
// question about who may do what is answered here, from current state, on
// every request. Nothing in KV answers an authorisation question.
//
// Every command runs inside one `transactionSync`, and does no `await`
// inside it. That is what makes the invariants hold: a Durable Object runs
// one event at a time, and a synchronous transaction cannot be interleaved
// with another request's -- so two removals of the last two owners racing
// each other are two transactions in some order, and the second one sees
// the first one's result. It is also why the audit's SHA-256 is computed
// here in plain JavaScript rather than with crypto.subtle, which only
// answers asynchronously.
//
// A denial is a committed transaction too: it appends its audit event and
// returns the refusal. Only a bug throws, and a throw rolls everything back.

/// The executable role matrix (the contract's table, ch 14 7.2). Each
/// action maps a role to "yes", or to a narrower rule the command applies:
/// "own" (sees only their own entry), "self" (acts only on themselves) and
/// "not-owners" (acts on anyone who is not an owner). A role missing from
/// a row is denied -- default deny, R1.
export const ROLES = ["owner", "admin", "developer", "reviewer", "recipient"];
export const MATRIX = {
  "org.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "own" },
  "invite.owner-admin": { owner: "yes" },
  "invite.other": { owner: "yes", admin: "yes" },
  "role.change": { owner: "yes", admin: "yes" },
  "role.owner": { owner: "yes" },
  "member.remove": {
    owner: "yes",
    admin: "not-owners",
    developer: "self",
    reviewer: "self",
    recipient: "self",
  },
  "root-key.set": { owner: "yes" },
  "candidate.publish": { owner: "yes", developer: "yes" },
  "candidate.approve": { owner: "yes", reviewer: "yes" },
  "release.download": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
  "audit.read": { owner: "yes", admin: "yes" },
  // Release control and groups (IC-949, FLEET-CONTRACT.md R); the commands
  // are in org-releases.js.
  "release.withdraw": { owner: "yes", admin: "yes", reviewer: "yes" },
  "release.set-current": { owner: "yes", admin: "yes" },
  "group.manage": { owner: "yes", admin: "yes" },
  "release.audience": { owner: "yes", admin: "yes" },
  // Company connections (IC-952); the commands are in org-connections.js.
  // Who sets one up and holds its credential; who sees what exists (the
  // people who build and approve the apps that use one); who an app may
  // use one for -- anyone who may open the organisation's apps, narrowed
  // by the connection's own audience and app list.
  "connection.manage": { owner: "yes", admin: "yes" },
  "connection.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes" },
  "connection.use": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
  // SCIM provisioning and audit export (IC-955): org-scim.js and
  // org-audit-export.js. An owner sets them up; an admin may watch the
  // export's delivery, as an admin reads the audit.
  "scim.manage": { owner: "yes" },
  "audit-export.manage": { owner: "yes" },
  "audit-export.view": { owner: "yes", admin: "yes" },
  // Team data for the organisation's apps (IC-951, org-data.js): whoever
  // may open a release may use its data; owners and admins see the counts.
  "data.use": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
  "data.usage": { owner: "yes", admin: "yes" },
  // The organisation keyring (F-316, org-keys.js): every member's device
  // may hold the key and pass it on; an owner makes it; an owner or admin
  // restores a device from the recovery code.
  "keys.hold": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
  "keys.init": { owner: "yes" },
  "keys.recover": { owner: "yes", admin: "yes" },
  // Inventory, ownership and usage (IC-954): org-inventory.js. The people
  // who run and review the organisation's apps read the inventory; only
  // owners and admins see who opened each, choose an app's owner, and read
  // the daily usage counts.
  "inventory.view": { owner: "yes", admin: "yes", reviewer: "yes" },
  "inventory.people": { owner: "yes", admin: "yes" },
  "app.owner": { owner: "yes", admin: "yes" },
  "usage.view": { owner: "yes", admin: "yes" },
  // Insights (IC-959): org-insights.js. Every member reads the section
  // about themselves; owners and admins the organisation's; whoever sends
  // apps the one about their apps; whoever approves the queue's. Names of
  // people, and counts under 3 people, only for owners and admins.
  "insights.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
  "insights.org": { owner: "yes", admin: "yes" },
  "insights.developer": { owner: "yes", developer: "yes" },
  "insights.reviewer": { owner: "yes", reviewer: "yes" },
  "insights.people": { owner: "yes", admin: "yes" },
};

/// What `role` may do for `action`: "yes", a narrower rule, or null.
export function allows(role, action) {
  const row = MATRIX[action];
  if (!row) return null;
  return row[role] || null;
}

export const SLUG = /^[a-z][a-z0-9-]{1,38}$/;
export const LOGIN = /^[a-z0-9](?:[a-z0-9-]{0,38})$/;
export const EMAIL = /^[a-z0-9._%+'-]{1,64}@[a-z0-9-]+(\.[a-z0-9-]+)+$/;
export const INVITE_SECONDS = 7 * 24 * 60 * 60;

/// A principal is who a member, inviter, uploader, approver or audit actor
/// is (IC-947): `github:<login>` when GitHub vouched for the login, or
/// `email:<address>` when the person proved that inbox. Never a bare login:
/// a Krate session's `login` is the part of an address before the @, and
/// comparing it with a GitHub login let a chosen address be someone else
/// (K-1085). Answers `{ principal, kind, value }`, or null.
export function parsePrincipal(raw) {
  const text = String(raw || "").trim().toLowerCase();
  const colon = text.indexOf(":");
  if (colon <= 0) return null;
  const kind = text.slice(0, colon);
  const value = text.slice(colon + 1);
  if (kind === "github" && LOGIN.test(value)) return { principal: text, kind, value };
  if (kind === "email" && value.length <= 254 && EMAIL.test(value)) return { principal: text, kind, value };
  return null;
}

/// What a person reads for a principal: the GitHub login, or the address.
export function displayOf(principal) {
  const p = parsePrincipal(principal);
  return p ? p.value : String(principal || "");
}
export const GENESIS = "0".repeat(64);

/// Plain-words messages for each stable reason. The reason is the contract;
/// the message is for the person, and says what to do next.
const MESSAGES = {
  "not-signed-in": "sign in first -- run `krate account login`, then try again",
  "not-a-member": (o, d) => d || `you are not a member of ${o} -- ask one of its owners to invite you`,
  removed: (o) => `you were removed from ${o} -- ask one of its owners if that is a mistake`,
  "role-cannot": (o, d) => d || `your role in ${o} cannot do that`,
  "last-owner": (o) =>
    `that would leave ${o} with no owner -- make someone else an owner first`,
  "self-approval": (o, d) =>
    d || "you uploaded this candidate, so someone else with review authority has to approve it",
  "digest-mismatch": (o, d) => d || `no candidate with that digest in ${o} -- check the digest with \`krate org releases\``,
  "not-signed-for-this-org": (o, d) =>
    `this app is not signed for ${o}: ${d || "it needs the organisation's root key or a release key that root delegated"}`,
  "not-marked-for-this-org": (o, d) =>
    d ||
    `this app is not marked as ${o}'s private app -- mark and sign it with \`krate sign app.krate --org ${o}\`, then publish again`,
  "invite-expired": () => "this invitation has expired -- ask for a new one",
  "invite-used": () => "this invitation has already been used",
  "invite-for-someone-else": () =>
    "this invitation is for a different GitHub account -- sign in as the person it names",
  "invite-revoked": (o, d) => d || "this invitation was withdrawn -- ask for a new one",
  "slug-taken": (o) => `${o} is already taken -- pick another name`,
  "slug-invalid": () =>
    "an organisation name is 2 to 39 characters: lower-case letters, digits and dashes, starting with a letter",
  "idempotency-key-reused": () =>
    "that Idempotency-Key was already used for a different request -- use a new key",
  "no-such-org": (o) => `there is no organisation called ${o}`,
};

/// More stable reasons, from a module that extends the store (Addendum 3):
/// `{ reason: { message: string | (slug, detail) => string, status } }`.
export function addReasons(reasons) {
  for (const [reason, { message, status }] of Object.entries(reasons)) {
    MESSAGES[reason] = message;
    STATUS[reason] = status;
  }
}

export function messageFor(reason, slug, detail) {
  const m = MESSAGES[reason];
  if (typeof m === "function") return m(slug, detail);
  return m || detail || reason;
}

const STATUS = {
  "not-signed-in": 401,
  "not-a-member": 403,
  removed: 403,
  "role-cannot": 403,
  "last-owner": 409,
  "self-approval": 403,
  "digest-mismatch": 404,
  "not-signed-for-this-org": 422,
  "not-marked-for-this-org": 422,
  "invite-expired": 410,
  "invite-used": 410,
  "invite-for-someone-else": 403,
  "invite-revoked": 410,
  "slug-taken": 409,
  "slug-invalid": 400,
  "idempotency-key-reused": 409,
  "no-such-org": 404,
};

// ------------------------------------------------------- canonical hashing

/// JSON with every object's keys sorted and no whitespace: one event has
/// one encoding, so its hash can be recomputed by anyone holding it.
export function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value === undefined ? null : value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  const keys = Object.keys(value).sort();
  return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson(value[k])}`).join(",")}}`;
}

/// What an event's hash covers: every field but the hash itself, the
/// previous event's hash among them.
export function eventPreimage(event) {
  return canonicalJson({
    seq: event.seq,
    at: event.at,
    actor: event.actor ?? null,
    action: event.action,
    target: event.target ?? null,
    result: event.result,
    reason: event.reason ?? null,
    prev_hash: event.prev_hash,
  });
}

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

/// SHA-256 of a string's UTF-8 bytes, as hex, synchronously -- so it can
/// run inside a transaction (see the top of this file). Checked against
/// crypto.subtle in the tests, and the audit route recomputes every hash
/// with crypto.subtle, so a fault here shows as chain_ok false rather
/// than passing quietly.
export function sha256HexSync(text) {
  const data = new TextEncoder().encode(text);
  const bitLength = data.length * 8;
  const padded = new Uint8Array(((data.length + 9 + 63) >> 6) << 6);
  padded.set(data);
  padded[data.length] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(padded.length - 8, Math.floor(bitLength / 2 ** 32));
  view.setUint32(padded.length - 4, bitLength >>> 0);
  const h = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const w = new Uint32Array(64);
  const rotr = (x, n) => (x >>> n) | (x << (32 - n));
  for (let off = 0; off < padded.length; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = view.getUint32(off + i * 4);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (hh + S1 + ch + K[i] + w[i]) >>> 0;
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) >>> 0;
      hh = g;
      g = f;
      f = e;
      e = (d + t1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) >>> 0;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
  }
  return [...h].map((x) => x.toString(16).padStart(8, "0")).join("");
}

/// Recompute a chain of events (async, with crypto.subtle -- a second
/// implementation from the one that wrote them). True only when every
/// event's sequence follows the last, its prev_hash is the last event's
/// hash, and its own hash is what its fields hash to.
export async function chainOk(events) {
  let prev = GENESIS;
  let seq = 0;
  for (const event of events) {
    if (event.seq !== seq + 1 || event.prev_hash !== prev) return false;
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(eventPreimage(event)));
    const hex = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
    if (hex !== event.hash) return false;
    prev = event.hash;
    seq = event.seq;
  }
  return true;
}

// ------------------------------------------------------------ signatures

/// Why a judged signature does not make an app this organisation's, or
/// null when it does: signed by the organisation's root, or by a release
/// key that root delegated, whose slip is genuine, covers the namespace,
/// was in force when it signed, has not run out by now, and was not
/// withdrawn (IC-946). `sig` is the validator's judgement.signature.
///
/// Two checks here are deliberately stricter than the runtime's. A slip
/// that has run out by the time of upload refuses, even if the bundle
/// says it was signed earlier, because `signed_at` is the signer's own
/// claim and a holder of a stale key could write any time it liked. And a
/// withdrawn key refuses outright, whatever its compromise time: the hub
/// sees the upload happen now, so nothing a withdrawn key brings in is
/// older than the withdrawal from the organisation's point of view.
export function notSignedForOrg(sig, root, revoked, nowSeconds) {
  if (!root) return "the organisation has no root key yet -- an owner registers one with `krate org root-key`";
  if (!sig || sig.state === "absent") return "it is not signed";
  if (sig.state !== "valid") return `its signature does not verify (${sig.state})`;
  const chain = sig.chain || null;
  if (!chain) {
    return sig.public_key === root ? null : "it is signed by a key that is not the organisation's root";
  }
  if (chain.state !== "revocation-unchecked") {
    return chain.detail || `its release key's permission slip does not hold (${chain.state})`;
  }
  if (chain.root !== root) return "its release key was delegated by a different root";
  if (chain.key !== sig.public_key) return "its permission slip is for a different key";
  if (Number.isFinite(chain.expires) && nowSeconds > chain.expires) {
    return "its release key's permission slip has run out";
  }
  if (revoked.has(chain.key)) return "its release key was withdrawn by the organisation's root";
  return null;
}

// ---------------------------------------------------- the Durable Object

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS org (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     slug TEXT NOT NULL, name TEXT NOT NULL,
     created_at INTEGER NOT NULL, created_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS members (
     principal TEXT PRIMARY KEY, role TEXT NOT NULL, state TEXT NOT NULL,
     added_at INTEGER NOT NULL, added_by TEXT,
     removed_at INTEGER, removed_by TEXT)`,
  `CREATE TABLE IF NOT EXISTS invites (
     code TEXT PRIMARY KEY, principal TEXT NOT NULL, role TEXT NOT NULL,
     issued_by TEXT NOT NULL, issued_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
     state TEXT NOT NULL, closed_at INTEGER, closed_by TEXT)`,
  `CREATE TABLE IF NOT EXISTS root_keys (
     seq INTEGER PRIMARY KEY, public_key TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS revoked_keys (
     key TEXT PRIMARY KEY, compromised_from INTEGER, reason TEXT,
     root TEXT NOT NULL, recorded_at INTEGER NOT NULL, recorded_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS candidates (
     digest TEXT PRIMARY KEY, name TEXT, version TEXT, namespace TEXT,
     size INTEGER, signer TEXT, big INTEGER NOT NULL DEFAULT 0,
     uploaded_by TEXT NOT NULL, uploaded_at INTEGER NOT NULL,
     state TEXT NOT NULL, approved_by TEXT, approved_at INTEGER)`,
  `CREATE TABLE IF NOT EXISTS idempotency (
     actor TEXT NOT NULL, key TEXT NOT NULL, request_hash TEXT NOT NULL,
     status INTEGER NOT NULL, body TEXT, at INTEGER NOT NULL,
     PRIMARY KEY (actor, key))`,
  `CREATE TABLE IF NOT EXISTS audit (
     seq INTEGER PRIMARY KEY, at INTEGER NOT NULL, actor TEXT,
     action TEXT NOT NULL, target TEXT, result TEXT NOT NULL, reason TEXT,
     prev_hash TEXT NOT NULL, hash TEXT NOT NULL)`,
];

/// What other modules add to the store (Addendum 3: lifecycle, billing,
/// sign-in, offline access), each in its own `org-*.js`, so two people
/// building different areas do not edit the same lines. Order is the
/// order they were added.
const EXTENSIONS = { schema: [], columns: [], gates: [], ticks: [], views: [] };

/// Add to the store:
/// - `schema`: CREATE statements, run with the store's own;
/// - `columns`: `[table, column, type]` added to a table that already
///   exists. CREATE TABLE IF NOT EXISTS never changes a table an older
///   build made, and live organisations keep their tables, so a column a
///   later build adds must be added to those too, or every command that
///   names it fails ("no such column");
/// - `ops`: `{ "op-name": function(ctx) }`, methods on the store;
/// - `mutations`: which of those ops take an Idempotency-Key;
/// - `tick(ctx)`: applies what is due before any command (a window that
///   ended, a wait that passed), inside the command's transaction;
/// - `gate(ctx)`: answers a refusal before the command runs, or null;
/// - `view(org, me)`: adds fields to `org` in GET /orgs/<slug>.
export function extendOrg({ schema = [], columns = [], ops = {}, mutations = [], tick, gate, view }) {
  EXTENSIONS.schema.push(...schema);
  EXTENSIONS.columns.push(...columns);
  for (const [name, fn] of Object.entries(ops)) {
    const key = `op_${name.replace(/-/g, "_")}`;
    // Loud, not silent: two modules naming one command would leave only
    // the last, and the other's route answering with the wrong command.
    if (Object.prototype.hasOwnProperty.call(Org.prototype, key)) throw new Error(`extendOrg: the command ${name} already exists`);
    Org.prototype[key] = fn;
  }
  for (const m of mutations) MUTATIONS.add(m);
  if (tick) EXTENSIONS.ticks.push(tick);
  if (gate) EXTENSIONS.gates.push(gate);
  if (view) EXTENSIONS.views.push(view);
}

/// The commands that change state. Each takes an Idempotency-Key.
const MUTATIONS = new Set([
  "create",
  "invite",
  "revoke-invite",
  "accept",
  "set-role",
  "remove",
  "set-root",
  "add-candidate",
  "approve",
]);

function notAMember(who, ctx) {
  return who
    ? `${displayOf(who)} is not a member of ${ctx.slug}`
    : "address a member by principal: github:<login> or email:<address>";
}

function inviteCode() {
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  const bytes = crypto.getRandomValues(new Uint8Array(20));
  return [...bytes].map((b) => alphabet[b % alphabet.length]).join("");
}

export class Org {
  constructor(ctx, env) {
    this.ctx = ctx;
    this.env = env;
    this.sql = ctx.storage.sql;
    for (const statement of [...SCHEMA, ...EXTENSIONS.schema]) this.sql.exec(statement);
    // Names below are code constants, never input, so they are spliced.
    for (const [table, column, type] of EXTENSIONS.columns) {
      const has = this.sql.exec(`PRAGMA table_info(${table})`).toArray().some((c) => c.name === column);
      if (!has) this.sql.exec(`ALTER TABLE ${table} ADD COLUMN ${column} ${type}`);
    }
  }

  /// Seconds since the epoch. A method so a test can move the clock.
  now() {
    return Math.floor(Date.now() / 1000);
  }

  rows(query, ...bindings) {
    return this.sql.exec(query, ...bindings).toArray();
  }

  row(query, ...bindings) {
    return this.rows(query, ...bindings)[0] || null;
  }

  async fetch(request) {
    let command;
    try {
      command = await request.json();
    } catch (_) {
      return new Response(JSON.stringify({ status: 400, body: { error: "bad-command", message: "not a command" } }), {
        status: 400,
      });
    }
    const answer = this.ctx.storage.transactionSync(() => this.run(command));
    return new Response(JSON.stringify(answer), { headers: { "content-type": "application/json" } });
  }

  /// One command, inside the caller's transaction. Answers `{status, body}`.
  run(command) {
    const { op, slug } = command;
    // Every principal the person proved, and the one they act as here: the
    // one this organisation knows them by, an active membership first and a
    // GitHub one before an email one; otherwise the first they hold.
    const principals = (Array.isArray(command.principals) ? command.principals : [])
      .map((p) => parsePrincipal(p))
      .filter(Boolean)
      .map((p) => p.principal);
    const args = command.args || {};
    const idem = command.idempotency || null;
    const now = this.now();
    const actor = this.actingAs(principals);
    const ctx = { op, slug, actor, principals, args, now };
    if (this.org()) for (const tick of EXTENSIONS.ticks) tick.call(this, ctx);

    // The door's pre-check before storing bytes replays too, so a retried
    // upload is answered without storing anything twice.
    if ((MUTATIONS.has(op) || op === "check-candidate") && actor && idem && idem.key) {
      const prior = this.row(
        "SELECT request_hash, status, body FROM idempotency WHERE actor = ? AND key = ?",
        actor,
        String(idem.key),
      );
      if (prior) {
        if (prior.request_hash !== idem.hash) {
          return this.deny(ctx, `${op}`, null, "idempotency-key-reused");
        }
        return { status: prior.status, body: prior.body ? JSON.parse(prior.body) : null, replayed: true };
      }
    }

    const handler = this[`op_${op.replace(/-/g, "_")}`];
    if (typeof handler !== "function") {
      return { status: 400, body: { error: "bad-command", message: `no such command: ${op}` } };
    }
    if (this.org()) {
      for (const gate of EXTENSIONS.gates) {
        const refusal = gate.call(this, ctx);
        if (refusal) return refusal;
      }
    }
    const answer = handler.call(this, ctx);
    if (MUTATIONS.has(op) && actor && idem && idem.key && answer.status >= 200 && answer.status < 300) {
      // Only a success is remembered: a refusal retried after its cause is
      // fixed should be able to succeed under the same key.
      this.sql.exec(
        "INSERT INTO idempotency (actor, key, request_hash, status, body, at) VALUES (?, ?, ?, ?, ?, ?)",
        actor,
        String(idem.key),
        String(idem.hash || ""),
        answer.status,
        answer.body == null ? null : JSON.stringify(answer.body),
        now,
      );
    }
    return answer;
  }

  // ------------------------------------------------------------- helpers

  org() {
    return this.row("SELECT slug, name, created_at, created_by FROM org WHERE id = 1");
  }

  rootKey() {
    const r = this.row("SELECT public_key FROM root_keys ORDER BY seq DESC LIMIT 1");
    return r ? r.public_key : null;
  }

  member(principal) {
    if (!principal) return null;
    return this.row("SELECT * FROM members WHERE principal = ?", principal);
  }

  /// Which of `principals` the person acts as in this organisation.
  actingAs(principals) {
    if (!principals.length) return null;
    const rank = (m) => (m.state === "active" ? 0 : 2) + (m.principal.startsWith("github:") ? 0 : 1);
    const held = principals
      .map((p) => this.member(p))
      .filter(Boolean)
      .sort((a, b) => rank(a) - rank(b));
    return held.length ? held[0].principal : principals[0];
  }

  activeOwners() {
    return this.row("SELECT COUNT(*) AS n FROM members WHERE role = 'owner' AND state = 'active'").n;
  }

  /// Append one audit event, chained to the last (R11: never app bytes,
  /// source, tokens, keys' private halves or app data -- only who, what,
  /// which target, and the outcome).
  audit(actor, action, target, result, reason = null) {
    const last = this.row("SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1");
    const event = {
      seq: last ? last.seq + 1 : 1,
      at: this.now(),
      actor: actor || null,
      action,
      target: target || null,
      result,
      reason: reason || null,
      prev_hash: last ? last.hash : GENESIS,
    };
    event.hash = sha256HexSync(eventPreimage(event));
    this.sql.exec(
      "INSERT INTO audit (seq, at, actor, action, target, result, reason, prev_hash, hash) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
      event.seq,
      event.at,
      event.actor,
      event.action,
      event.target,
      event.result,
      event.reason,
      event.prev_hash,
      event.hash,
    );
    return event;
  }

  /// A refusal: audited, then answered with the stable reason.
  deny(ctx, action, target, reason, detail) {
    // Only an authenticated principal's denials are audited: the door
    // answers requests with no session itself, and if one reaches here
    // anyway it must not be able to grow the audit.
    // A quiet check (a live socket's standing, org-data-live.js) asks the
    // same question without being an attempt by anyone: not audited.
    if (ctx.actor && !ctx.quiet && this.org()) this.audit(ctx.actor, action, target, "denied", reason);
    return {
      status: STATUS[reason] || 403,
      body: { error: reason, message: messageFor(reason, ctx.slug, detail) },
    };
  }

  /// What the modules' gates answer for `ctx`, as a command would meet
  /// them: the first refusal, or null.
  gateRefusal(ctx) {
    for (const gate of EXTENSIONS.gates) {
      const refusal = gate.call(this, ctx);
      if (refusal) return refusal;
    }
    return null;
  }

  ok(body, status = 200) {
    return { status, body };
  }

  /// The actor's current membership, or the refusal that applies. Every
  /// command but `create` and `accept` starts here: no session, no such
  /// organisation, not a member, removed.
  standing(ctx, action, target) {
    if (!this.org()) {
      return {
        refusal: { status: 404, body: { error: "no-such-org", message: messageFor("no-such-org", ctx.slug) } },
      };
    }
    if (!ctx.actor) return { refusal: this.deny(ctx, action, target, "not-signed-in") };
    const me = this.member(ctx.actor);
    if (!me) return { refusal: this.deny(ctx, action, target, "not-a-member") };
    if (me.state !== "active") {
      // A removed member already knows which organisation this is (R8).
      const refusal = this.deny(ctx, action, target, "removed");
      refusal.body.org = ctx.slug;
      refusal.body.org_name = this.org().name;
      return { refusal };
    }
    return { me };
  }

  membership(m) {
    if (!m) return null;
    const p = parsePrincipal(m.principal);
    const out = {
      org: this.org().slug,
      principal: m.principal,
      display: p ? p.value : m.principal,
      kind: p ? p.kind : null,
      role: m.role,
      state: m.state,
      since: m.added_at,
      added_by: m.added_by,
    };
    if (m.state === "removed") {
      out.removed_at = m.removed_at;
      out.removed_by = m.removed_by;
    }
    return out;
  }

  orgView() {
    const o = this.org();
    const key = this.row("SELECT public_key, set_at, set_by FROM root_keys ORDER BY seq DESC LIMIT 1");
    return {
      slug: o.slug,
      name: o.name,
      created_at: o.created_at,
      created_by: o.created_by,
      root_key: key ? key.public_key : null,
      // When and by whom the current root was set (Addendum 2, item 3).
      root_key_set_at: key ? key.set_at : null,
      root_key_set_by: key ? key.set_by : null,
    };
  }

  // ------------------------------------------------------------ commands

  op_create(ctx) {
    if (!ctx.actor) return { status: 401, body: { error: "not-signed-in", message: messageFor("not-signed-in") } };
    if (this.org()) return this.deny(ctx, "org.create", `org:${ctx.slug}`, "slug-taken");
    const name = String(ctx.args.name || ctx.slug).slice(0, 80);
    this.sql.exec(
      "INSERT INTO org (id, slug, name, created_at, created_by) VALUES (1, ?, ?, ?, ?)",
      ctx.slug,
      name,
      ctx.now,
      ctx.actor,
    );
    this.sql.exec(
      "INSERT INTO members (principal, role, state, added_at, added_by) VALUES (?, 'owner', 'active', ?, ?)",
      ctx.actor,
      ctx.now,
      ctx.actor,
    );
    this.audit(ctx.actor, "org.create", `org:${ctx.slug}`, "allowed");
    this.audit(ctx.actor, "member.add", `member:${ctx.actor}`, "allowed", "owner");
    return this.ok({ org: this.orgView(), membership: this.membership(this.member(ctx.actor)) }, 201);
  }

  /// GET /orgs/<slug>/available: whether the name is free. No audit: a
  /// person typing a name is not an attempt on anyone's organisation.
  op_exists(ctx) {
    return this.ok({ slug: ctx.slug, available: !this.org() });
  }

  /// GET /orgs/<slug>/invites/<code>/door, signed out: for an OPEN
  /// invitation only: the organisation's name (the code in the link is
  /// the proof that this person was sent it), whether to offer the
  /// company's sign-in first, and
  /// which kind of account it names. Anything else is the same 404, so a
  /// guessed code learns nothing.
  op_invite_door(ctx) {
    const none = { status: 404, body: { error: "invite-revoked", message: messageFor("invite-revoked", ctx.slug) } };
    if (!this.org()) return none;
    const invite = this.row("SELECT * FROM invites WHERE code = ?", String(ctx.args.code || ""));
    if (!invite || invite.state !== "open" || ctx.now >= invite.expires_at) return none;
    let company = false;
    try {
      company = Boolean(this.row("SELECT 1 AS x FROM sso WHERE id = 1") || this.row("SELECT 1 AS x FROM saml WHERE id = 1"));
    } catch (_) {}
    const p = parsePrincipal(invite.principal);
    return this.ok({ name: this.org().name, company_sign_in: company, kind: p ? p.kind : null });
  }

  op_show(ctx) {
    const { me, refusal } = this.standing(ctx, "org.view", `org:${ctx.slug}`);
    if (refusal) return refusal;
    const rule = allows(me.role, "org.view");
    const members =
      rule === "yes"
        ? this.rows("SELECT * FROM members ORDER BY added_at, rowid").map((m) => this.membership(m))
        : [this.membership(me)];
    const org = this.orgView();
    for (const view of EXTENSIONS.views) view.call(this, org, me);
    return this.ok({ org, me: this.membership(me), members });
  }

  inviteView(invite, site) {
    const p = parsePrincipal(invite.principal);
    return {
      code: invite.code,
      expires_at: invite.expires_at,
      principal: invite.principal,
      display: p ? p.value : invite.principal,
      kind: p ? p.kind : null,
      role: invite.role,
      join_url: `${site || "https://krate.tech"}/join/?o=${encodeURIComponent(this.org().slug)}&c=${encodeURIComponent(invite.code)}`,
    };
  }

  op_invite(ctx) {
    // `{github, role}` or `{email, role}`: exactly one of the two (IC-947).
    const github = ctx.args.github;
    const email = ctx.args.email;
    const role = String(ctx.args.role || "");
    const named = (github !== undefined && github !== null && github !== "") + (email !== undefined && email !== null && email !== "");
    const p = named !== 1 ? null : github ? parsePrincipal(`github:${github}`) : parsePrincipal(`email:${email}`);
    const target = `invite:${p ? p.principal : "unknown"}/${role}`;
    const { me, refusal } = this.standing(ctx, "invite.create", target);
    if (refusal) return refusal;
    if (!ROLES.includes(role)) {
      return this.deny(ctx, "invite.create", target, "role-cannot", `there is no role called ${JSON.stringify(role)}`);
    }
    if (!p) {
      return this.deny(
        ctx,
        "invite.create",
        target,
        "role-cannot",
        named !== 1
          ? "an invitation names exactly one of a GitHub username (github) or an email address (email)"
          : github
            ? "that is not a GitHub username"
            : "that is not an email address",
      );
    }
    const action = role === "owner" || role === "admin" ? "invite.owner-admin" : "invite.other";
    if (allows(me.role, action) !== "yes") {
      return this.deny(ctx, "invite.create", target, "role-cannot", `a ${me.role} cannot invite a ${role}`);
    }
    const code = inviteCode();
    const expires = ctx.now + INVITE_SECONDS;
    this.sql.exec(
      "INSERT INTO invites (code, principal, role, issued_by, issued_at, expires_at, state) VALUES (?, ?, ?, ?, ?, ?, 'open')",
      code,
      p.principal,
      role,
      ctx.actor,
      ctx.now,
      expires,
    );
    this.audit(ctx.actor, "invite.create", target, "allowed");
    return this.ok(this.inviteView(this.row("SELECT * FROM invites WHERE code = ?", code), ctx.args.site), 201);
  }

  op_revoke_invite(ctx) {
    const code = String(ctx.args.code || "");
    const invite = this.row("SELECT * FROM invites WHERE code = ?", code);
    const target = invite ? `invite:${invite.principal}/${invite.role}` : "invite:unknown";
    const { me, refusal } = this.standing(ctx, "invite.revoke", target);
    if (refusal) return refusal;
    if (!invite) {
      return this.deny(ctx, "invite.revoke", target, "invite-revoked", `no invitation with that code in ${ctx.slug}`);
    }
    const action = invite.role === "owner" || invite.role === "admin" ? "invite.owner-admin" : "invite.other";
    if (allows(me.role, action) !== "yes") {
      return this.deny(ctx, "invite.revoke", target, "role-cannot", `a ${me.role} cannot withdraw an invitation for a ${invite.role}`);
    }
    if (invite.state === "used") return this.deny(ctx, "invite.revoke", target, "invite-used");
    if (invite.state === "open") {
      this.sql.exec(
        "UPDATE invites SET state = 'revoked', closed_at = ?, closed_by = ? WHERE code = ?",
        ctx.now,
        ctx.actor,
        code,
      );
      this.audit(ctx.actor, "invite.revoke", target, "allowed");
    }
    return this.ok(null, 204);
  }

  /// GET /orgs/<slug>/invites/<code> (Addendum 2, item 1): what the join
  /// page shows before Accept -- the organisation, the role, who invited
  /// you and who it is for. Only to the principal it names; anyone else
  /// hears `invite-for-someone-else` and nothing beyond the slug. Reading
  /// it changes nothing and is never audited, refusals included.
  op_invite_info(ctx) {
    const code = String(ctx.args.code || "");
    const refuse = (reason, detail) => ({
      status: STATUS[reason] || 403,
      body: { error: reason, message: messageFor(reason, ctx.slug, detail) },
    });
    if (!this.org()) return refuse("no-such-org");
    if (!ctx.actor) return refuse("not-signed-in");
    const invite = this.row("SELECT * FROM invites WHERE code = ?", code);
    if (!invite) return refuse("invite-revoked", "no invitation with that code -- check the link, or ask for a new one");
    if (!ctx.principals.includes(invite.principal)) return refuse("invite-for-someone-else");
    if (invite.state === "revoked") return refuse("invite-revoked");
    if (invite.state === "used") return refuse("invite-used");
    if (ctx.now >= invite.expires_at) return refuse("invite-expired");
    const p = parsePrincipal(invite.principal);
    return this.ok({
      org: { slug: this.org().slug, name: this.org().name },
      role: invite.role,
      invited_by: { principal: invite.issued_by, display: displayOf(invite.issued_by) },
      for: { principal: invite.principal, display: p ? p.value : invite.principal, kind: p ? p.kind : null },
      expires_at: invite.expires_at,
      state: "open",
      join_url: this.inviteView(invite, ctx.args.site).join_url,
    });
  }

  op_accept(ctx) {
    const code = String(ctx.args.code || "");
    if (!this.org()) {
      return { status: 404, body: { error: "no-such-org", message: messageFor("no-such-org", ctx.slug) } };
    }
    const invite = this.row("SELECT * FROM invites WHERE code = ?", code);
    const target = invite ? `invite:${invite.principal}/${invite.role}` : "invite:unknown";
    if (!ctx.actor) return this.deny(ctx, "invite.accept", target, "not-signed-in");
    if (!invite) {
      return this.deny(ctx, "invite.accept", target, "invite-revoked", `no invitation with that code in ${ctx.slug} -- check the code, or ask for a new one`);
    }
    // Who it is for is asked first, so a person it does not name learns
    // nothing about its state. The person accepts as the principal the
    // invitation names: an email invitation with any sign-in that proved
    // that inbox, a GitHub one only with GitHub.
    if (!ctx.principals.includes(invite.principal)) {
      return this.deny(ctx, "invite.accept", target, "invite-for-someone-else");
    }
    const as = invite.principal;
    ctx = { ...ctx, actor: as };
    if (invite.state === "revoked") return this.deny(ctx, "invite.accept", target, "invite-revoked");
    if (invite.state === "used") return this.deny(ctx, "invite.accept", target, "invite-used");
    if (ctx.now >= invite.expires_at) return this.deny(ctx, "invite.accept", target, "invite-expired");

    const current = this.member(as);
    if (
      current &&
      current.state === "active" &&
      current.role === "owner" &&
      invite.role !== "owner" &&
      this.activeOwners() <= 1
    ) {
      return this.deny(ctx, "invite.accept", target, "last-owner");
    }
    this.sql.exec(
      "UPDATE invites SET state = 'used', closed_at = ?, closed_by = ? WHERE code = ?",
      ctx.now,
      as,
      code,
    );
    if (current) {
      this.sql.exec(
        "UPDATE members SET role = ?, state = 'active', added_at = ?, added_by = ?, removed_at = NULL, removed_by = NULL WHERE principal = ?",
        invite.role,
        ctx.now,
        invite.issued_by,
        as,
      );
    } else {
      this.sql.exec(
        "INSERT INTO members (principal, role, state, added_at, added_by) VALUES (?, ?, 'active', ?, ?)",
        as,
        invite.role,
        ctx.now,
        invite.issued_by,
      );
    }
    this.audit(as, "invite.accept", target, "allowed");
    return this.ok({ membership: this.membership(this.member(as)), org: { slug: this.org().slug, name: this.org().name } });
  }

  /// The member a URL names, as a principal: `github%3Aalice`. A bare
  /// login is not a principal and names nobody.
  targetOf(ctx) {
    const p = parsePrincipal(ctx.args.principal);
    return p ? p.principal : null;
  }

  op_set_role(ctx) {
    const who = this.targetOf(ctx);
    const role = String(ctx.args.role || "");
    const target = `member:${who || String(ctx.args.principal || "").slice(0, 80)}`;
    const { me, refusal } = this.standing(ctx, "member.role", target);
    if (refusal) return refusal;
    if (!ROLES.includes(role)) {
      return this.deny(ctx, "member.role", target, "role-cannot", `there is no role called ${JSON.stringify(role)}`);
    }
    const them = this.member(who);
    if (!them || them.state !== "active") {
      return this.deny(ctx, "member.role", target, "not-a-member", notAMember(who, ctx));
    }
    // Making or unmaking an owner is an owner's; so is anything touching an
    // admin, because only an owner can invite one. Everything else between
    // developer, reviewer and recipient is an owner's or an admin's.
    const high = (r) => r === "owner" || r === "admin";
    const action = them.role === "owner" || role === "owner" ? "role.owner" : high(them.role) || high(role) ? "invite.owner-admin" : "role.change";
    if (allows(me.role, action) !== "yes") {
      return this.deny(ctx, "member.role", target, "role-cannot", `a ${me.role} cannot change a ${them.role} to ${role}`);
    }
    if (them.role === "owner" && role !== "owner" && this.activeOwners() <= 1) {
      return this.deny(ctx, "member.role", target, "last-owner");
    }
    if (them.role !== role) {
      this.sql.exec("UPDATE members SET role = ? WHERE principal = ?", role, who);
      this.audit(ctx.actor, "member.role", target, "allowed", `${them.role}->${role}`);
    }
    return this.ok({ membership: this.membership(this.member(who)), org: { slug: this.org().slug, name: this.org().name } });
  }

  op_remove(ctx) {
    const who = this.targetOf(ctx);
    const target = `member:${who || String(ctx.args.principal || "").slice(0, 80)}`;
    const { me, refusal } = this.standing(ctx, "member.remove", target);
    if (refusal) return refusal;
    const them = this.member(who);
    if (!them || them.state !== "active") {
      return this.deny(ctx, "member.remove", target, "not-a-member", notAMember(who, ctx));
    }
    const rule = allows(me.role, "member.remove");
    const self = who === me.principal;
    const permitted = rule === "yes" || (rule === "self" && self) || (rule === "not-owners" && (them.role !== "owner" || self));
    if (!permitted) {
      return this.deny(ctx, "member.remove", target, "role-cannot", `a ${me.role} cannot remove a ${them.role}`);
    }
    if (them.role === "owner" && this.activeOwners() <= 1) {
      return this.deny(ctx, "member.remove", target, "last-owner");
    }
    // The member's row stays, marked removed: their uploads and approvals
    // keep naming them (R12), and their next fetch is refused as `removed`.
    this.sql.exec(
      "UPDATE members SET state = 'removed', removed_at = ?, removed_by = ? WHERE principal = ?",
      ctx.now,
      ctx.actor,
      who,
    );
    this.audit(ctx.actor, "member.remove", target, "allowed", them.role);
    // Invitations they issued that nobody has accepted go with their
    // authority (ch 14 7.3: removal ends pending authority).
    for (const invite of this.rows("SELECT code, principal, role FROM invites WHERE issued_by = ? AND state = 'open'", who)) {
      this.sql.exec(
        "UPDATE invites SET state = 'revoked', closed_at = ?, closed_by = ? WHERE code = ?",
        ctx.now,
        ctx.actor,
        invite.code,
      );
      this.audit(ctx.actor, "invite.revoke", `invite:${invite.principal}/${invite.role}`, "allowed", "issuer removed");
    }
    return this.ok(null, 204);
  }

  op_set_root(ctx) {
    const key = String(ctx.args.public_key || "").toLowerCase();
    const { me, refusal } = this.standing(ctx, "root-key.set", "root-key");
    if (refusal) return refusal;
    if (allows(me.role, "root-key.set") !== "yes") {
      return this.deny(ctx, "root-key.set", "root-key", "role-cannot", `only an owner registers ${ctx.slug}'s root key`);
    }
    if (!/^[0-9a-f]{64}$/.test(key)) {
      return this.deny(ctx, "root-key.set", "root-key", "role-cannot", "a root key is a 64-hex Ed25519 public key (the publisher root `krate delegate` uses)");
    }
    const previous = this.rootKey();
    if (previous !== key) {
      const seq = (this.row("SELECT MAX(seq) AS n FROM root_keys").n || 0) + 1;
      this.sql.exec(
        "INSERT INTO root_keys (seq, public_key, set_at, set_by) VALUES (?, ?, ?, ?)",
        seq,
        key,
        ctx.now,
        ctx.actor,
      );
      this.audit(ctx.actor, previous ? "root-key.rotate" : "root-key.register", `root-key:${key.slice(0, 16)}`, "allowed");
    }
    // Withdrawn release keys, from a list the root signed (checked by the
    // validator before it reached here). Never un-withdrawn: a later list
    // that leaves a key out does not bring it back.
    const revocations = Array.isArray(ctx.args.revocations) ? ctx.args.revocations : [];
    let added = 0;
    for (const r of revocations) {
      const k = String(r.key || "").toLowerCase();
      if (!/^[0-9a-f]{64}$/.test(k)) continue;
      if (this.row("SELECT key FROM revoked_keys WHERE key = ?", k)) continue;
      this.sql.exec(
        "INSERT INTO revoked_keys (key, compromised_from, reason, root, recorded_at, recorded_by) VALUES (?, ?, ?, ?, ?, ?)",
        k,
        Number(r.compromised_from) || 0,
        String(r.reason || "").slice(0, 200),
        key,
        ctx.now,
        ctx.actor,
      );
      this.audit(ctx.actor, "release-key.revoke", `release-key:${k.slice(0, 16)}`, "allowed");
      added += 1;
    }
    const top = this.row("SELECT * FROM root_keys ORDER BY seq DESC LIMIT 1");
    return this.ok({
      key: {
        public_key: top.public_key,
        set_at: top.set_at,
        set_by: top.set_by,
        previous: previous && previous !== top.public_key ? previous : null,
        revoked_keys: this.row("SELECT COUNT(*) AS n FROM revoked_keys").n,
        revoked_now: added,
      },
    });
  }

  revokedKeys() {
    return new Set(this.rows("SELECT key FROM revoked_keys").map((r) => r.key));
  }

  /// May the actor publish a candidate with this signature? The check the
  /// door asks before it stores any bytes; `add-candidate` asks it again
  /// inside the transaction that records the candidate.
  publishRefusal(ctx, target) {
    const { me, refusal } = this.standing(ctx, "candidate.publish", target);
    if (refusal) return refusal;
    if (allows(me.role, "candidate.publish") !== "yes") {
      return this.deny(ctx, "candidate.publish", target, "role-cannot", `a ${me.role} cannot publish to ${ctx.slug}`);
    }
    if (ctx.args.signature !== undefined) {
      const why = notSignedForOrg(ctx.args.signature, this.rootKey(), this.revokedKeys(), ctx.now);
      if (why) return this.deny(ctx, "candidate.publish", target, "not-signed-for-this-org", why);
      // Addendum 5: a candidate carries its organisation's mark, and the
      // signature just found valid covers it (the signed statement covers
      // every entry). Asked only once the bundle has been judged.
      if (ctx.args.mark !== ctx.slug) {
        const detail = ctx.args.mark
          ? `this app is marked as ${ctx.args.mark}'s private app, not ${ctx.slug}'s`
          : undefined;
        return this.deny(ctx, "candidate.publish", target, "not-marked-for-this-org", detail);
      }
    }
    return null;
  }

  op_check_candidate(ctx) {
    const digest = String(ctx.args.digest || "");
    const refusal = this.publishRefusal(ctx, `candidate:${digest}`);
    if (refusal) return refusal;
    return this.ok({ ok: true });
  }

  op_add_candidate(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const refusal = this.publishRefusal(ctx, target);
    if (refusal) return refusal;
    const existing = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (existing) {
      // The same bytes again: the candidate they already are, uploader
      // unchanged (approval is bound to these bytes, not to who sent them).
      return this.ok({ digest, state: existing.state, name: existing.name, version: existing.version });
    }
    this.sql.exec(
      `INSERT INTO candidates (digest, name, version, namespace, size, signer, big, uploaded_by, uploaded_at, state)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'waiting-for-approval')`,
      digest,
      String(ctx.args.name || "").slice(0, 80),
      String(ctx.args.version || "").slice(0, 40),
      String(ctx.args.namespace || "").slice(0, 120),
      Number(ctx.args.size) || 0,
      String((ctx.args.signature && ctx.args.signature.public_key) || ""),
      ctx.args.big ? 1 : 0,
      ctx.actor,
      ctx.now,
    );
    this.audit(ctx.actor, "candidate.publish", target, "allowed");
    return this.ok({ digest, state: "waiting-for-approval", name: ctx.args.name || "", version: ctx.args.version || "" }, 201);
  }

  op_approve(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const { me, refusal } = this.standing(ctx, "candidate.approve", target);
    if (refusal) return refusal;
    // The exact digest, or nothing: approving X never touches Y.
    const candidate = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    // Separation first: whoever uploaded it hears that someone else must
    // approve it, whatever their role, because that is the next step.
    // Any principal the approver holds counts: one person signed in with
    // both GitHub and a proved inbox is still one person.
    if (candidate && ctx.principals.includes(candidate.uploaded_by)) {
      return this.deny(ctx, "candidate.approve", target, "self-approval");
    }
    if (allows(me.role, "candidate.approve") !== "yes") {
      return this.deny(ctx, "candidate.approve", target, "role-cannot", `a ${me.role} cannot approve releases in ${ctx.slug}`);
    }
    if (!candidate) return this.deny(ctx, "candidate.approve", target, "digest-mismatch");
    // A key withdrawn after the upload withdraws what it signed and nobody
    // has released yet.
    if (candidate.state !== "released" && candidate.signer && this.revokedKeys().has(candidate.signer)) {
      return this.deny(ctx, "candidate.approve", target, "not-signed-for-this-org", "its release key was withdrawn by the organisation's root after it was uploaded");
    }
    if (candidate.state !== "released") {
      this.sql.exec(
        "UPDATE candidates SET state = 'released', approved_by = ?, approved_at = ? WHERE digest = ?",
        ctx.actor,
        ctx.now,
        digest,
      );
      this.audit(ctx.actor, "candidate.approve", target, "allowed", ctx.approveReason || null);
    }
    return this.ok({ release: { digest, url: `${ctx.args.base || ""}/a/${digest}` } });
  }

  op_releases(ctx) {
    const { me, refusal } = this.standing(ctx, "release.list", `org:${ctx.slug}`);
    if (refusal) return refusal;
    if (allows(me.role, "release.download") !== "yes") {
      return this.deny(ctx, "release.list", `org:${ctx.slug}`, "role-cannot");
    }
    const base = ctx.args.base || "";
    const releases = this.rows(
      "SELECT * FROM candidates WHERE state = 'released' ORDER BY approved_at DESC, digest",
    ).map((c) => ({
      name: c.name,
      version: c.version,
      digest: c.digest,
      url: `${base}/a/${c.digest}`,
      uploaded_by: c.uploaded_by,
      approved_by: c.approved_by,
      at: c.approved_at,
    }));
    return this.ok(releases);
  }

  /// Every candidate, released or waiting, with who sent and who approved
  /// it -- for owners and admins, and for the tests that prove history
  /// survives a removal.
  op_candidates(ctx) {
    const { me, refusal } = this.standing(ctx, "candidate.list", `org:${ctx.slug}`);
    if (refusal) return refusal;
    if (allows(me.role, "audit.read") !== "yes" && allows(me.role, "candidate.publish") !== "yes" && allows(me.role, "candidate.approve") !== "yes") {
      return this.deny(ctx, "candidate.list", `org:${ctx.slug}`, "role-cannot");
    }
    return this.ok(
      this.rows("SELECT * FROM candidates ORDER BY uploaded_at, digest").map((c) => ({
        digest: c.digest,
        name: c.name,
        version: c.version,
        state: c.state,
        uploaded_by: c.uploaded_by,
        uploaded_at: c.uploaded_at,
        approved_by: c.approved_by,
        approved_at: c.approved_at,
      })),
    );
  }

  /// The check behind every fetch of a private release (R1, R8). Read from
  /// current state, every time: no session, not a member, removed, and a
  /// digest that is only a candidate are each refused.
  op_download(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `release:${digest}`;
    const { me, refusal } = this.standing(ctx, "release.download", target);
    if (refusal) return refusal;
    if (allows(me.role, "release.download") !== "yes") {
      return this.deny(ctx, "release.download", target, "role-cannot");
    }
    const candidate = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (!candidate || candidate.state !== "released") {
      return this.deny(
        ctx,
        "release.download",
        target,
        "digest-mismatch",
        candidate ? "that digest is a candidate waiting for approval, not a release" : `no release with that digest in ${ctx.slug}`,
      );
    }
    this.audit(ctx.actor, "release.download", target, "allowed");
    return this.ok({ name: candidate.name, version: candidate.version, big: Boolean(candidate.big), org_name: this.org().name });
  }

  /// What the site's open page may show about a private release before
  /// anything is downloaded (Addendum 2, item 6): the checks a download
  /// makes, refused the same way and audited the same way; allowed, it is
  /// a look, not a download, and records nothing.
  op_view(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `release:${digest}`;
    const { me, refusal } = this.standing(ctx, "release.download", target);
    if (refusal) return refusal;
    if (allows(me.role, "release.download") !== "yes") {
      return this.deny(ctx, "release.download", target, "role-cannot");
    }
    const candidate = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (!candidate || candidate.state !== "released") {
      return this.deny(ctx, "release.download", target, "digest-mismatch", "that digest is not a release");
    }
    return this.ok({ name: candidate.name, version: candidate.version, org_name: this.org().name });
  }

  op_audit(ctx) {
    const { me, refusal } = this.standing(ctx, "audit.read", `org:${ctx.slug}`);
    if (refusal) return refusal;
    if (allows(me.role, "audit.read") !== "yes") {
      return this.deny(ctx, "audit.read", `org:${ctx.slug}`, "role-cannot", `a ${me.role} cannot read ${ctx.slug}'s audit`);
    }
    return this.ok({ events: this.rows("SELECT * FROM audit ORDER BY seq") });
  }
}
