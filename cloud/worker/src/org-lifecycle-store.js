// An organisation's lifecycle, inside its store (IC-948, Addendum 3 D,
// shapes from Addendum 4).
//
// Two things that are separate on purpose (R17): the lifecycle -- active,
// closing (a scheduled close with a window), closed -- and recovery of a
// lost owner. Neither ever stops an owner from taking the standard export
// (R24). Organisations are not billed (founder, 2026-11-07): nothing here
// pauses, suspends or limits an organisation for payment. Everything here
// runs inside the store's transactions, like every other command, and
// appends to the same audit chain.
//
// Settings, none of them a published promise: ORG_CLOSE_WINDOW_DAYS (30),
// ORG_EXPORT_WINDOW_DAYS (30, how long former owners can take the export
// after closing), ORG_EXPORT_PACKAGE_DAYS (7, how long one built package
// lives).

import { extendOrg, addReasons, parsePrincipal, displayOf, canonicalJson, sha256HexSync } from "./org.js";

const DAY = 24 * 60 * 60;
export const RECOVERY_WAIT_SECONDS = 72 * 60 * 60;

const setting = (env, name, fallback) => {
  const n = Number(env && env[name]);
  return Number.isFinite(n) && n > 0 ? n : fallback;
};
export const closeWindowDays = (env) => setting(env, "ORG_CLOSE_WINDOW_DAYS", 30);
export const exportWindowDays = (env) => setting(env, "ORG_EXPORT_WINDOW_DAYS", 30);
export const exportPackageDays = (env) => setting(env, "ORG_EXPORT_PACKAGE_DAYS", 7);

addReasons({
  "org-closing": {
    status: 409,
    message: (o, d) => d || `${o} is closing -- nothing new can be published or added; an owner can cancel the close`,
  },
  "org-closed": { status: 410, message: (o, d) => d || `${o} is closed` },
  "close-statement-mismatch": {
    status: 409,
    message: () => "that is not the digest of this close's statement -- read the statement again and approve the digest it shows",
  },
  "close-confirm-mismatch": {
    status: 400,
    message: (o) => `to close ${o}, type its name exactly: ${o}`,
  },
  "export-not-ready": { status: 409, message: (o, d) => d || "that export is not ready" },
  "recovery-not-waiting": { status: 409, message: (o, d) => d || "that recovery is not waiting any more" },
  "rate-limited": { status: 429, message: () => "too many refused attempts -- wait a minute and try again" },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS lifecycle (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     state TEXT NOT NULL, closed_at INTEGER, closed_owners TEXT, swept_at INTEGER)`,
  `CREATE TABLE IF NOT EXISTS closes (
     id TEXT PRIMARY KEY, state TEXT NOT NULL, statement TEXT NOT NULL, digest TEXT NOT NULL,
     requested_by TEXT NOT NULL, requested_at INTEGER NOT NULL,
     approved_by TEXT, approved_at INTEGER, ends_at INTEGER,
     cancelled_by TEXT, cancelled_at INTEGER)`,
  `CREATE TABLE IF NOT EXISTS exports (
     id TEXT PRIMARY KEY, state TEXT NOT NULL, requested_by TEXT NOT NULL,
     created_at INTEGER NOT NULL, expires_at INTEGER, size INTEGER, sha256 TEXT, problem TEXT)`,
  `CREATE TABLE IF NOT EXISTS recoveries (
     id TEXT PRIMARY KEY, state TEXT NOT NULL, principal TEXT NOT NULL, by TEXT NOT NULL,
     reason TEXT NOT NULL, requested_at INTEGER NOT NULL, effective_at INTEGER NOT NULL,
     closed_by TEXT, closed_at INTEGER, not_mailed TEXT)`,
  `CREATE TABLE IF NOT EXISTS invite_mail (day TEXT PRIMARY KEY, n INTEGER NOT NULL)`,
];

const randomId = (prefix) => {
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  return prefix + [...crypto.getRandomValues(new Uint8Array(12))].map((b) => alphabet[b % alphabet.length]).join("");
};

// ------------------------------------------------------------- helpers

function lifecycle(store) {
  return store.row("SELECT * FROM lifecycle WHERE id = 1") || { state: "active", closed_at: null, closed_owners: null };
}

function openClose(store) {
  return store.row("SELECT * FROM closes WHERE state IN ('waiting-for-second-owner', 'scheduled') ORDER BY requested_at DESC LIMIT 1");
}

function setLifecycle(store, state, extra = {}) {
  const now = lifecycle(store);
  store.sql.exec(
    `INSERT INTO lifecycle (id, state, closed_at, closed_owners) VALUES (1, ?, ?, ?)
     ON CONFLICT(id) DO UPDATE SET state = excluded.state, closed_at = excluded.closed_at, closed_owners = excluded.closed_owners`,
    state,
    extra.closed_at !== undefined ? extra.closed_at : now.closed_at,
    extra.closed_owners !== undefined ? extra.closed_owners : now.closed_owners,
  );
}

function formerOwners(store) {
  try {
    return JSON.parse(lifecycle(store).closed_owners || "[]");
  } catch (_) {
    return [];
  }
}

function closeView(c) {
  if (!c) return null;
  return {
    id: c.id,
    state: c.state,
    requested_by: c.requested_by,
    requested_at: c.requested_at,
    approved_by: c.approved_by || null,
    statement: JSON.parse(c.statement),
    statement_digest: c.digest,
    ends_at: c.ends_at || null,
  };
}

function recoveryView(r) {
  if (!r) return null;
  return {
    id: r.id,
    for: r.principal,
    by: r.by,
    reason: r.reason,
    requested_at: r.requested_at,
    effective_at: r.effective_at,
    state: r.state,
    // Who the notice could not reach, for the owners' banner.
    not_mailed: r.not_mailed ? JSON.parse(r.not_mailed) : [],
  };
}

/// The active owner or admin the command comes from, or the refusal.
function ownerOrAdmin(store, ctx, action, target, { ownersOnly = false } = {}) {
  const { me, refusal } = store.standing(ctx, action, target);
  if (refusal) return { refusal };
  const ok = ownersOnly ? me.role === "owner" : me.role === "owner" || me.role === "admin";
  if (!ok) {
    return { refusal: store.deny(ctx, action, target, "role-cannot", ownersOnly ? `only an owner of ${ctx.slug} can do that` : `only an owner or admin of ${ctx.slug} can do that`) };
  }
  return { me };
}

// ---------------------------------------------------- what is due, gates

/// Apply what time has made true: a scheduled close whose window ended,
/// a recovery whose wait passed. Runs before every command.
function tick(ctx) {
  const close = this.row("SELECT * FROM closes WHERE state = 'scheduled' AND ends_at <= ?", ctx.now);
  if (close && lifecycle(this).state !== "closed") {
    const owners = this.rows("SELECT principal FROM members WHERE role = 'owner' AND state = 'active'").map((m) => m.principal);
    this.sql.exec("UPDATE closes SET state = 'closed' WHERE id = ?", close.id);
    setLifecycle(this, "closed", { closed_at: close.ends_at, closed_owners: JSON.stringify(owners) });
    this.audit("system:krate", "org.closed", `org:${ctx.slug}`, "allowed", `close ${close.id}`);
  }
  const due = this.row("SELECT * FROM recoveries WHERE state = 'waiting' AND effective_at <= ?", ctx.now);
  if (due) {
    const m = this.member(due.principal);
    if (m) {
      this.sql.exec(
        "UPDATE members SET role = 'owner', state = 'active', removed_at = NULL, removed_by = NULL WHERE principal = ?",
        due.principal,
      );
    } else {
      this.sql.exec(
        "INSERT INTO members (principal, role, state, added_at, added_by) VALUES (?, 'owner', 'active', ?, ?)",
        due.principal,
        due.effective_at,
        `krate-support:${due.by}`,
      );
    }
    this.sql.exec("UPDATE recoveries SET state = 'done', closed_at = ? WHERE id = ?", ctx.now, due.id);
    // "Krate support restored ownership, by <admin>, reason ..."
    this.audit(`krate-support:${due.by}`, "member.recover", `member:${due.principal}`, "allowed", due.reason);
  }
}

const PUBLISH = new Set(["check-candidate", "add-candidate", "approve"]);
const GROW = new Set(["invite", "accept", "set-role"]);
const CLOSED_OPEN = new Set([
  "show",
  "audit",
  "exports",
  "export-begin",
  "export-done",
  "export-get",
  "sweep",
  "swept",
  "invite-info",
  "exists",
  // A member deleting their Krate account leaves a closed organisation too
  // (account-delete.js): its records keep naming them, as removed.
  "account-leave",
]);

/// Refuse what the lifecycle does not allow, before the
/// command runs. Every refusal is audited like any other.
function gate(ctx) {
  const life = lifecycle(this).state;
  const target = `org:${ctx.slug}`;
  if (life === "closing") {
    if (PUBLISH.has(ctx.op) || GROW.has(ctx.op) || ctx.op === "set-root") {
      return this.deny(ctx, ctx.op, target, "org-closing");
    }
  }
  if (life === "closed" && !CLOSED_OPEN.has(ctx.op)) {
    return this.deny(ctx, ctx.op, target, "org-closed");
  }
  if (life === "closed" && (ctx.op === "show" || ctx.op === "audit")) {
    // Read-only for the owners it had when it closed (Addendum 4, D5).
    if (!ctx.principals.some((p) => formerOwners(this).includes(p))) {
      return this.deny(ctx, ctx.op, target, "org-closed");
    }
  }
  return null;
}

/// The fields Addendum 4 (D1) adds to GET /orgs/<slug>.
function view(org, me) {
  const life = lifecycle(this);
  org.state = life.state;
  org.close = closeView(openClose(this));
  org.closed_at = life.closed_at || null;
  // Owners and admins only.
  org.recovery =
    me && me.state === "active" && (me.role === "owner" || me.role === "admin")
      ? recoveryView(this.row("SELECT * FROM recoveries ORDER BY requested_at DESC LIMIT 1"))
      : null;
}

// ------------------------------------------------------------ commands

const ops = {
  /// POST /orgs/<slug>/close (Addendum 4, D2). A sole owner confirms by
  /// retyping the slug; with two or more owners a second owner approves the
  /// statement's digest.
  close(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = ownerOrAdmin(this, ctx, "org.close", target, { ownersOnly: true });
    if (refusal) return refusal;
    if (lifecycle(this).state === "closing") return this.deny(ctx, "org.close", target, "org-closing", `${ctx.slug} is already closing`);
    const pending = openClose(this);
    if (pending) return this.ok({ close: closeView(pending) });
    const owners = this.activeOwners();
    const days = closeWindowDays(this.env);
    const statement = {
      org: ctx.slug,
      name: this.org().name,
      requested_by: me.principal,
      requested_at: ctx.now,
      window_days: days,
      effect:
        `During the ${days}-day window members keep opening releases, nothing new can be published, and owners can cancel or export. ` +
        `Then the organisation is closed: releases stop serving, and its former owners can take the standard export for ${exportWindowDays(this.env)} more days before its files are deleted.`,
    };
    const digest = sha256HexSync(canonicalJson(statement));
    const id = randomId("cl_");
    if (owners >= 2) {
      this.sql.exec(
        "INSERT INTO closes (id, state, statement, digest, requested_by, requested_at) VALUES (?, 'waiting-for-second-owner', ?, ?, ?, ?)",
        id,
        JSON.stringify(statement),
        digest,
        me.principal,
        ctx.now,
      );
      this.audit(ctx.actor, "org.close", target, "allowed", "waiting for a second owner");
      return this.ok({ close: closeView(this.row("SELECT * FROM closes WHERE id = ?", id)) }, 201);
    }
    if (String(ctx.args.confirm_slug || "") !== ctx.slug) {
      return this.deny(ctx, "org.close", target, "close-confirm-mismatch");
    }
    const ends = ctx.now + days * DAY;
    this.sql.exec(
      "INSERT INTO closes (id, state, statement, digest, requested_by, requested_at, ends_at) VALUES (?, 'scheduled', ?, ?, ?, ?, ?)",
      id,
      JSON.stringify(statement),
      digest,
      me.principal,
      ctx.now,
      ends,
    );
    setLifecycle(this, "closing");
    this.audit(ctx.actor, "org.close", target, "allowed", "scheduled by the sole owner");
    return this.ok({ close: closeView(this.row("SELECT * FROM closes WHERE id = ?", id)) }, 201);
  },

  /// POST /orgs/<slug>/close/<id>/approve {statement_digest}: a second owner.
  "close-approve"(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = ownerOrAdmin(this, ctx, "org.close.approve", target, { ownersOnly: true });
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM closes WHERE id = ?", String(ctx.args.id || ""));
    if (!c || c.state !== "waiting-for-second-owner") {
      return this.deny(ctx, "org.close.approve", target, "close-statement-mismatch", "there is no close waiting for approval with that id");
    }
    if (c.requested_by === me.principal || ctx.principals.includes(c.requested_by)) {
      return this.deny(ctx, "org.close.approve", target, "self-approval", "you asked for this close, so another owner has to approve it");
    }
    if (String(ctx.args.statement_digest || "") !== c.digest) {
      return this.deny(ctx, "org.close.approve", target, "close-statement-mismatch");
    }
    const ends = ctx.now + closeWindowDays(this.env) * DAY;
    this.sql.exec(
      "UPDATE closes SET state = 'scheduled', approved_by = ?, approved_at = ?, ends_at = ? WHERE id = ?",
      me.principal,
      ctx.now,
      ends,
      c.id,
    );
    setLifecycle(this, "closing");
    this.audit(ctx.actor, "org.close.approve", target, "allowed", c.digest.slice(0, 16));
    return this.ok({ close: closeView(this.row("SELECT * FROM closes WHERE id = ?", c.id)) });
  },

  /// DELETE /orgs/<slug>/close/<id> (Addendum 4, D3): owners, while it waits
  /// or is scheduled.
  "close-cancel"(ctx) {
    const target = `org:${ctx.slug}`;
    const { refusal } = ownerOrAdmin(this, ctx, "org.close.cancel", target, { ownersOnly: true });
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM closes WHERE id = ?", String(ctx.args.id || ""));
    if (!c || (c.state !== "waiting-for-second-owner" && c.state !== "scheduled")) {
      return this.deny(ctx, "org.close.cancel", target, "close-statement-mismatch", "there is no waiting or scheduled close with that id");
    }
    this.sql.exec("UPDATE closes SET state = 'cancelled', cancelled_by = ?, cancelled_at = ? WHERE id = ?", ctx.actor, ctx.now, c.id);
    setLifecycle(this, "active");
    this.audit(ctx.actor, "org.close.cancel", target, "allowed");
    return this.ok(null, 204);
  },

  /// Who may take the export now: an active owner, or -- after the close --
  /// a former owner within the export window (R24).
  "export-begin"(ctx) {
    const target = `org:${ctx.slug}`;
    const allowed = exportAllowed(this, ctx, "org.export", target);
    if (allowed.refusal) return allowed.refusal;
    const id = randomId("ex_");
    this.sql.exec("INSERT INTO exports (id, state, requested_by, created_at) VALUES (?, 'building', ?, ?)", id, allowed.as, ctx.now);
    this.audit(allowed.as, "org.export", `export:${id}`, "allowed");
    return this.ok({ export: exportView(this.row("SELECT * FROM exports WHERE id = ?", id)), data: exportData(this, ctx) }, 202);
  },

  "export-done"(ctx) {
    const id = String(ctx.args.id || "");
    const ok = ctx.args.state === "ready";
    this.sql.exec(
      "UPDATE exports SET state = ?, size = ?, sha256 = ?, expires_at = ?, problem = ? WHERE id = ?",
      ok ? "ready" : "failed",
      Number(ctx.args.size) || null,
      ctx.args.sha256 || null,
      ok ? ctx.now + exportPackageDays(this.env) * DAY : null,
      ok ? null : String(ctx.args.problem || "").slice(0, 300),
      id,
    );
    return this.ok({ export: exportView(this.row("SELECT * FROM exports WHERE id = ?", id)) });
  },

  exports(ctx) {
    const allowed = exportAllowed(this, ctx, "org.exports", `org:${ctx.slug}`);
    if (allowed.refusal) return allowed.refusal;
    return this.ok(this.rows("SELECT * FROM exports ORDER BY created_at DESC").map(exportView));
  },

  "export-get"(ctx) {
    const id = String(ctx.args.id || "");
    const target = `export:${id}`;
    const allowed = exportAllowed(this, ctx, "org.export.get", target);
    if (allowed.refusal) return allowed.refusal;
    const e = this.row("SELECT * FROM exports WHERE id = ?", id);
    if (!e) return this.deny(ctx, "org.export.get", target, "export-not-ready", "there is no export with that id");
    if (e.state !== "ready") return this.deny(ctx, "org.export.get", target, "export-not-ready", e.state === "failed" ? `that export failed (${e.problem || "no reason recorded"}) -- build a new one` : "that export is still being built");
    if (e.expires_at <= ctx.now) {
      return this.deny(ctx, "org.export.get", target, "export-not-ready", "that package has expired -- build a new export; the organisation's export stays available");
    }
    return this.ok({ export: exportView(e) });
  },

  /// The scheduled sweep: what to delete once a closed organisation's
  /// export window has passed. Read-only until `swept` records it.
  sweep(ctx) {
    const life = lifecycle(this);
    if (life.state !== "closed" || life.swept_at) return this.ok({ due: false });
    if (ctx.now < (life.closed_at || 0) + exportWindowDays(this.env) * DAY) return this.ok({ due: false });
    return this.ok({
      due: true,
      digests: this.rows("SELECT digest, big FROM candidates").map((c) => ({ digest: c.digest, big: Boolean(c.big) })),
      exports: this.rows("SELECT id FROM exports").map((e) => e.id),
    });
  },

  swept(ctx) {
    const deleted = Array.isArray(ctx.args.deleted) ? ctx.args.deleted : [];
    this.sql.exec("UPDATE lifecycle SET swept_at = ? WHERE id = 1", ctx.now);
    this.sql.exec("UPDATE exports SET state = 'deleted' WHERE state != 'deleted'");
    // What it deleted, by digest: the record of the deletion is kept.
    this.audit("system:krate", "org.files.deleted", `org:${ctx.slug}`, "allowed", `${deleted.length} objects: ${deleted.map((d) => String(d).slice(0, 16)).join(",")}`.slice(0, 900));
    return this.ok({ swept: true, deleted: deleted.length });
  },

  /// POST /orgs/<slug>/recover, by a Krate admin the door has checked:
  /// `{principal, reason, admin}`. Nobody gains anything until the wait
  /// passes without an objection.
  "recover-start"(ctx) {
    const p = parsePrincipal(ctx.args.principal);
    const reason = String(ctx.args.reason || "").trim().slice(0, 500);
    const admin = String(ctx.args.admin || "");
    if (!p) return { status: 400, body: { error: "role-cannot", message: "name the new owner as github:<login> or email:<address>" } };
    if (reason.length < 10) return { status: 400, body: { error: "role-cannot", message: "write the reason: what was checked, and who asked" } };
    if (lifecycle(this).state === "closed") return { status: 410, body: { error: "org-closed", message: `${ctx.slug} is closed` } };
    const waiting = this.row("SELECT * FROM recoveries WHERE state = 'waiting'");
    if (waiting) return this.ok({ recovery: recoveryView(waiting), notify: [] });
    const id = randomId("rc_");
    this.sql.exec(
      "INSERT INTO recoveries (id, state, principal, by, reason, requested_at, effective_at) VALUES (?, 'waiting', ?, ?, ?, ?, ?)",
      id,
      p.principal,
      admin,
      reason,
      ctx.now,
      ctx.now + RECOVERY_WAIT_SECONDS,
    );
    this.audit(`krate-support:${admin}`, "member.recover.request", `member:${p.principal}`, "allowed", reason);
    const notify = this.rows("SELECT principal, role FROM members WHERE state = 'active' AND role IN ('owner', 'admin')").map((m) => m.principal);
    return this.ok({ recovery: recoveryView(this.row("SELECT * FROM recoveries WHERE id = ?", id)), notify, org_name: this.org().name }, 201);
  },

  /// After the notices went: who could not be told, kept with the recovery
  /// so the owners' banner can say so. From the door only.
  "recover-noted"(ctx) {
    const list = (Array.isArray(ctx.args.not_mailed) ? ctx.args.not_mailed : []).map(String).slice(0, 200);
    this.sql.exec("UPDATE recoveries SET not_mailed = ? WHERE id = ?", JSON.stringify(list), String(ctx.args.id || ""));
    return this.ok({ recovery: recoveryView(this.row("SELECT * FROM recoveries WHERE id = ?", String(ctx.args.id || ""))) });
  },

  /// POST /orgs/<slug>/recover/<id>/object (Addendum 4, D6): any current
  /// owner or admin cancels it.
  "recover-object"(ctx) {
    const id = String(ctx.args.id || "");
    const target = `recovery:${id}`;
    const { refusal } = ownerOrAdmin(this, ctx, "member.recover.object", target);
    if (refusal) return refusal;
    const r = this.row("SELECT * FROM recoveries WHERE id = ?", id);
    if (!r || r.state !== "waiting") return this.deny(ctx, "member.recover.object", target, "recovery-not-waiting");
    this.sql.exec("UPDATE recoveries SET state = 'cancelled', closed_by = ?, closed_at = ? WHERE id = ?", ctx.actor, ctx.now, id);
    this.audit(ctx.actor, "member.recover.object", `member:${r.principal}`, "allowed", "objected; recovery cancelled");
    return this.ok({ recovery: recoveryView(this.row("SELECT * FROM recoveries WHERE id = ?", id)) });
  },

  /// The day's invitation mails, per organisation: at most 20 (Addendum 3, A).
  "invite-mail-slot"(ctx) {
    const day = new Date(ctx.now * 1000).toISOString().slice(0, 10);
    const used = (this.row("SELECT n FROM invite_mail WHERE day = ?", day) || { n: 0 }).n;
    if (used >= 20) return this.ok({ allowed: false });
    this.sql.exec("INSERT INTO invite_mail (day, n) VALUES (?, 1) ON CONFLICT(day) DO UPDATE SET n = n + 1", day);
    return this.ok({ allowed: true, used: used + 1 });
  },

  /// Record what happened to an invitation's mail on the answer the
  /// Idempotency-Key replays, so a retry reports the same `emailed`.
  "invite-mailed"(ctx) {
    const key = ctx.args.idempotency_key;
    if (ctx.actor && key) {
      const prior = this.row("SELECT body FROM idempotency WHERE actor = ? AND key = ?", ctx.actor, String(key));
      if (prior && prior.body) {
        const body = JSON.parse(prior.body);
        body.emailed = Boolean(ctx.args.emailed);
        if (ctx.args.email_problem) body.email_problem = ctx.args.email_problem;
        else delete body.email_problem;
        this.sql.exec("UPDATE idempotency SET body = ? WHERE actor = ? AND key = ?", JSON.stringify(body), ctx.actor, String(key));
      }
    }
    return this.ok({ ok: true });
  },
};

function exportView(e) {
  if (!e) return null;
  const out = { id: e.id, state: e.state, created_at: e.created_at, expires_at: e.expires_at || null };
  if (e.size) out.size = e.size;
  return out;
}

/// Owners while it is open (R24); former owners within
/// the export window once it has closed.
function exportAllowed(store, ctx, action, target) {
  const life = lifecycle(store);
  if (life.state === "closed") {
    const as = ctx.principals.find((p) => formerOwners(store).includes(p));
    if (!as) return { refusal: store.deny(ctx, action, target, "org-closed") };
    if (ctx.now >= (life.closed_at || 0) + exportWindowDays(store.env) * DAY && action !== "org.exports") {
      return { refusal: store.deny(ctx, action, target, "org-closed", `${ctx.slug} closed and its export window has ended`) };
    }
    return { as };
  }
  const { me, refusal } = ownerOrAdmin(store, ctx, action, target, { ownersOnly: true });
  if (refusal) return { refusal };
  return { as: me.principal };
}

/// What the standard export carries from the store. Never a key, token,
/// session, secret or invitation code.
function exportData(store, ctx) {
  const o = store.org();
  return {
    org: { slug: o.slug, name: o.name, created_at: o.created_at, created_by: o.created_by, state: lifecycle(store).state },
    members: store.rows("SELECT * FROM members ORDER BY added_at, rowid").map((m) => ({
      principal: m.principal,
      display: displayOf(m.principal),
      role: m.role,
      state: m.state,
      since: m.added_at,
      added_by: m.added_by,
      removed_at: m.removed_at || null,
      removed_by: m.removed_by || null,
    })),
    candidates: store.rows("SELECT * FROM candidates ORDER BY uploaded_at, digest").map((c) => ({
      digest: c.digest,
      name: c.name,
      version: c.version,
      namespace: c.namespace,
      size: c.size,
      big: Boolean(c.big),
      state: c.state,
      uploaded_by: c.uploaded_by,
      uploaded_at: c.uploaded_at,
      approved_by: c.approved_by,
      approved_at: c.approved_at,
    })),
    audit: store.rows("SELECT * FROM audit ORDER BY seq"),
  };
}

export { lifecycle };

extendOrg({
  schema: SCHEMA,
  // Added after organisations already existed (84643f288).
  columns: [["recoveries", "not_mailed", "TEXT"]],
  ops,
  // An export is not one: building a second one is harmless, and its answer
  // carries the store's data, which is not for the idempotency table.
  mutations: ["close", "close-approve", "close-cancel", "recover-object"],
  tick,
  gate,
  view,
});
