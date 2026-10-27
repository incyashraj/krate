// Staged rollout (G10): a pilot group first, then everyone; a group pinned
// to one version.
//
// Each app id still has one current release, the one everyone gets
// (org-releases.js). On top of it:
//
// - **A pilot group** per app (an existing group). While it is set, a newly
//   approved version goes to that group only: the people in it get it, and
//   everyone else stays on the current release. "Release to everyone" makes
//   it current. An approved version can also be put into the pilot by hand.
//   The first version of an app with a pilot group goes to the pilot too:
//   until it is released to everyone, nobody else has the app.
// - **A pin**: a group kept on one approved version (finance stays on
//   2.0.2), whatever is current.
//
// What a person gets, everywhere it is decided (the app's link, the
// listing, the check-in, a download, a lease and its renewal, team data,
// a connection): a pin for one of their groups (the first by group name,
// when two of their groups are pinned), else the pilot version if they are
// in the pilot group, else the current release. A version in the pilot is
// refused to anyone outside it (`in-pilot`, 403), except to the people who
// review releases (owners, admins, reviewers) and a group pinned to it.
//
// Withdrawing a version the pilot or a pin points to ends that pilot or
// pin; its people fall back to what they would get without it, and the
// answer says so. Every change is a command in the organisation's store,
// audited, and in the role matrix (`release.rollout`, owners and admins:
// the people who choose which release is current).

import { Org, MATRIX, extendOrg, addReasons, allows } from "./org.js";
import { appOf, GROUP, SEES_ALL } from "./org-releases.js";

MATRIX["release.rollout"] = { owner: "yes", admin: "yes" };

const APP_MAX = 120;
const appArg = (raw) => String(raw || "").trim().toLowerCase().slice(0, APP_MAX);

addReasons({
  "in-pilot": {
    status: 403,
    message: (o, d) => d || `this version is with a pilot group in ${o} first -- you get it when it is released to everyone`,
  },
  "pilot-out": {
    status: 409,
    message: (o, d) => d || `a version is still with the pilot group in ${o} -- release it to everyone or withdraw it first`,
  },
  "no-pilot": {
    status: 404,
    message: (o, d) => d || `no version of that app is with a pilot group in ${o}`,
  },
});

const SCHEMA = [
  // One row per app that tries new versions with a group first. `digest`
  // is the version with the pilot now, or null between pilots.
  `CREATE TABLE IF NOT EXISTS rollouts (
     app_id TEXT PRIMARY KEY, grp TEXT NOT NULL, digest TEXT,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS release_pins (
     app_id TEXT NOT NULL, grp TEXT NOT NULL, digest TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL,
     PRIMARY KEY (app_id, grp))`,
];

const ops = {
  // ------------------------------------------------------------- helpers

  rolloutRow(app) {
    return this.row("SELECT * FROM rollouts WHERE app_id = ?", app);
  },

  releasedRow(digest, app) {
    const c = digest ? this.row("SELECT * FROM candidates WHERE digest = ?", digest) : null;
    return c && c.state === "released" && appOf(c) === app ? c : null;
  },

  /// The version with the pilot group now, while it is released; or null.
  pilotDigestOf(app) {
    const r = this.rolloutRow(app);
    return r && r.digest && this.releasedRow(r.digest, app) ? r.digest : null;
  },

  /// Every digest that is with a pilot group now, in one look.
  pilotDigests() {
    const out = new Set();
    for (const r of this.rows("SELECT app_id, digest FROM rollouts WHERE digest IS NOT NULL")) {
      if (this.releasedRow(r.digest, r.app_id)) out.add(r.digest);
    }
    return out;
  },

  /// The pins of one app that still point at a released version, by group.
  pinsOf(app) {
    return this.rows("SELECT * FROM release_pins WHERE app_id = ? ORDER BY grp", app).filter((p) => this.releasedRow(p.digest, app));
  },

  /// How many active members a group has.
  groupPeople(name) {
    return this.row(
      "SELECT COUNT(*) AS n FROM group_members g JOIN members m ON m.principal = g.principal AND m.state = 'active' WHERE g.grp = ?",
      name,
    ).n;
  },

  versionFor(app, me) {
    const groups = me ? this.groupsOf(me.principal) : new Set();
    const pin = this.pinsOf(app).find((p) => groups.has(p.grp));
    if (pin) return { digest: pin.digest, ring: "pin", group: pin.grp };
    const r = this.rolloutRow(app);
    const pilot = this.pilotDigestOf(app);
    if (pilot && groups.has(r.grp)) return { digest: pilot, ring: "pilot", group: r.grp };
    return { digest: this.currentOf(app), ring: "everyone", group: null };
  },

  rolloutHidden(me, c) {
    if (!c || c.state !== "released" || !me) return false;
    const app = appOf(c);
    if (this.pilotDigestOf(app) !== c.digest) return false;
    if (SEES_ALL.has(me.role)) return false;
    const groups = this.groupsOf(me.principal);
    if (groups.has(this.rolloutRow(app).grp)) return false;
    return !this.pinsOf(app).some((p) => p.digest === c.digest && groups.has(p.grp));
  },

  versionWords(digest) {
    const c = digest ? this.row("SELECT name, version FROM candidates WHERE digest = ?", digest) : null;
    return c ? { digest, version: c.version || null, name: c.name || null } : null;
  },

  /// What the people who manage releases read about an app's rings.
  rolloutView(app) {
    const r = this.rolloutRow(app);
    const pilot = this.pilotDigestOf(app);
    return {
      everyone: this.versionWords(this.currentOf(app)),
      pilot: r ? { group: r.grp, people: this.groupPeople(r.grp), version: pilot ? this.versionWords(pilot) : null, set_at: r.set_at, set_by: r.set_by } : null,
      pins: this.pinsOf(app).map((p) => ({ group: p.grp, people: this.groupPeople(p.grp), version: this.versionWords(p.digest), set_at: p.set_at, set_by: p.set_by })),
    };
  },

  rolloutMay(ctx, action, target) {
    return this.mayDo(ctx, action, target, (r) => `a ${r} cannot choose who gets which version of ${ctx.slug}'s apps`);
  },

  groupArg(ctx, action, target, raw) {
    const name = String(raw || "").trim().toLowerCase();
    if (!GROUP.test(name)) return { refusal: this.deny(ctx, action, target, "group-invalid") };
    if (!this.groupRow(name)) return { refusal: this.deny(ctx, action, target, "no-such-group", `${ctx.slug} has no group called ${name} -- make it on People first`) };
    return { name };
  },

  // ------------------------------------------------------------ commands

  /// PUT /orgs/<slug>/apps/<app-id>/pilot {group, digest?}: new versions of
  /// the app go to this group first. With a digest, that approved version
  /// goes to the group now.
  "rollout-set"(ctx) {
    const app = appArg(ctx.args.app_id);
    const target = `app:${app}`;
    const { me, refusal } = this.rolloutMay(ctx, "release.rollout", target);
    if (refusal) return refusal;
    const g = this.groupArg(ctx, "release.rollout", target, ctx.args.group);
    if (g.refusal) return g.refusal;
    const before = this.rolloutRow(app);
    let digest = before ? this.pilotDigestOf(app) : null;
    if (ctx.args.digest) {
      const want = String(ctx.args.digest).toLowerCase();
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", want);
      if (c && c.state === "withdrawn" && appOf(c) === app) return this.withdrawnRefusal(ctx, c, "release.rollout", target);
      if (!this.releasedRow(want, app)) {
        return this.deny(ctx, "release.rollout", target, "digest-mismatch", `that is not an approved version of ${app || "that app"} in ${ctx.slug}`);
      }
      if (this.currentOf(app) === want) {
        return this.deny(ctx, "release.rollout", target, "digest-mismatch", `${c.version || "that version"} is already the version everyone gets`);
      }
      digest = want;
    }
    this.sql.exec(
      `INSERT INTO rollouts (app_id, grp, digest, set_at, set_by) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT (app_id) DO UPDATE SET grp = excluded.grp, digest = excluded.digest, set_at = excluded.set_at, set_by = excluded.set_by`,
      app,
      g.name,
      digest,
      ctx.now,
      me.principal,
    );
    if (!before || before.grp !== g.name || (before.digest || null) !== (digest || null)) {
      this.audit(me.principal, "release.pilot", target, "allowed", `group:${g.name}${digest ? ` ${digest}` : ""}`);
    }
    return this.ok({ app_id: app, rollout: this.rolloutView(app) });
  },

  /// DELETE /orgs/<slug>/apps/<app-id>/pilot: new versions go to everyone
  /// again. Refused while a version is still with the pilot.
  "rollout-clear"(ctx) {
    const app = appArg(ctx.args.app_id);
    const target = `app:${app}`;
    const { me, refusal } = this.rolloutMay(ctx, "release.rollout", target);
    if (refusal) return refusal;
    const r = this.rolloutRow(app);
    if (!r) return this.ok(null, 204);
    const pilot = this.pilotDigestOf(app);
    if (pilot) {
      const v = this.versionWords(pilot);
      return this.deny(ctx, "release.rollout", target, "pilot-out", `${v.version || "a version"} is still with the ${r.grp} group -- release it to everyone or withdraw it first`);
    }
    this.sql.exec("DELETE FROM rollouts WHERE app_id = ?", app);
    this.audit(me.principal, "release.pilot.off", target, "allowed", `group:${r.grp}`);
    return this.ok(null, 204);
  },

  /// POST /orgs/<slug>/apps/<app-id>/pilot/release: "Release to everyone".
  /// The pilot's version becomes current; the group stays the pilot for
  /// the next version.
  "rollout-release"(ctx) {
    const app = appArg(ctx.args.app_id);
    const target = `app:${app}`;
    const { me, refusal } = this.rolloutMay(ctx, "release.rollout", target);
    if (refusal) return refusal;
    const pilot = this.pilotDigestOf(app);
    if (!pilot) return this.deny(ctx, "release.rollout", target, "no-pilot");
    const was = this.currentOf(app);
    this.setCurrent(app, pilot, me.principal, ctx.now);
    this.sql.exec("UPDATE rollouts SET digest = NULL, set_at = ?, set_by = ? WHERE app_id = ?", ctx.now, me.principal, app);
    this.audit(me.principal, "release.pilot.release", target, "allowed", pilot);
    this.audit(me.principal, "release.current", target, "allowed", `${pilot} from the pilot${was ? `, after ${was}` : ""}`);
    return this.ok({ app_id: app, current: this.versionWords(pilot), rollout: this.rolloutView(app) });
  },

  /// PUT /orgs/<slug>/apps/<app-id>/pins/<group> {digest}: keep a group on
  /// one approved version.
  "pin-set"(ctx) {
    const app = appArg(ctx.args.app_id);
    const g0 = String(ctx.args.group || "").trim().toLowerCase().slice(0, 40);
    const target = `app:${app}/${g0}`;
    const { me, refusal } = this.rolloutMay(ctx, "release.rollout", target);
    if (refusal) return refusal;
    const g = this.groupArg(ctx, "release.rollout", target, g0);
    if (g.refusal) return g.refusal;
    const digest = String(ctx.args.digest || "").toLowerCase();
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn" && appOf(c) === app) return this.withdrawnRefusal(ctx, c, "release.rollout", target);
    if (!this.releasedRow(digest, app)) {
      return this.deny(ctx, "release.rollout", target, "digest-mismatch", `that is not an approved version of ${app || "that app"} in ${ctx.slug}`);
    }
    const before = this.row("SELECT digest FROM release_pins WHERE app_id = ? AND grp = ?", app, g.name);
    if (!before || before.digest !== digest) {
      this.sql.exec(
        `INSERT INTO release_pins (app_id, grp, digest, set_at, set_by) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (app_id, grp) DO UPDATE SET digest = excluded.digest, set_at = excluded.set_at, set_by = excluded.set_by`,
        app,
        g.name,
        digest,
        ctx.now,
        me.principal,
      );
      this.audit(me.principal, "release.pin", target, "allowed", digest);
    }
    return this.ok({ app_id: app, rollout: this.rolloutView(app) });
  },

  /// DELETE /orgs/<slug>/apps/<app-id>/pins/<group>.
  "pin-clear"(ctx) {
    const app = appArg(ctx.args.app_id);
    const g = String(ctx.args.group || "").trim().toLowerCase().slice(0, 40);
    const target = `app:${app}/${g}`;
    const { me, refusal } = this.rolloutMay(ctx, "release.rollout", target);
    if (refusal) return refusal;
    const there = this.row("SELECT digest FROM release_pins WHERE app_id = ? AND grp = ?", app, g);
    if (there) {
      this.sql.exec("DELETE FROM release_pins WHERE app_id = ? AND grp = ?", app, g);
      this.audit(me.principal, "release.unpin", target, "allowed", there.digest);
    }
    return this.ok(null, 204);
  },
};

function install(OrgClass) {
  const helpers = {};
  const commands = {};
  for (const [k, fn] of Object.entries(ops)) (k.includes("-") ? commands : helpers)[k] = fn;
  // The helpers replace org-releases.js's defaults (everyone gets current).
  Object.assign(OrgClass.prototype, helpers);
  extendOrg({ schema: SCHEMA, ops: commands, mutations: Object.keys(commands) });
  const P = OrgClass.prototype;

  // A version with the pilot is not "the newest approved" for everyone:
  // not when nothing is chosen, and not when the current one is withdrawn.
  const releasedOf = P.releasedOf;
  P.releasedOf = function (app) {
    const pilot = this.pilotDigestOf(app);
    const list = releasedOf.call(this, app);
    return pilot ? list.filter((c) => c.digest !== pilot) : list;
  };

  // The pilot is refused to anyone outside it, wherever a release is
  // opened: download, check-in, lease, team data, connections.
  const audienceRefusal = P.audienceRefusal;
  P.audienceRefusal = function (ctx, me, c, action, target) {
    const outside = audienceRefusal.call(this, ctx, me, c, action, target);
    if (outside) return outside;
    if (!this.rolloutHidden(me, c)) return null;
    const r = this.rolloutRow(appOf(c));
    const name = this.org().name || ctx.slug;
    const refusal = this.deny(
      ctx,
      action,
      target,
      "in-pilot",
      `${c.name || "this app"} ${c.version || ""} is with the ${r.grp} group in ${name} first -- you get it when it is released to everyone`.replace(/ {2,}/g, " "),
    );
    const now = this.versionWords(this.versionFor(appOf(c), me).digest);
    if (now) refusal.body.for_you = now;
    return refusal;
  };

  // A new approval of an app with a pilot group goes to that group; the
  // version everyone gets stays where it was.
  const approve = P.op_approve;
  P.op_approve = function (ctx) {
    const digest = String(ctx.args.digest || "");
    const before = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    const app = before ? appOf(before) : "";
    const r = app ? this.rolloutRow(app) : null;
    const was = r ? this.currentOf(app) : null;
    const answer = approve.call(this, ctx);
    if (!r || answer.status !== 200 || !before || before.state === "released") return answer;
    const after = this.row("SELECT state FROM candidates WHERE digest = ?", digest);
    if (!after || after.state !== "released") return answer;
    this.setCurrent(app, was, ctx.actor, ctx.now);
    this.sql.exec("UPDATE rollouts SET digest = ? WHERE app_id = ?", digest, app);
    this.audit(ctx.actor, "release.pilot", `app:${app}`, "allowed", `group:${r.grp} ${digest} approved`);
    if (answer.body && typeof answer.body === "object") {
      answer.body.pilot = { group: r.grp, people: this.groupPeople(r.grp), everyone: this.versionWords(was) };
    }
    return answer;
  };

  // Withdrawing a version the pilot or a pin points to ends that pilot or
  // pin, and says what its people get now.
  const withdraw = P.op_withdraw;
  P.op_withdraw = function (ctx) {
    const digest = String(ctx.args.digest || "");
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    const app = c ? appOf(c) : "";
    const pilotRow = app && this.pilotDigestOf(app) === digest ? this.rolloutRow(app) : null;
    const pins = app ? this.pinsOf(app).filter((p) => p.digest === digest) : [];
    const answer = withdraw.call(this, ctx);
    if (answer.status !== 200) return answer;
    const fallback = [];
    if (pilotRow) {
      this.sql.exec("UPDATE rollouts SET digest = NULL WHERE app_id = ?", app);
      this.audit(ctx.actor, "release.pilot", `app:${app}`, "allowed", `ended: ${digest} withdrawn`);
    }
    for (const p of pins) {
      this.sql.exec("DELETE FROM release_pins WHERE app_id = ? AND grp = ?", app, p.grp);
      this.audit(ctx.actor, "release.unpin", `app:${app}/${p.grp}`, "allowed", `${digest} withdrawn`);
    }
    // What each group gets now, the way versionFor decides it for a member
    // of only that group.
    const now = (grp) => {
      const pin = this.pinsOf(app).find((x) => x.grp === grp);
      if (pin) return this.versionWords(pin.digest);
      const r = this.rolloutRow(app);
      const pilot = this.pilotDigestOf(app);
      if (pilot && r.grp === grp) return this.versionWords(pilot);
      return this.versionWords(this.currentOf(app));
    };
    if (pilotRow) fallback.push({ group: pilotRow.grp, was: "pilot", now: now(pilotRow.grp) });
    for (const p of pins) fallback.push({ group: p.grp, was: "pin", now: now(p.grp) });
    if (fallback.length) {
      const v = c.version || "it";
      answer.body.fallback = fallback;
      answer.body.fallback_words = fallback
        .map((f) => `${f.was === "pilot" ? `The ${f.group} group had ${v} as its pilot` : `The ${f.group} group was kept on ${v}`}; ${f.now ? `it gets ${f.now.version || "the current version"} now` : "nothing of the app opens for it now"}.`)
        .join(" ");
    }
    return answer;
  };

  // Making the pilot's version current by hand is releasing it to everyone.
  const setCurrent = P.op_set_current;
  P.op_set_current = function (ctx) {
    const app = appArg(ctx.args.app_id);
    const pilot = app ? this.pilotDigestOf(app) : null;
    const answer = setCurrent.call(this, ctx);
    if (answer.status === 200 && pilot && pilot === String(ctx.args.digest || "").toLowerCase()) {
      this.sql.exec("UPDATE rollouts SET digest = NULL WHERE app_id = ?", app);
      this.audit(ctx.actor, "release.pilot.release", `app:${app}`, "allowed", pilot);
    }
    return answer;
  };

  // The app's link, for someone outside the pilot of an app whose only
  // version is with the pilot: say that, rather than "no release".
  const currentOfOp = P.op_current_of;
  P.op_current_of = function (ctx) {
    const app = appArg(ctx.args.app_id);
    const me = this.org() ? this.member(ctx.actor) : null;
    const pilot = app && me && me.state === "active" ? this.pilotDigestOf(app) : null;
    if (pilot && allows(me.role, "release.download") === "yes" && !this.versionFor(app, me).digest) {
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", pilot);
      const name = this.org().name || ctx.slug;
      return this.deny(
        ctx,
        "release.download",
        `app:${app}`,
        "in-pilot",
        `${c.name || "this app"} is with the ${this.rolloutRow(app).grp} group in ${name} first -- you get it when it is released to everyone`,
      );
    }
    return currentOfOp.call(this, ctx);
  };

  // The check-in says which version this person should have now, so a
  // computer on the wrong one hears it.
  const view = P.op_view;
  P.op_view = function (ctx) {
    const answer = view.call(this, ctx);
    if (answer.status !== 200 || !answer.body) return answer;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", String(ctx.args.digest || ""));
    const me = this.member(ctx.actor);
    if (!c || !me) return answer;
    const mine = this.versionFor(appOf(c), me);
    // Only when it is another version: a check-in on the right one hears nothing new.
    if (mine.digest && mine.digest !== c.digest) answer.body.for_you = { ...this.versionWords(mine.digest), ring: mine.ring, group: mine.group };
    return answer;
  };

  // The releases list marks the pilot's version.
  const releases = P.op_releases;
  P.op_releases = function (ctx) {
    const answer = releases.call(this, ctx);
    if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
    const pilots = this.pilotDigests();
    answer.body = answer.body.map((r) => (pilots.has(r.digest) ? { ...r, pilot: true } : r));
    return answer;
  };
}

install(Org);

// ---------------------------------------------------------------- routes

/// /orgs/<slug>/apps/<app-id>/pilot[/release] and
/// /orgs/<slug>/apps/<app-id>/pins/<group>. Answers a Response, or null.
export async function orgRolloutRoute(h) {
  const { rest, method, mutateRaw, orgAnswer } = h;
  if (rest[0] !== "apps" || rest.length < 3) return null;
  const app = rest[1];
  if (rest[2] === "pilot" && rest.length === 3) {
    if (method === "PUT") return orgAnswer(await mutateRaw("rollout-set", (b) => ({ app_id: app, group: b.group, digest: b.digest })));
    if (method === "DELETE") return orgAnswer(await mutateRaw("rollout-clear", () => ({ app_id: app })));
  }
  if (rest[2] === "pilot" && rest.length === 4 && rest[3] === "release" && method === "POST") {
    return orgAnswer(await mutateRaw("rollout-release", () => ({ app_id: app })));
  }
  if (rest[2] === "pins" && rest.length === 4) {
    if (method === "PUT") return orgAnswer(await mutateRaw("pin-set", (b) => ({ app_id: app, group: rest[3], digest: b.digest })));
    if (method === "DELETE") return orgAnswer(await mutateRaw("pin-clear", () => ({ app_id: app, group: rest[3] })));
  }
  return null;
}
