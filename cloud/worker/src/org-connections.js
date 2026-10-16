// Company connections (IC-952, ORG-FIRST-PLAN.md "IC-952 contract").
//
// An owner or admin defines a connection once: a name (`finance-api`), the
// base it covers (an https address: host and path prefix), the credential
// (one header, or a user and password sent as Basic), what an app may do
// with it (methods and path prefixes; GET only unless they say otherwise),
// who may use it (everyone, or groups) and which apps (any of the
// organisation's, or named app ids).
//
// The credential is end-to-end encrypted (founder decision F-316): the
// admin's browser or the CLI seals it to the organisation's public key
// (HPKE, crates/orgkeys `seal_for_org`, for a context that binds the
// organisation, the connection's name, its header and its address), and
// this hub keeps the sealed blob as it came, with the key version. Only a
// member's device, holding the organisation key, opens it. The hub cannot
// read it, so it cannot check the value either: the page and the CLI do,
// before sealing, and the device again after opening. It is write-only and
// replaced, never edited. Moving a connection to another base needs a new
// credential: the old one is sealed for the old address and would not open,
// and an admin who never knew the secret must not be able to send it to a
// server of their own.
//
// When the organisation key is replaced after a removal, the member device
// that makes the new version also seals every credential again to it
// (org-keys.js rotate, extended below), and the hub refuses a rotation that
// would leave one on the old version.
//
// A device fetches the credential when an app that asks for
// `net.connection:<name>` opens (GET .../connections/<name>/credential/
// <digest>). That fetch is checked the way a lease is: the person's
// standing, the release (not withdrawn, inside its audience), then the
// connection's own audience, its app list, and that the release itself
// asked for the connection. The runtime keeps the credential in memory for
// that one run and adds it to the app's requests to the base; the app never
// sees it. Each use is counted per day and the count goes in the audit as
// one line a day.
//
// What this cannot do, and the docs say: the request goes from the
// person's device, so the credential is on that device, in Krate's memory,
// for the run. Someone who controls their own machine can read their own
// memory. The guards here decide who gets it at all.

import { Org, extendOrg, addReasons, allows } from "./org.js";
// Release control first: a credential fetch reuses its audience and
// withdrawn checks, and this module's order is its own, not index.js's.
import { GROUP, appOf } from "./org-releases.js";
// The keyring first too: its rotation and its view are extended here.
import "./org-keys.js";
// Days are the organisation's own (org-time.js, F-319).
import { dayBefore } from "./org-time.js";
// Telling the API who is calling: the token, signed by the Worker.
import { issueIdentity, IDENTITY_HEADER } from "./org-identity-token.js";

export const CONNECTION = /^[a-z][a-z0-9-]{1,38}$/;
export const METHODS = ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"];
const APP_ID = /^[a-z0-9][a-z0-9._-]{0,119}$/;
const MAX_PATHS = 50;
const MAX_APPS = 100;
/// Headers the transport owns. A credential in one would be rewritten or
/// refused on the way out, so it is refused here, when it can be fixed.
const TRANSPORT_HEADERS = new Set([
  "host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer", "keep-alive", "expect",
]);
const HEADER_NAME = /^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,64}$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;
/// A sealed credential: base64 of HPKE's 32-byte encapsulated key and the
/// ciphertext with its 16-byte tag; the value inside is at most 4 KiB.
const SEALED_MIN = 68;
const SEALED_MAX = 8192;
const DAY = 24 * 60 * 60;

const goodSealed = (s) => typeof s === "string" && s.length >= SEALED_MIN && s.length <= SEALED_MAX && s.length % 4 === 0 && B64.test(s);

addReasons({
  "no-such-connection": {
    status: 404,
    message: (o, d) => d || `${o} has no connection by that name -- an owner or admin of ${o} sets one up in Settings`,
  },
  "connection-taken": { status: 409, message: (o, d) => d || `${o} already has a connection by that name` },
  "connection-invalid": { status: 400, message: (o, d) => d || "that connection is not complete" },
  "credential-required": {
    status: 400,
    message: () =>
      "moving a connection to another address needs its credential again: the stored one is never sent anywhere new",
  },
  "connection-not-in-audience": {
    status: 403,
    message: (o, d) => d || `this connection is only for some groups in ${o} -- ask an admin of ${o} to add you to one`,
  },
  "app-not-allowed": {
    status: 403,
    message: (o, d) => d || `${o} has not let this app use that connection -- an owner or admin of ${o} can add it`,
  },
  "connection-not-requested": {
    status: 403,
    message: (o, d) => d || "this version of the app did not ask for that connection, so it does not get it",
  },
  "credential-needs-entry": {
    status: 409,
    message: (o, d) =>
      d || `that connection's credential must be entered again by an owner or admin of ${o}: it was kept the old way, which Krate's hub could read`,
  },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS connections (
     name TEXT PRIMARY KEY, base TEXT NOT NULL,
     methods TEXT NOT NULL, paths TEXT NOT NULL,
     audience TEXT, apps TEXT,
     kind TEXT NOT NULL, header_name TEXT NOT NULL, sealed TEXT NOT NULL,
     created_at INTEGER NOT NULL, created_by TEXT NOT NULL,
     changed_at INTEGER NOT NULL, changed_by TEXT NOT NULL,
     credential_at INTEGER NOT NULL, credential_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS connection_uses (
     name TEXT NOT NULL, day TEXT NOT NULL, n INTEGER NOT NULL,
     audited INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (name, day))`,
];

// Which connections each candidate asked for (its manifest's
// `net.connection:<name>` capabilities), kept when it is uploaded; and the
// version of the organisation key each credential is sealed to (F-316).
// NULL is a credential kept the old way, sealed with this hub's own key;
// 0 is one that must be entered again (the one-time migration below).
// `identity`: whether each request also tells the API who is calling (a
// signed Krate-Identity token, org-identity-token.js). NULL, as every
// connection made before the setting has, is on: the default.
const COLUMNS = [["candidates", "connections", "TEXT"], ["connections", "credential_v", "INTEGER"], ["connections", "identity", "INTEGER"]];

/// The setting as kept: on unless an admin turned it off.
const identityOn = (c) => c.identity !== 0;


const list = (text) => {
  if (!text) return null;
  try {
    const v = JSON.parse(text);
    return Array.isArray(v) ? v : null;
  } catch (_) {
    return [];
  }
};

/// "finance", "finance and ops", "finance, ops and legal".
function spoken(names) {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

// ------------------------------------------------------- what is valid

/// The hub is this machine's loopback: a local `wrangler dev` proof. Only
/// then may a connection's base be plain http, and only to this machine.
function devHub(env) {
  return /^http:\/\/(127\.0\.0\.1|localhost|\[::1\])(:\d+)?\/?$/.test(String((env && env.PUBLIC_BASE) || ""));
}

/// A connection's base as it is kept: `https://host[:port]/path`, the path
/// without a trailing slash (the root is ""). Answers the text, or null
/// with the words of why.
export function normalBase(raw, env) {
  let u;
  try {
    u = new URL(String(raw || "").trim());
  } catch (_) {
    return { problem: "the address is a full URL, such as https://api.acme.com/finance" };
  }
  if (u.username || u.password) return { problem: "the address cannot carry a user or password: put those in the credential" };
  if (u.search || u.hash || String(raw).includes("?") || String(raw).includes("#")) {
    return { problem: "the address is a host and a path, with no ? or # part" };
  }
  const loop = /^(127\.0\.0\.1|localhost|\[::1\])$/.test(u.hostname);
  if (u.protocol !== "https:" && !(u.protocol === "http:" && loop && devHub(env))) {
    return { problem: "the address must start with https:// so the credential is never sent in the clear" };
  }
  if (u.hostname.startsWith("[")) return { problem: "an IPv6 address cannot be a connection's address yet: use a host name" };
  const path = u.pathname.replace(/\/+$/, "");
  // Judged on what was typed too: the URL parser quietly folds `a/../b`
  // into `b`, and the address kept must be the one the admin read.
  const typed = String(raw).replace(/^[a-z]+:\/\/[^/]*/i, "");
  if (/%|\/\.\.?(\/|$)|\\/.test(path) || /%|\/\.\.?(\/|$)|\\/.test(typed)) {
    return { problem: "the address's path cannot hold %, . or .. parts" };
  }
  const port = u.port ? `:${u.port}` : "";
  const base = `${u.protocol}//${u.hostname.toLowerCase()}${port}${path}`;
  if (base.length > 300) return { problem: "the address is too long" };
  return { base };
}

/// `allow`: `{methods, paths}`, GET on everything under the base unless
/// they say otherwise.
export function normalAllow(raw) {
  const a = raw && typeof raw === "object" ? raw : {};
  const methods = a.methods === undefined || a.methods === null ? ["GET"] : a.methods;
  const paths = a.paths === undefined || a.paths === null ? ["/"] : a.paths;
  if (!Array.isArray(methods) || !methods.length) return { problem: "name at least one method, such as GET" };
  const m = [...new Set(methods.map((x) => String(x || "").trim().toUpperCase()))];
  const badMethod = m.find((x) => !METHODS.includes(x));
  if (badMethod !== undefined) return { problem: `a method is one of ${METHODS.join(", ")}` };
  if (!Array.isArray(paths) || !paths.length || paths.length > MAX_PATHS) {
    return { problem: `name 1 to ${MAX_PATHS} paths under the address, such as /invoices, or / for all of it` };
  }
  const p = [];
  for (const raw of paths) {
    let x = String(raw || "").trim();
    if (!x.startsWith("/")) x = `/${x}`;
    if (x.length > 1) x = x.replace(/\/+$/, "") || "/";
    if (x.length > 200 || /[%?#\\\s]|\/\.\.?(\/|$)/.test(x)) {
      return { problem: `the path ${JSON.stringify(String(raw).slice(0, 40))} is not one Krate can match exactly: no %, ?, #, spaces, . or .. parts` };
    }
    if (!p.includes(x)) p.push(x);
  }
  return { allow: { methods: METHODS.filter((x) => m.includes(x)), paths: p.sort() } };
}

/// The sealed credential a create or replace carried: `{credential: {kind,
/// header, v, sealed}}`, sealed where it was typed. Answers `{kind,
/// header_name, v, sealed}`, `{problem}`, or null when it carried none. The
/// header's name is in the clear (the dashboard shows it, and the seal binds
/// it); its value is inside the seal, where this hub cannot look -- so a
/// request carrying a value in the clear is refused, never kept.
export function sealedCredentialOf(v) {
  if (v && (v.header !== undefined || v.basic !== undefined)) {
    return {
      problem:
        "this hub keeps a credential only sealed to the organisation's key where it is typed, so it never sees the value: reload the dashboard, or update Krate (krate.tech/install)",
    };
  }
  const c = v && v.credential;
  if (c === undefined || c === null) return null;
  if (typeof c !== "object") return { problem: "the credential is a sealed blob with its key version" };
  const kind = c.kind === "basic" ? "basic" : c.kind === "header" ? "header" : null;
  if (!kind) return { problem: "the credential is one header, or a user and password" };
  const name = kind === "basic" ? "Authorization" : String(c.header || "").trim();
  if (!HEADER_NAME.test(name)) return { problem: "the header name is one word, such as Authorization or X-Api-Key" };
  if (TRANSPORT_HEADERS.has(name.toLowerCase()) || name.toLowerCase().startsWith("proxy-") || name.toLowerCase() === IDENTITY_HEADER.toLowerCase()) {
    return { problem: `${name} is a header the connection itself sets, so it cannot carry a credential` };
  }
  if (!Number.isInteger(c.v) || c.v < 1) return { problem: "the credential says which version of the organisation's key it is sealed to" };
  if (!goodSealed(c.sealed)) return { problem: "the sealed credential is base64, up to 8 KiB" };
  return { kind, header_name: name, v: c.v, sealed: c.sealed };
}

// ------------------------------------------------------------ the store

const ops = {
  connectionRow(name) {
    return this.row("SELECT * FROM connections WHERE name = ?", name);
  },

  /// What anyone who may see connections reads: everything but the
  /// credential, which is only ever "set, by whom, when".
  connectionView(c, now) {
    const since = dayBefore(this.orgDay(now), 29);
    const today = this.orgDay(now);
    const uses = this.rows("SELECT day, n FROM connection_uses WHERE name = ? AND day >= ?", c.name, since);
    return {
      name: c.name,
      base: c.base,
      allow: { methods: JSON.parse(c.methods), paths: JSON.parse(c.paths) },
      audience: list(c.audience),
      apps: list(c.apps),
      identity: identityOn(c),
      // `sealed`: to version `version` of the organisation key, so only
      // members' devices open it. `needs-entry`: it was kept the old way and
      // is gone; an owner or admin enters it again.
      credential: {
        kind: c.kind, header: c.header_name, set_at: c.credential_at, set_by: c.credential_by,
        state: c.credential_v >= 1 ? "sealed" : "needs-entry", version: c.credential_v >= 1 ? c.credential_v : null,
      },
      created_at: c.created_at,
      created_by: c.created_by,
      changed_at: c.changed_at,
      changed_by: c.changed_by,
      uses: {
        today: uses.filter((u) => u.day === today).reduce((s, u) => s + u.n, 0),
        last_30_days: uses.reduce((s, u) => s + u.n, 0),
      },
    };
  },

  /// A credential sealed to the organisation key the hub has now, or the
  /// refusal: no key yet, or sealed to a version that is no longer current.
  connectionKeyRefusal(ctx, action, target, a) {
    const cur = this.currentKey();
    if (!credentialsOn(this)) {
      return this.deny(ctx, action, target, "no-org-key", `${ctx.slug} has no organisation key yet, and a credential is sealed to it -- an owner runs \`krate org keys init ${ctx.slug}\``);
    }
    if (a.credential_v !== cur.version) {
      return this.deny(ctx, action, target, "key-rotated", `${ctx.slug}'s key is now version ${cur.version}: the credential is sealed again to it`);
    }
    return null;
  },

  /// The command's member, refused unless their role may do `action`.
  connectionMay(ctx, action, target, words) {
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return { refusal };
    if (allows(me.role, action) !== "yes") return { refusal: this.deny(ctx, action, target, "role-cannot", words(me.role)) };
    return { me };
  },

  /// Audience and app list from a command's arguments, checked. `fallback`
  /// is the connection as it is, for a change that leaves one out.
  connectionScope(ctx, action, target, fallback) {
    const a = ctx.args;
    let audience = fallback ? fallback.audience : null;
    if (a.audience !== undefined) {
      const au = a.audience;
      if (au === null || au === "everyone" || (au && au.everyone === true)) {
        audience = null;
      } else {
        const raw = Array.isArray(au) ? au : au && Array.isArray(au.groups) ? au.groups : null;
        if (!raw || !raw.length || raw.length > 50) {
          return { refusal: this.deny(ctx, action, target, "connection-invalid", "name the groups who may use it, or say everyone") };
        }
        const groups = [...new Set(raw.map((g) => String(g || "").trim().toLowerCase()))].sort();
        if (groups.some((g) => !GROUP.test(g))) return { refusal: this.deny(ctx, action, target, "group-invalid") };
        const missing = groups.find((g) => !this.groupRow(g));
        if (missing) return { refusal: this.deny(ctx, action, target, "no-such-group", `${ctx.slug} has no group called ${missing} -- create it first`) };
        audience = JSON.stringify(groups);
      }
    }
    let apps = fallback ? fallback.apps : null;
    if (a.apps !== undefined) {
      const ap = a.apps;
      if (ap === null || ap === "any" || (ap && ap.any === true)) {
        apps = null;
      } else {
        const raw = Array.isArray(ap) ? ap : ap && Array.isArray(ap.ids) ? ap.ids : null;
        if (!raw || !raw.length || raw.length > MAX_APPS) {
          return { refusal: this.deny(ctx, action, target, "connection-invalid", "name the app ids that may use it (com.acme.ledger), or say any") };
        }
        const ids = [...new Set(raw.map((x) => String(x || "").trim().toLowerCase()))].sort();
        const bad = ids.find((x) => !APP_ID.test(x));
        if (bad !== undefined) {
          return { refusal: this.deny(ctx, action, target, "connection-invalid", `${JSON.stringify(bad.slice(0, 40))} is not an app id: it is the id in the app's manifest, such as com.acme.ledger`) };
        }
        apps = JSON.stringify(ids);
      }
    }
    return { audience, apps };
  },

  /// GET /orgs/<slug>/connections.
  op_connections_list(ctx) {
    const { refusal } = this.connectionMay(ctx, "connection.view", `org:${ctx.slug}`, (r) => `a ${r} cannot see ${ctx.slug}'s connections`);
    if (refusal) return refusal;
    return this.ok({ connections: this.rows("SELECT * FROM connections ORDER BY name").map((c) => this.connectionView(c, ctx.now)) });
  },

  /// POST /orgs/<slug>/connections. The credential arrives sealed to the
  /// organisation key where it was typed, with its kind, header name and
  /// key version; it is kept as it came.
  op_connection_create(ctx) {
    const a = ctx.args;
    const name = String(a.name || "").trim().toLowerCase();
    const target = `connection:${name.slice(0, 40)}`;
    const { me, refusal } = this.connectionMay(ctx, "connection.manage", target, (r) => `a ${r} cannot set up ${ctx.slug}'s connections`);
    if (refusal) return refusal;
    if (!CONNECTION.test(name)) {
      return this.deny(ctx, "connection.manage", target, "connection-invalid", "a connection's name is 2 to 39 characters: lower-case letters, digits and dashes, starting with a letter");
    }
    if (this.connectionRow(name)) return this.deny(ctx, "connection.manage", target, "connection-taken", `${ctx.slug} already has a connection called ${name}`);
    if (!a.base || !a.allow) return this.deny(ctx, "connection.manage", target, "connection-invalid", "give its address and what it allows");
    if (!a.sealed) return this.deny(ctx, "connection.manage", target, "connection-invalid", "give its credential: a header and its value, or a user and password");
    const unkeyed = this.connectionKeyRefusal(ctx, "connection.manage", target, a);
    if (unkeyed) return unkeyed;
    const scope = this.connectionScope(ctx, "connection.manage", target, null);
    if (scope.refusal) return scope.refusal;
    this.sql.exec(
      `INSERT INTO connections (name, base, methods, paths, audience, apps, kind, header_name, sealed,
         created_at, created_by, changed_at, changed_by, credential_at, credential_by, credential_v, identity)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      name, a.base, JSON.stringify(a.allow.methods), JSON.stringify(a.allow.paths), scope.audience, scope.apps,
      a.kind, a.header_name, a.sealed,
      ctx.now, me.principal, ctx.now, me.principal, ctx.now, me.principal, a.credential_v, a.identity === false ? 0 : 1,
    );
    this.audit(me.principal, "connection.create", target, "allowed", a.base);
    return this.ok({ connection: this.connectionView(this.connectionRow(name), ctx.now) }, 201);
  },

  /// PUT /orgs/<slug>/connections/<name>: address, allow, audience, apps.
  /// A new address needs a new credential with it.
  op_connection_update(ctx) {
    const a = ctx.args;
    const name = String(a.name || "").toLowerCase();
    const target = `connection:${name.slice(0, 40)}`;
    const { me, refusal } = this.connectionMay(ctx, "connection.manage", target, (r) => `a ${r} cannot change ${ctx.slug}'s connections`);
    if (refusal) return refusal;
    const c = this.connectionRow(name);
    if (!c) return this.deny(ctx, "connection.manage", target, "no-such-connection", `${ctx.slug} has no connection called ${name || "that"}`);
    const base = a.base || c.base;
    if (base !== c.base && !a.sealed) return this.deny(ctx, "connection.manage", target, "credential-required");
    if (a.sealed) {
      const unkeyed = this.connectionKeyRefusal(ctx, "connection.manage", target, a);
      if (unkeyed) return unkeyed;
    }
    const scope = this.connectionScope(ctx, "connection.manage", target, c);
    if (scope.refusal) return scope.refusal;
    const methods = a.allow ? JSON.stringify(a.allow.methods) : c.methods;
    const paths = a.allow ? JSON.stringify(a.allow.paths) : c.paths;
    const said = [];
    if (base !== c.base) said.push(`address ${base}`);
    if (methods !== c.methods || paths !== c.paths) said.push(`allow ${JSON.parse(methods).join(",")} ${JSON.parse(paths).join(",")}`);
    if (scope.audience !== c.audience) said.push(scope.audience ? `audience groups:${list(scope.audience).join(",")}` : "audience everyone");
    if (scope.apps !== c.apps) said.push(scope.apps ? `apps ${list(scope.apps).join(",")}` : "apps any");
    const identity = a.identity === undefined ? identityOn(c) : a.identity === true;
    if (identity !== identityOn(c)) said.push(identity ? "tells the API who is calling" : "does not tell the API who is calling");
    if (a.sealed) said.push("credential replaced");
    if (said.length) {
      this.sql.exec(
        `UPDATE connections SET base = ?, methods = ?, paths = ?, audience = ?, apps = ?, identity = ?, changed_at = ?, changed_by = ? WHERE name = ?`,
        base, methods, paths, scope.audience, scope.apps, identity ? 1 : 0, ctx.now, me.principal, name,
      );
      if (a.sealed) this.connectionNewCredential(name, a, me, ctx.now);
      this.audit(me.principal, "connection.change", target, "allowed", said.join("; "));
    }
    return this.ok({ connection: this.connectionView(this.connectionRow(name), ctx.now) });
  },

  connectionNewCredential(name, a, me, now) {
    this.sql.exec(
      "UPDATE connections SET kind = ?, header_name = ?, sealed = ?, credential_v = ?, credential_at = ?, credential_by = ? WHERE name = ?",
      a.kind, a.header_name, a.sealed, a.credential_v, now, me.principal, name,
    );
  },

  /// PUT /orgs/<slug>/connections/<name>/credential: replace it. The next
  /// run of every app that uses it gets the new one.
  op_connection_credential(ctx) {
    const a = ctx.args;
    const name = String(a.name || "").toLowerCase();
    const target = `connection:${name.slice(0, 40)}`;
    const { me, refusal } = this.connectionMay(ctx, "connection.manage", target, (r) => `a ${r} cannot change ${ctx.slug}'s connections`);
    if (refusal) return refusal;
    if (!this.connectionRow(name)) return this.deny(ctx, "connection.manage", target, "no-such-connection", `${ctx.slug} has no connection called ${name || "that"}`);
    if (!a.sealed) return this.deny(ctx, "connection.manage", target, "connection-invalid", "give the new credential: a header and its value, or a user and password");
    const unkeyed = this.connectionKeyRefusal(ctx, "connection.manage", target, a);
    if (unkeyed) return unkeyed;
    this.connectionNewCredential(name, a, me, ctx.now);
    this.sql.exec("UPDATE connections SET changed_at = ?, changed_by = ? WHERE name = ?", ctx.now, me.principal, name);
    this.audit(me.principal, "connection.credential", target, "allowed");
    return this.ok({ connection: this.connectionView(this.connectionRow(name), ctx.now) });
  },

  /// DELETE /orgs/<slug>/connections/<name>. The credential goes with it.
  op_connection_remove(ctx) {
    const name = String(ctx.args.name || "").toLowerCase();
    const target = `connection:${name.slice(0, 40)}`;
    const { me, refusal } = this.connectionMay(ctx, "connection.manage", target, (r) => `a ${r} cannot remove ${ctx.slug}'s connections`);
    if (refusal) return refusal;
    if (!this.connectionRow(name)) return this.deny(ctx, "connection.manage", target, "no-such-connection", `${ctx.slug} has no connection called ${name || "that"}`);
    this.connectionFlushUses(ctx.now + DAY, name);
    this.sql.exec("DELETE FROM connections WHERE name = ?", name);
    this.sql.exec("DELETE FROM connection_uses WHERE name = ?", name);
    this.audit(me.principal, "connection.remove", target, "allowed");
    return this.ok(null, 204);
  },

  /// GET /orgs/<slug>/connections/<name>/credential/<digest>: may this
  /// person's copy of this release use this connection now? Checked like a
  /// lease, then counted. Answers the sealed credential for the Worker, and,
  /// when the connection tells the API who is calling, what the Worker signs
  /// into the identity token. The token's refresh (.../identity/<digest>)
  /// asks this same question, so it passes exactly the same checks; each
  /// one is counted as a use, in the same daily count, never one audit line
  /// per token.
  op_connection_use(ctx) {
    const name = String(ctx.args.name || "").toLowerCase();
    const digest = String(ctx.args.digest || "");
    const target = `connection:${name.slice(0, 40)}`;
    const action = "connection.use";
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return refusal;
    if (allows(me.role, action) !== "yes") return this.deny(ctx, action, target, "role-cannot");
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn") return this.withdrawnRefusal(ctx, c, action, target);
    if (!c || c.state !== "released") return this.deny(ctx, action, target, "digest-mismatch", `that app is not one of ${ctx.slug}'s releases`);
    const outside = this.audienceRefusal(ctx, me, c, action, target);
    if (outside) return outside;
    const conn = this.connectionRow(name);
    if (!conn) return this.deny(ctx, action, target, "no-such-connection", `${this.org().name || ctx.slug} has no connection called ${name || "that"} any more -- ask an admin of ${this.org().name || ctx.slug}`);
    // From here the person is a member who may open this release, so the
    // address is theirs to know: the device words its refusal with it.
    const refuse = (reason, detail) => {
      const r = this.deny(ctx, action, target, reason, detail);
      r.body.base = conn.base;
      return r;
    };
    const orgName = this.org().name || ctx.slug;
    const audience = list(conn.audience);
    if (audience) {
      const mine = this.groupsOf(me.principal);
      if (!audience.some((g) => mine.has(g))) {
        const r = refuse("connection-not-in-audience", `the ${name} connection is only for the ${spoken(audience)} group${audience.length === 1 ? "" : "s"} in ${orgName} -- ask an admin of ${orgName} to add you`);
        r.body.groups = audience;
        return r;
      }
    }
    const apps = list(conn.apps);
    const app = appOf(c);
    if (apps && !apps.includes(app)) return refuse("app-not-allowed", `${orgName} has not let ${c.name || app} use the ${name} connection -- an owner or admin of ${orgName} can add ${app} to it`);
    const asked = list(c.connections) || [];
    if (!asked.includes(name)) return refuse("connection-not-requested", `this version of ${c.name || app} did not ask for the ${name} connection (net.connection:${name} in its manifest), so it does not get it`);
    if (!(conn.credential_v >= 1)) {
      return refuse("credential-needs-entry", `the ${name} credential must be entered again by an owner or admin of ${orgName} before apps can use it: it was kept the old way, which Krate's hub could read, so it was deleted`);
    }
    this.sql.exec(
      `INSERT INTO connection_uses (name, day, n) VALUES (?, ?, 1)
       ON CONFLICT (name, day) DO UPDATE SET n = n + 1`,
      name,
      this.orgDay(ctx.now),
    );
    return this.ok({
      name,
      base: conn.base,
      allow: { methods: JSON.parse(conn.methods), paths: JSON.parse(conn.paths) },
      kind: conn.kind,
      header_name: conn.header_name,
      credential_v: conn.credential_v,
      sealed: conn.sealed,
      identity: identityOn(conn)
        ? { sub: me.principal, org: this.org().slug || ctx.slug, app, release: digest, connection: name, aud: conn.base, now: ctx.now }
        : null,
    });
  },

  /// The one-time migration (F-316): a credential kept the old way was
  /// sealed with this hub's own key, so the hub could read it, and the hub
  /// cannot seal it to the organisation key without reading it. It is
  /// deleted and marked to be entered again; the dashboard and `krate org
  /// connection list` say so, and a device asking for it is told so.
  connectionMigrateOldCredentials() {
    const old = this.rows("SELECT name FROM connections WHERE credential_v IS NULL");
    for (const c of old) {
      this.sql.exec("UPDATE connections SET sealed = '', credential_v = 0 WHERE name = ?", c.name);
      this.audit("system:krate", "connection.needs-entry", `connection:${c.name}`, "allowed", "kept the old way: enter it again");
    }
  },

  /// What the device that replaces the organisation key needs to seal every
  /// credential again: each one's name, address, header, version and blob.
  connectionsToReseal() {
    return this.rows("SELECT name, base, kind, header_name, credential_v, sealed FROM connections WHERE credential_v >= 1 ORDER BY name").map(
      (c) => ({ name: c.name, base: c.base, kind: c.kind, header: c.header_name, v: c.credential_v, sealed: c.sealed }),
    );
  },

  /// One audit line per connection per finished day: how many times its
  /// credential was handed out. `before` is the first moment not yet
  /// finished; `only` limits it to one connection.
  connectionFlushUses(before, only) {
    const day = this.orgDay(before);
    const due = this.rows(
      `SELECT name, day, n FROM connection_uses WHERE audited = 0 AND day < ? ${only ? "AND name = ?" : ""} ORDER BY day, name`,
      ...(only ? [day, only] : [day]),
    );
    for (const u of due) {
      this.audit("system:krate", "connection.use", `connection:${u.name}`, "allowed", `${u.n} use${u.n === 1 ? "" : "s"} on ${u.day}`);
      this.sql.exec("UPDATE connection_uses SET audited = 1 WHERE name = ? AND day = ?", u.name, u.day);
    }
  },
};

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({
    schema: SCHEMA,
    columns: COLUMNS,
    mutations: ["connection-create", "connection-update", "connection-credential", "connection-remove"],
    // Yesterday's counts become audit lines with the first command of today;
    // a credential kept the old way is gone before anything can use it.
    tick(ctx) {
      this.connectionMigrateOldCredentials();
      this.connectionFlushUses(ctx.now);
    },
  });
  const P = OrgClass.prototype;
  for (const name of ["op_keys_view", "op_keys_rotate"]) {
    if (typeof P[name] !== "function") throw new Error(`org-connections: ${name} is not installed`);
  }

  // A device that will replace the stale key is also handed every sealed
  // credential, to seal again to the new version.
  const view = P.op_keys_view;
  P.op_keys_view = function (ctx) {
    const answer = view.call(this, ctx);
    if (answer.status === 200 && answer.body && Array.isArray(answer.body.seal_to) && answer.body.seal_to.length) {
      answer.body.credentials = this.connectionsToReseal();
    }
    return answer;
  };

  // And the rotation is refused unless every credential comes back sealed
  // to the new version (or, one that did not open on that device, marked
  // to be entered again): none is left on a key a removed member holds.
  const rotate = P.op_keys_rotate;
  P.op_keys_rotate = function (ctx) {
    const a = ctx.args;
    const cur = this.currentKey();
    const by = this.deviceRow(String(a.by_device || ""));
    const mine = by && (by.principal === ctx.actor || (Array.isArray(ctx.principals) && ctx.principals.includes(by.principal)));
    const held = mine && by.state === "active" ? this.envelopeOf(by.device_id) : null;
    // Judged only for a rotation the keyring itself would take, so nobody
    // else learns a connection's name from the refusal.
    const due = cur && cur.state === "stale" && held && held.version === cur.version ? this.connectionsToReseal() : [];
    const given = a.credentials && typeof a.credentials === "object" ? a.credentials : {};
    const back = (c) => {
      const g = given[c.name];
      if (g && g.unopened === true) return "unopened";
      return g && g.v === a.version && goodSealed(g.sealed) ? "sealed" : null;
    };
    const missing = due.find((c) => !back(c));
    if (missing) {
      return {
        status: 400,
        body: { error: "bad-request", message: `every connection's credential is sealed again to the new version: ${missing.name} is missing` },
      };
    }
    const answer = rotate.call(this, ctx);
    if (answer.status !== 200) return answer;
    let sealed = 0;
    for (const c of due) {
      if (back(c) === "sealed") {
        this.sql.exec("UPDATE connections SET sealed = ?, credential_v = ? WHERE name = ?", given[c.name].sealed, a.version, c.name);
        sealed += 1;
      } else {
        this.sql.exec("UPDATE connections SET sealed = '', credential_v = 0 WHERE name = ?", c.name);
        this.audit(ctx.actor, "connection.needs-entry", `connection:${c.name}`, "allowed", `did not open on ${by.device_id}: enter it again`);
      }
    }
    if (sealed) this.audit(ctx.actor, "connection.reseal", `key:v${a.version}`, "allowed", `${sealed} credential${sealed === 1 ? "" : "s"} sealed again by ${by.device_id}`);
    return answer;
  };
  // A candidate keeps the connections its manifest asks for.
  const add = OrgClass.prototype.op_add_candidate;
  OrgClass.prototype.op_add_candidate = function (ctx) {
    const answer = add.call(this, ctx);
    const asked = Array.isArray(ctx.args.connections)
      ? [...new Set(ctx.args.connections.map((n) => String(n || "").toLowerCase()).filter((n) => CONNECTION.test(n)))].sort()
      : [];
    if (answer.status === 201 && asked.length) {
      this.sql.exec("UPDATE candidates SET connections = ? WHERE digest = ? AND connections IS NULL", JSON.stringify(asked), String(ctx.args.digest || ""));
    }
    return answer;
  };
}

install(Org);

/// The connections a judged bundle's manifest asks for.
export function connectionsAsked(judgement) {
  const caps = (judgement && judgement.manifest && judgement.manifest.capabilities) || [];
  return caps
    .map((c) => String((c && c.cap) || ""))
    .filter((cap) => cap.startsWith("net.connection:"))
    .map((cap) => cap.slice("net.connection:".length).toLowerCase());
}

// ------------------------------------------------- the credential seam
//
// Every place a credential is taken, kept or handed to a device goes
// through these three functions, and nothing else in the hub touches one.
// Under the organisation keyring (F-316) none of them can read it: the
// admin's browser or CLI sealed it to the organisation's public key (HPKE),
// the hub keeps that blob as it came, and only a member's device opens it.
// Every check made before a credential is handed out stays as it was.

/// Can this organisation's store take and hand out credentials? Only once
/// it has an organisation key: a credential is sealed to it.
export function credentialsOn(store) {
  return Boolean(store && store.currentKey());
}

/// What a create or replace carried, made into what the store keeps:
/// null when it carried no credential, `{problem}` for one that cannot be
/// kept, or `{sealed, v, kind, header_name}` -- the blob exactly as it came,
/// which the Idempotency-Key is bound to as it is (it is ciphertext).
export function credentialToStore(body) {
  return sealedCredentialOf(body);
}

/// What a device that may use the connection is handed for it: the sealed
/// blob, unopened, with what it is sealed for.
export function credentialForDevice(kept) {
  return { credential: { kind: kept.kind, header: kept.header_name, v: kept.credential_v, sealed: kept.sealed } };
}

// ---------------------------------------------------------------- routes

const noStore = (value, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json", "cache-control": "private, no-store" } });

/// /orgs/<slug>/connections... Answers a Response, or null.
export async function connectionRoute(h) {
  const { request, env, rest, method, askRaw, readBody, idempotencyOf, orgAnswer, url } = h;
  if (rest[0] !== "connections") return null;
  const bad = (message) => orgAnswer({ status: 400, body: { error: "connection-invalid", message } });

  if (rest.length === 1 && method === "GET") return orgAnswer(await askRaw("connections-list"));

  // The settings and the sealed credential of a create or change, checked.
  const prepare = (name, v, needsCredential) => {
    const args = { name };
    const stored = credentialToStore(v);
    if (stored && stored.problem) return { refusal: bad(stored.problem) };
    if (v.base !== undefined) {
      const b = normalBase(v.base, env);
      if (b.problem) return { refusal: bad(b.problem) };
      // The credential is sealed for the address the hub keeps, so it must
      // arrive with that address exactly: the device opens it with it.
      if (stored && String(v.base) !== b.base) {
        return { refusal: bad(`the credential is sealed for the address exactly as it is kept: send it as ${b.base}`) };
      }
      args.base = b.base;
    }
    if (v.allow !== undefined || needsCredential) {
      const a = normalAllow(v.allow);
      if (a.problem) return { refusal: bad(a.problem) };
      args.allow = a.allow;
    }
    if (v.audience !== undefined) args.audience = v.audience;
    if (v.apps !== undefined) args.apps = v.apps;
    if (v.identity !== undefined) {
      if (typeof v.identity !== "boolean") return { refusal: bad("identity is true (tell the API who is calling) or false") };
      args.identity = v.identity;
    }
    if (stored) {
      Object.assign(args, { sealed: stored.sealed, credential_v: stored.v, kind: stored.kind, header_name: stored.header_name });
    } else if (needsCredential) {
      return { refusal: bad("give its credential: a header and its value, or a user and password") };
    }
    return { args, bound: v };
  };

  if (rest.length === 1 && method === "POST") {
    const body = await readBody();
    if (body.tooLarge || body.bad) return bad("the body is JSON, under 64 KiB");
    const name = String(body.value.name || "").trim().toLowerCase();
    if (!CONNECTION.test(name)) {
      return bad("a connection's name is 2 to 39 characters: lower-case letters, digits and dashes, starting with a letter");
    }
    if (body.value.base === undefined) return bad("give its address, such as https://api.acme.com/finance");
    const p = prepare(name, body.value, true);
    if (p.refusal) return p.refusal;
    return orgAnswer(await askRaw("connection-create", p.args, await idempotencyOf(request, url.pathname, JSON.stringify(p.bound))));
  }

  const name = String(rest[1] || "").toLowerCase();
  if (!CONNECTION.test(name)) return orgAnswer({ status: 404, body: { error: "no-such-connection", message: "that is not a connection's name" } });

  if (rest.length === 2 && method === "PUT") {
    const body = await readBody();
    if (body.tooLarge || body.bad) return bad("the body is JSON, under 64 KiB");
    const p = prepare(name, body.value, false);
    if (p.refusal) return p.refusal;
    return orgAnswer(await askRaw("connection-update", p.args, await idempotencyOf(request, url.pathname, JSON.stringify(p.bound))));
  }
  if (rest.length === 2 && method === "DELETE") {
    return orgAnswer(await askRaw("connection-remove", { name }, await idempotencyOf(request, url.pathname, "")));
  }
  if (rest.length === 3 && rest[2] === "credential" && method === "PUT") {
    const body = await readBody();
    if (body.tooLarge || body.bad) return bad("the body is JSON, under 64 KiB");
    const v = body.value;
    const p = prepare(name, { header: v.header, basic: v.basic, credential: v.credential }, false);
    if (p.refusal) return p.refusal;
    if (!p.args.sealed) return bad("give the new credential: a header and its value, or a user and password");
    return orgAnswer(await askRaw("connection-credential", p.args, await idempotencyOf(request, url.pathname, JSON.stringify(p.bound))));
  }
  if (rest.length === 4 && rest[2] === "credential" && method === "GET") {
    const digest = String(rest[3]).toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(digest)) {
      return orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the release's 64-hex sha256" } });
    }
    const decided = await askRaw("connection-use", { name, digest });
    if (decided.status !== 200) return noStore(decided.body, decided.status);
    const d = decided.body;
    const identity = d.identity ? await issueIdentity(env, d.identity) : null;
    return noStore({ name: d.name, base: d.base, allow: d.allow, ...credentialForDevice(d), ...(identity ? { identity } : {}) });
  }
  // A fresh identity token for the same run: the same question as the
  // credential fetch, so the same checks, answering only the token. A
  // person removed since the app opened is refused here, so their name
  // stops reaching the API within one token's life.
  if (rest.length === 4 && rest[2] === "identity" && method === "GET") {
    const digest = String(rest[3]).toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(digest)) {
      return orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the release's 64-hex sha256" } });
    }
    const decided = await askRaw("connection-use", { name, digest });
    if (decided.status !== 200) return noStore(decided.body, decided.status);
    const d = decided.body;
    return noStore({ name: d.name, identity: d.identity ? await issueIdentity(env, d.identity) : null });
  }
  return null;
}
