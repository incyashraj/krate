// Release control and groups (IC-949, FLEET-CONTRACT.md section R).
//
// Four things an organisation's people do with what it has released:
//
// - **Withdraw** a release, with a reason the people who had it are shown.
//   From that commit on, a download, a look and a lease renewal of the
//   digest are refused as `release-withdrawn` (410). A device refuses it
//   when it next checks in, because its check-in is that same look. A lease
//   already issued cannot be called back from a device that stays offline
//   (R9): it holds until it runs out, and the dashboard says so before
//   anyone confirms. Withdrawing is not undone; the same bytes stay
//   withdrawn, and releasing the app again means a new candidate.
// - **Current release** per app id: the approved release that the
//   organisation's link for an app (`/orgs/<slug>/apps/<app-id>/current`)
//   opens. Approving a release makes it current; choosing an older
//   approved one is the rollback; withdrawing the current one makes the
//   newest remaining approved release current, or none.
// - **Groups**: named sets of members (`finance`), managed by owners and
//   admins.
// - **Audience** of a release: everyone in the organisation (the default),
//   or a list of groups. A member outside it is refused download, look and
//   lease as `not-in-audience` (403). Owners, admins and reviewers always
//   pass: they have to be able to review what goes out.
//
// Every one of these is a command in the organisation's store, in one
// transaction, audited, and in the role matrix (org.js MATRIX).

import { Org, extendOrg, addReasons, allows, parsePrincipal, displayOf } from "./org.js";
// The lease command must exist before it is wrapped below: a withdrawn or
// out-of-audience release must never renew offline access. Importing it
// here makes the order this module's own, not the order of index.js.
import "./org-lease.js";
// Days are the organisation's own (org-time.js, F-319).
import "./org-time.js";

/// Who passes every audience: they must be able to review (section R).
export const SEES_ALL = new Set(["owner", "admin", "reviewer"]);
export const GROUP = /^[a-z][a-z0-9-]{0,38}$/;
const REASON_MAX = 300;
const APP_MAX = 120;

addReasons({
  "release-withdrawn": {
    status: 410,
    message: (o, d) => d || `${o} withdrew this release -- ask an admin of ${o} which version to use now`,
  },
  "not-in-audience": {
    status: 403,
    message: (o, d) => d || `this app is only for some groups in ${o} -- ask an admin of ${o} to add you to one`,
  },
  "reason-required": {
    status: 400,
    message: () => "say why it is withdrawn, in up to 300 characters: the people who had it are shown the reason",
  },
  "no-such-group": {
    status: 404,
    message: (o, d) => d || `there is no group by that name in ${o} -- create it first`,
  },
  "group-taken": { status: 409, message: (o, d) => d || `${o} already has a group by that name` },
  "group-invalid": {
    status: 400,
    message: () => "a group name is 1 to 39 characters: lower-case letters, digits and dashes, starting with a letter",
  },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS groups (
     name TEXT PRIMARY KEY, created_at INTEGER NOT NULL, created_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS group_members (
     grp TEXT NOT NULL, principal TEXT NOT NULL,
     added_at INTEGER NOT NULL, added_by TEXT NOT NULL,
     PRIMARY KEY (grp, principal))`,
  `CREATE TABLE IF NOT EXISTS current_releases (
     app_id TEXT PRIMARY KEY, digest TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
];

// The candidates table is older than this module, and live organisations
// keep their tables, so its new columns are added to stores that have it.
const COLUMNS = [
  ["candidates", "app_id", "TEXT"],
  ["candidates", "audience", "TEXT"],
  ["candidates", "withdrawn_at", "INTEGER"],
  ["candidates", "withdrawn_by", "TEXT"],
  ["candidates", "withdraw_reason", "TEXT"],
];

/// Which app a candidate is a version of: its manifest's app id, or for a
/// candidate recorded before app ids were kept, its name.
export function appOf(c) {
  return String((c && c.app_id) || (c && c.name) || "").trim().toLowerCase();
}

const appArg = (raw) => String(raw || "").trim().toLowerCase().slice(0, APP_MAX);

function audienceOf(c) {
  if (!c || !c.audience) return null;
  try {
    const list = JSON.parse(c.audience);
    return Array.isArray(list) && list.length ? list : null;
  } catch (_) {
    // An audience that cannot be read admits nobody but the reviewers.
    return [];
  }
}

/// What a release can do, in the validator's plain words (IC-950, kept in
/// the `caps` column org-review.js adds): `{cap, words}` each. A release
/// recorded before that column has none to say.
function capabilityWords(c) {
  try {
    const list = JSON.parse((c && c.caps) || "[]");
    if (!Array.isArray(list)) return [];
    return list.filter((x) => x && x.cap).map((x) => ({ cap: String(x.cap), words: String(x.words || x.cap) }));
  } catch (_) {
    return [];
  }
}

/// "finance", "finance and ops", "finance, ops and legal".
function spoken(list) {
  if (list.length <= 1) return list.join("");
  return `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
}


const ops = {
  // ------------------------------------------------------------- helpers

  groupsOf(principal) {
    return new Set(this.rows("SELECT grp FROM group_members WHERE principal = ?", principal).map((r) => r.grp));
  },

  /// Which release of `app` this member gets, and why: `{digest, ring,
  /// group}`. Here, everyone gets the current one; staged rollout
  /// (org-rollout.js) adds a pilot group and pinned groups.
  versionFor(app, _me) {
    return { digest: this.currentOf(app), ring: "everyone", group: null };
  },

  /// Whether a released version is kept from this member by a staged
  /// rollout (org-rollout.js): never, here.
  rolloutHidden(_me, _c) {
    return false;
  },

  /// The refusal for a member outside a release's audience, or null.
  audienceRefusal(ctx, me, c, action, target) {
    const audience = audienceOf(c);
    if (!audience || SEES_ALL.has(me.role)) return null;
    const mine = this.groupsOf(me.principal);
    if (audience.some((g) => mine.has(g))) return null;
    const name = this.org().name || ctx.slug;
    const groups = audience.length ? `the ${spoken(audience)} group${audience.length === 1 ? "" : "s"}` : "some groups";
    const refusal = this.deny(ctx, action, target, "not-in-audience", `this app is only for ${groups} in ${name} -- ask an admin of ${name} to add you`);
    // Which groups, for the pages to say; the person is a member already.
    refusal.body.groups = audience;
    return refusal;
  },

  withdrawnRefusal(ctx, c, action, target) {
    const name = this.org().name || ctx.slug;
    const refusal = this.deny(
      ctx,
      action,
      target,
      "release-withdrawn",
      `${name} withdrew ${c.name || "this app"} ${c.version || ""} on ${this.orgDay(c.withdrawn_at || ctx.now)}: "${c.withdraw_reason || "no reason given"}" -- ask an admin of ${name} which version to use now`.replace(/ {2,}/g, " "),
    );
    refusal.body.withdrawn = { at: c.withdrawn_at, reason: c.withdraw_reason };
    return refusal;
  },

  /// The released versions of one app, newest approval first.
  releasedOf(app) {
    return this.rows("SELECT *, rowid FROM candidates WHERE state = 'released' ORDER BY approved_at DESC, rowid DESC").filter(
      (c) => appOf(c) === app,
    );
  },

  /// The release the app's link opens: the one chosen, while it is still
  /// released; otherwise the newest approved one; otherwise none.
  currentOf(app) {
    const chosen = this.row("SELECT digest FROM current_releases WHERE app_id = ?", app);
    if (chosen) {
      const c = this.row("SELECT * FROM candidates WHERE digest = ? AND state = 'released'", chosen.digest);
      if (c && appOf(c) === app) return c.digest;
    }
    const newest = this.releasedOf(app)[0];
    return newest ? newest.digest : null;
  },

  setCurrent(app, digest, actor, now) {
    if (digest) {
      this.sql.exec(
        `INSERT INTO current_releases (app_id, digest, set_at, set_by) VALUES (?, ?, ?, ?)
         ON CONFLICT (app_id) DO UPDATE SET digest = excluded.digest, set_at = excluded.set_at, set_by = excluded.set_by`,
        app,
        digest,
        now,
        actor,
      );
    } else {
      this.sql.exec("DELETE FROM current_releases WHERE app_id = ?", app);
    }
  },

  /// Offline access already issued for a digest and not yet run out: how
  /// many people hold it, and the last moment any of it holds.
  leasesOutstanding(digest, now) {
    let r = { n: 0, until: null };
    try {
      r = this.row(
        "SELECT COUNT(DISTINCT principal) AS n, MAX(expires_at) AS until FROM leases WHERE digest = ? AND expires_at > ?",
        digest,
        now,
      ) || r;
    } catch (_) {
      // No leases table: offline access is not part of this build.
    }
    return { active: r.n || 0, until: r.n ? r.until : null };
  },

  releaseView(c, base, opts = {}) {
    const audience = audienceOf(c);
    const out = {
      app_id: appOf(c),
      name: c.name,
      version: c.version,
      digest: c.digest,
      url: `${base || ""}/a/${c.digest}`,
      state: c.state,
      uploaded_by: c.uploaded_by,
      approved_by: c.approved_by,
      at: c.approved_at,
      audience: audience,
      size: Number(c.size) || 0,
      capabilities: capabilityWords(c),
    };
    if (typeof this.offlineClassFor === "function") out.offline = this.offlineClassFor(c.name);
    if (c.state === "withdrawn") {
      out.withdrawn_at = c.withdrawn_at;
      out.withdrawn_by = c.withdrawn_by;
      out.withdraw_reason = c.withdraw_reason;
    }
    if (opts.leases) out.leases = this.leasesOutstanding(c.digest, opts.now);
    return out;
  },

  /// The command's member, refused unless their role may do `action`.
  mayDo(ctx, action, target, words) {
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return { refusal };
    if (allows(me.role, action) !== "yes") {
      return { refusal: this.deny(ctx, action, target, "role-cannot", words(me.role)) };
    }
    return { me };
  },

  groupRow(name) {
    return this.row("SELECT * FROM groups WHERE name = ?", name);
  },

  // ------------------------------------------------------------ commands

  /// POST /orgs/<slug>/releases/<digest>/withdraw {reason}.
  op_withdraw(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `release:${digest}`;
    const { me, refusal } = this.mayDo(ctx, "release.withdraw", target, (r) => `a ${r} cannot withdraw ${ctx.slug}'s releases`);
    if (refusal) return refusal;
    const reason = String(ctx.args.reason == null ? "" : ctx.args.reason).trim();
    if (!reason || reason.length > REASON_MAX) return this.deny(ctx, "release.withdraw", target, "reason-required");
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn") {
      return this.deny(ctx, "release.withdraw", target, "release-withdrawn", `it was already withdrawn on ${this.orgDay(c.withdrawn_at)}`);
    }
    if (!c || c.state !== "released") {
      return this.deny(
        ctx,
        "release.withdraw",
        target,
        "digest-mismatch",
        c ? "that digest is waiting for approval, not released, so there is nothing to withdraw" : `no release with that digest in ${ctx.slug}`,
      );
    }
    const app = appOf(c);
    const wasCurrent = this.currentOf(app) === digest;
    this.sql.exec(
      "UPDATE candidates SET state = 'withdrawn', withdrawn_at = ?, withdrawn_by = ?, withdraw_reason = ? WHERE digest = ?",
      ctx.now,
      me.principal,
      reason,
      digest,
    );
    this.audit(me.principal, "release.withdraw", target, "allowed", reason);
    let current = this.currentOf(app);
    if (wasCurrent) {
      const next = this.releasedOf(app)[0];
      current = next ? next.digest : null;
      this.setCurrent(app, current, me.principal, ctx.now);
      this.audit(me.principal, "release.current", `app:${app}`, "allowed", `${current || "none"} after withdrawing ${digest}`);
    }
    const after = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    return this.ok({
      release: this.releaseView(after, ctx.args.base),
      current: { app_id: app, digest: current },
      leases: this.leasesOutstanding(digest, ctx.now),
    });
  },

  /// PUT /orgs/<slug>/apps/<app-id>/current {digest}: the rollback.
  op_set_current(ctx) {
    const app = appArg(ctx.args.app_id);
    const digest = String(ctx.args.digest || "").toLowerCase();
    const target = `app:${app}`;
    const { me, refusal } = this.mayDo(ctx, "release.set-current", target, (r) => `a ${r} cannot choose which release of an app is current`);
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn" && appOf(c) === app) return this.withdrawnRefusal(ctx, c, "release.set-current", target);
    if (!c || c.state !== "released" || appOf(c) !== app) {
      return this.deny(ctx, "release.set-current", target, "digest-mismatch", `that digest is not an approved release of ${app || "that app"} in ${ctx.slug}`);
    }
    if (this.currentOf(app) !== digest) {
      this.setCurrent(app, digest, me.principal, ctx.now);
      this.audit(me.principal, "release.current", target, "allowed", digest);
    }
    return this.ok({ app_id: app, current: this.releaseView(c, ctx.args.base) });
  },

  /// GET /orgs/<slug>/apps/<app-id>/current: which digest the app's link
  /// opens for this person, with the checks a download makes. A look, not
  /// a download: the download that follows is what is recorded.
  op_current_of(ctx) {
    const app = appArg(ctx.args.app_id);
    const target = `app:${app}`;
    const { me, refusal } = this.mayDo(ctx, "release.download", target, (r) => `a ${r} cannot open ${ctx.slug}'s releases`);
    if (refusal) return refusal;
    // What this person gets: a pin for one of their groups, the pilot if
    // they are in it, else the current release (org-rollout.js).
    const mine = this.versionFor(app, me);
    const digest = mine.digest;
    if (!digest) {
      // The last one withdrawn, in the audit's order (two in one second
      // have the same time).
      const withdrawn = this.rows(
        `SELECT c.* FROM audit a JOIN candidates c ON a.target = 'release:' || c.digest
         WHERE a.action = 'release.withdraw' AND a.result = 'allowed' AND c.state = 'withdrawn' ORDER BY a.seq DESC`,
      ).find((c) => appOf(c) === app);
      if (withdrawn) return this.withdrawnRefusal(ctx, withdrawn, "release.download", target);
      return this.deny(ctx, "release.download", target, "digest-mismatch", `${ctx.slug} has no approved release of ${app || "that app"}`);
    }
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    const outside = this.audienceRefusal(ctx, me, c, "release.download", target);
    if (outside) return outside;
    return this.ok({ app_id: app, digest, name: c.name, version: c.version, url: `${ctx.args.base || ""}/a/${digest}`, ring: mine.ring, group: mine.group });
  },

  /// GET /orgs/<slug>/apps: each app, its current release and its history.
  /// Owners, admins and reviewers see everything, withdrawn versions
  /// included. Anyone else sees the company's apps as they open them
  /// (IC-953): only apps whose current release is in their audience, and
  /// no withdrawn version. Each app says what its link opens now -- its
  /// version, size, what it can do (IC-950's words), offline class and the
  /// link -- so a device can list and open it in one look. Whoever may
  /// withdraw also sees the offline access each release holds out.
  op_app_releases(ctx) {
    const { me, refusal } = this.mayDo(ctx, "release.download", `org:${ctx.slug}`, (r) => `a ${r} cannot see ${ctx.slug}'s releases`);
    if (refusal) return refusal;
    const mine = this.groupsOf(me.principal);
    const everything = SEES_ALL.has(me.role);
    const visible = (c) => {
      const a = audienceOf(c);
      return (!a || everything || a.some((g) => mine.has(g))) && !this.rolloutHidden(me, c);
    };
    const leases = allows(me.role, "release.withdraw") === "yes";
    const byApp = new Map();
    const rows = this.rows(
      "SELECT *, rowid FROM candidates WHERE state IN ('released', 'withdrawn') ORDER BY approved_at DESC, rowid DESC",
    );
    for (const c of rows) {
      if (!visible(c)) continue;
      // A withdrawn version is history for the people who manage releases;
      // for everyone else it is gone.
      if (!everything && c.state !== "released") continue;
      const app = appOf(c);
      if (!byApp.has(app)) byApp.set(app, { app_id: app, name: c.name, current: null, releases: [] });
      byApp.get(app).releases.push(this.releaseView(c, ctx.args.base, { leases, now: ctx.now }));
    }
    const apps = [];
    for (const entry of byApp.values()) {
      // People who manage releases see what everyone gets, and what they
      // get themselves beside it; anyone else sees what they get.
      const theirs = this.versionFor(entry.app_id, me);
      const current = everything ? this.currentOf(entry.app_id) : theirs.digest;
      // The current release, only when this person may open it.
      entry.current = entry.releases.some((r) => r.digest === current) ? current : null;
      const forYou = entry.releases.some((r) => r.digest === theirs.digest) ? theirs.digest : null;
      entry.for_you = forYou ? { digest: forYou, ring: theirs.ring, group: theirs.group } : null;
      if (everything && typeof this.rolloutView === "function") entry.rollout = this.rolloutView(entry.app_id);
      // An app whose current release is not theirs to open is not on their
      // list at all, even when an older version was once for them.
      if (!everything && !entry.current) continue;
      const named = entry.releases.find((r) => r.state === "released") || entry.releases[0];
      entry.name = named.name;
      const now = entry.current ? entry.releases.find((r) => r.digest === entry.current) : null;
      Object.assign(entry, {
        version: now ? now.version : null,
        digest: now ? now.digest : null,
        size: now ? now.size : null,
        capabilities: now ? now.capabilities : [],
        offline: now ? now.offline || null : null,
        link: `${ctx.args.base || ""}/orgs/${ctx.slug}/apps/${encodeURIComponent(entry.app_id)}/current`,
      });
      apps.push(entry);
    }
    return this.ok({ apps });
  },

  /// PUT /orgs/<slug>/releases/<digest>/audience {groups} | {everyone: true}.
  /// A waiting candidate takes one too, so an app can be limited before
  /// its approval rather than after.
  op_audience_set(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `release:${digest}`;
    const { me, refusal } = this.mayDo(ctx, "release.audience", target, (r) => `a ${r} cannot choose who opens ${ctx.slug}'s releases`);
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn") return this.withdrawnRefusal(ctx, c, "release.audience", target);
    if (!c) return this.deny(ctx, "release.audience", target, "digest-mismatch", `no release or candidate with that digest in ${ctx.slug}`);
    let groups = null;
    if (ctx.args.everyone !== true) {
      const raw = Array.isArray(ctx.args.groups) ? ctx.args.groups : null;
      if (!raw || !raw.length || raw.length > 50) {
        return this.deny(ctx, "release.audience", target, "group-invalid", "name the groups (one or more), or give everyone: true");
      }
      groups = [...new Set(raw.map((g) => String(g || "").trim().toLowerCase()))].sort();
      const bad = groups.find((g) => !GROUP.test(g));
      if (bad !== undefined) return this.deny(ctx, "release.audience", target, "group-invalid");
      const missing = groups.find((g) => !this.groupRow(g));
      if (missing) return this.deny(ctx, "release.audience", target, "no-such-group", `${ctx.slug} has no group called ${missing} -- create it first`);
    }
    const before = audienceOf(c);
    const same = JSON.stringify(before) === JSON.stringify(groups);
    if (!same) {
      this.sql.exec("UPDATE candidates SET audience = ? WHERE digest = ?", groups ? JSON.stringify(groups) : null, digest);
      this.audit(me.principal, "release.audience", target, "allowed", groups ? `groups:${groups.join(",")}` : "everyone");
    }
    return this.ok({ digest, audience: groups });
  },

  /// GET /orgs/<slug>/groups. Members who see everyone see every group;
  /// a recipient sees the groups they are in, with only themselves.
  op_groups(ctx) {
    const { me, refusal } = this.standing(ctx, "group.list", `org:${ctx.slug}`);
    if (refusal) return refusal;
    const all = allows(me.role, "org.view") === "yes";
    const mine = this.groupsOf(me.principal);
    const groups = this.rows("SELECT * FROM groups ORDER BY name")
      .filter((g) => all || mine.has(g.name))
      .map((g) => ({
        name: g.name,
        created_at: g.created_at,
        created_by: g.created_by,
        members: all
          ? this.rows("SELECT principal FROM group_members WHERE grp = ? ORDER BY added_at, principal", g.name).map((r) => r.principal)
          : [me.principal],
      }));
    return this.ok({ groups });
  },

  /// POST /orgs/<slug>/groups {name}.
  op_group_create(ctx) {
    const name = String(ctx.args.name || "").trim().toLowerCase();
    const target = `group:${name.slice(0, 40)}`;
    const { me, refusal } = this.mayDo(ctx, "group.manage", target, (r) => `a ${r} cannot manage ${ctx.slug}'s groups`);
    if (refusal) return refusal;
    if (!GROUP.test(name)) return this.deny(ctx, "group.manage", target, "group-invalid");
    if (this.groupRow(name)) return this.deny(ctx, "group.manage", target, "group-taken", `${ctx.slug} already has a group called ${name}`);
    this.sql.exec("INSERT INTO groups (name, created_at, created_by) VALUES (?, ?, ?)", name, ctx.now, me.principal);
    this.audit(me.principal, "group.create", target, "allowed");
    return this.ok({ group: { name, created_at: ctx.now, created_by: me.principal, members: [] } }, 201);
  },

  /// PUT /orgs/<slug>/groups/<name>/members/<principal>: only an active
  /// member joins a group.
  op_group_add(ctx) {
    return this.groupChange(ctx, true);
  },

  /// DELETE /orgs/<slug>/groups/<name>/members/<principal>.
  op_group_remove(ctx) {
    return this.groupChange(ctx, false);
  },

  groupChange(ctx, adding) {
    const name = String(ctx.args.name || "").trim().toLowerCase();
    const p = parsePrincipal(ctx.args.principal);
    const who = p ? p.principal : String(ctx.args.principal || "").slice(0, 80);
    const target = `group:${name.slice(0, 40)}/${who}`;
    const action = adding ? "group.add" : "group.remove";
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return refusal;
    if (allows(me.role, "group.manage") !== "yes") {
      return this.deny(ctx, action, target, "role-cannot", `a ${me.role} cannot manage ${ctx.slug}'s groups`);
    }
    if (!this.groupRow(name)) return this.deny(ctx, action, target, "no-such-group", `${ctx.slug} has no group called ${name || "that"}`);
    if (!p) return this.deny(ctx, action, target, "not-a-member", "name the member by principal: github:<login> or email:<address>");
    const there = this.row("SELECT 1 AS y FROM group_members WHERE grp = ? AND principal = ?", name, who);
    if (adding) {
      const them = this.member(who);
      if (!them || them.state !== "active") {
        return this.deny(ctx, action, target, "not-a-member", `${displayOf(who)} is not a member of ${ctx.slug} -- invite them first`);
      }
      if (!there) {
        this.sql.exec("INSERT INTO group_members (grp, principal, added_at, added_by) VALUES (?, ?, ?, ?)", name, who, ctx.now, me.principal);
        this.audit(me.principal, "group.add", target, "allowed");
      }
      return this.ok({ group: name, principal: who, member: true });
    }
    if (there) {
      this.sql.exec("DELETE FROM group_members WHERE grp = ? AND principal = ?", name, who);
      this.audit(me.principal, "group.remove", target, "allowed");
    }
    return this.ok(null, 204);
  },
};

/// The checks every download, look and lease of a digest makes, before
/// the command's own: standing first (so a stranger learns nothing about
/// a digest), then withdrawn, then the audience.
function guarded(original, action) {
  return function (ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `release:${digest}`;
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn") return this.withdrawnRefusal(ctx, c, action, target);
    if (c && c.state === "released") {
      const outside = this.audienceRefusal(ctx, me, c, action, target);
      if (outside) return outside;
    }
    return original.call(this, ctx);
  };
}

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({
    schema: SCHEMA,
    columns: COLUMNS,
    mutations: ["withdraw", "set-current", "audience-set", "group-create", "group-add", "group-remove"],
  });
  const P = OrgClass.prototype;

  P.op_download = guarded(P.op_download, "release.download");
  P.op_view = guarded(P.op_view, "release.download");
  // Loud, not skipped: an unwrapped lease would keep renewing offline access
  // to a withdrawn release.
  if (typeof P.op_lease !== "function") throw new Error("org-releases: the lease command is not installed");
  P.op_lease = guarded(P.op_lease, "lease.issue");

  // A candidate keeps the app id its manifest names.
  const addCandidate = P.op_add_candidate;
  P.op_add_candidate = function (ctx) {
    const answer = addCandidate.call(this, ctx);
    const app = appArg(ctx.args.app_id);
    if (answer.status === 201 && app) {
      this.sql.exec("UPDATE candidates SET app_id = ? WHERE digest = ? AND app_id IS NULL", app, String(ctx.args.digest || ""));
    }
    return answer;
  };

  // A withdrawn digest stays withdrawn: approving it again is refused. A
  // new approval becomes its app's current release.
  const approve = P.op_approve;
  P.op_approve = function (ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const before = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (before && before.state === "withdrawn") {
      const { refusal } = this.standing(ctx, "candidate.approve", target);
      if (refusal) return refusal;
      return this.deny(
        ctx,
        "candidate.approve",
        target,
        "release-withdrawn",
        "these bytes were withdrawn, and a withdrawal is not undone -- send a new version for approval instead",
      );
    }
    const answer = approve.call(this, ctx);
    if (answer.status === 200 && before && before.state !== "released") {
      const after = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
      if (after && after.state === "released") this.setCurrent(appOf(after), digest, ctx.actor, ctx.now);
    }
    return answer;
  };

  // The releases list: only what this person may open, each with its app
  // id, whether it is current, and its audience.
  const releases = P.op_releases;
  P.op_releases = function (ctx) {
    const answer = releases.call(this, ctx);
    if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
    const me = this.member(ctx.actor);
    const mine = me ? this.groupsOf(me.principal) : new Set();
    const current = new Map();
    answer.body = answer.body
      .map((r) => ({ r, c: this.row("SELECT * FROM candidates WHERE digest = ?", r.digest) }))
      .filter(({ c }) => {
        const a = audienceOf(c);
        return (!a || (me && SEES_ALL.has(me.role)) || a.some((g) => mine.has(g))) && !(me && this.rolloutHidden(me, c));
      })
      .map(({ r, c }) => {
        const app = appOf(c);
        if (!current.has(app)) current.set(app, this.currentOf(app));
        return { ...r, app_id: app, current: current.get(app) === r.digest, audience: audienceOf(c) };
      });
    return answer;
  };

  // The candidates list carries the audience too, for the picker.
  const candidates = P.op_candidates;
  P.op_candidates = function (ctx) {
    const answer = candidates.call(this, ctx);
    if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
    answer.body = answer.body.map((r) => {
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", r.digest);
      const out = { ...r, app_id: appOf(c), audience: audienceOf(c) };
      if (c && c.state === "withdrawn") Object.assign(out, { withdrawn_at: c.withdrawn_at, withdrawn_by: c.withdrawn_by, withdraw_reason: c.withdraw_reason });
      return out;
    });
    return answer;
  };

  // Someone removed leaves every group: if they are invited back, they
  // start in none, rather than finding old audiences open to them again.
  const remove = P.op_remove;
  P.op_remove = function (ctx) {
    const who = this.targetOf(ctx);
    const answer = remove.call(this, ctx);
    if (answer.status === 204 && who) {
      for (const g of this.rows("SELECT grp FROM group_members WHERE principal = ?", who)) {
        this.sql.exec("DELETE FROM group_members WHERE grp = ? AND principal = ?", g.grp, who);
        this.audit(ctx.actor, "group.remove", `group:${g.grp}/${who}`, "allowed", "member removed");
      }
    }
    return answer;
  };
}

// Importing this module teaches the store release control and groups. It
// is imported after org-lease.js, whose lease command it guards.
install(Org);

// ---------------------------------------------------------------- routes

const DIGEST = /^[0-9a-f]{64}$/;

/// /orgs/<slug>/releases/<digest>/{withdraw,audience}, /orgs/<slug>/apps,
/// /orgs/<slug>/apps/<app-id>/current and /orgs/<slug>/groups/...
/// Answers a Response, or null.
export async function orgReleaseRoute(h) {
  const { request, url, env, rest, method, askRaw, mutateRaw, orgAnswer, json, base, fetchBundle } = h;
  const notDigest = () => orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the release's 64-hex sha256" } });

  if (rest[0] === "releases" && rest.length === 3) {
    const digest = String(rest[1]).toLowerCase();
    if (rest[2] === "withdraw" && method === "POST") {
      if (!DIGEST.test(digest)) return notDigest();
      return orgAnswer(await mutateRaw("withdraw", (b) => ({ digest, reason: b.reason, base })));
    }
    if (rest[2] === "audience" && method === "PUT") {
      if (!DIGEST.test(digest)) return notDigest();
      return orgAnswer(await mutateRaw("audience-set", (b) => ({ digest, groups: b.groups, everyone: b.everyone })));
    }
  }
  if (rest[0] === "apps" && rest.length === 1 && method === "GET") return orgAnswer(await askRaw("app-releases", { base }));
  if (rest[0] === "apps" && rest.length === 3 && rest[2] === "current") {
    const app = rest[1];
    if (method === "PUT") {
      return orgAnswer(await mutateRaw("set-current", (b) => ({ app_id: app, digest: String(b.digest || "").toLowerCase(), base })));
    }
    if (method === "GET" || method === "HEAD") {
      // The organisation's link for an app: what it opens now. JSON says
      // which release that is; anything else gets the bytes, with every
      // check /a/<digest> makes.
      const answer = await askRaw("current-of", { app_id: app, base });
      if (answer.status !== 200) return orgAnswer(answer);
      if ((request.headers.get("accept") || "").includes("application/json")) {
        return json(answer.body);
      }
      return fetchBundle(request, url, answer.body.digest, env);
    }
  }
  if (rest[0] === "groups") {
    if (rest.length === 1 && method === "GET") return orgAnswer(await askRaw("groups"));
    if (rest.length === 1 && method === "POST") return orgAnswer(await mutateRaw("group-create", (b) => ({ name: b.name })));
    if (rest.length === 4 && rest[2] === "members" && (method === "PUT" || method === "DELETE")) {
      const args = () => ({ name: rest[1], principal: rest[3] });
      return orgAnswer(await mutateRaw(method === "PUT" ? "group-add" : "group-remove", args));
    }
  }
  return null;
}
