// Insights (IC-959, ORG-FIRST-PLAN.md "IC-959 contract", F-318): each
// person, by role, sees the numbers their decisions need, in one answer.
//
// Built only from what the hub already sees -- never what happens inside
// an app, never team data (F-316). Most counts already exist in the
// organisation's own store, and are read from where they are kept:
//
// - opens: usage_daily (org-inventory.js), one row per person per release
//   per app per day;
// - releases, waits, withdrawals: the candidates table;
// - offline access still held: leases (org-lease.js);
// - team data: org_data_apps (bytes), org_data_daily and org_data_days
//   (writes; org-data.js);
// - connection hand-outs: connection_uses (org-connections.js);
// - devices holding the organisation key: key_devices and key_envelopes;
// - members, roles, sign-in, invitations, seats, SCIM tokens, the audit
//   export's delivery: their own tables.
//
// What was missing is added here, pre-aggregated per day and kept 90 days,
// so a read never walks raw events:
//
// - people_daily: who opened any of the organisation's apps on a day (the
//   org-wide distinct count, which per-app rows cannot add up to);
// - app_daily: how many people opened each app on a day;
// - insights_daily: refusals by reason (and app), rollbacks, SCIM requests
//   by outcome -- counters, never who;
// - member_seen: the last day each member did anything here;
// - key_devices.last_seen: the last time a device looked at the keyring.
//
// Rules (the contract): names of people only to owners and admins; a
// count of 1 or 2 people reads "<3" to anyone else (0 is shown: it names
// nobody); every section is chosen by the role matrix (org.js MATRIX,
// insights.*).

import { Org, extendOrg, allows, displayOf, parsePrincipal } from "./org.js";
import { appOf } from "./org-releases.js";
import { changesOf } from "./org-review.js";
import "./org-inventory.js";
import "./org-lease.js";
import "./org-keys.js";
import { quotaOf } from "./org-data.js";
import "./org-connections.js";
import "./org-sso.js";
import "./org-saml.js";
import "./org-scim.js";
import "./org-audit-export.js";
import { seatsUsed, billingState, freeSeats } from "./org-lifecycle-store.js";
// Days are the organisation's own (org-time.js, F-319).
import { dayBefore } from "./org-time.js";

export const KEEP_DAYS = 90;
export const SILENT_DAYS = 60;
export const INACTIVE_DAYS = 30;
const DAY = 86400;
/// What a count of 1 or 2 people reads as, for anyone but owners and admins.
export const FEWER = "<3";
/// A week's refusals are a spike when there are at least this many and
/// more than twice the weekly average of the four weeks before.
export const SPIKE_MIN = 10;


/// The refusals that stop someone using an app, counted against that app.
const OPENING = /^(release\.download|lease\.issue|data\.|connection\.use)/;

/// The SCIM commands that are the company directory talking to Krate.
const DIRECTORY_OPS = new Set([
  "scim-users-list",
  "scim-user-get",
  "scim-user-create",
  "scim-user-update",
  "scim-user-delete",
  "scim-groups-list",
  "scim-group-get",
  "scim-group-create",
  "scim-group-update",
  "scim-group-delete",
]);

/// The sections a role reads, in order. The matrix decides; this only
/// orders them.
export const SECTION_ORDER = ["org", "developer", "reviewer", "yours"];
export function sectionsFor(role) {
  const out = [];
  if (allows(role, "insights.org") === "yes") out.push("org");
  if (allows(role, "insights.developer") === "yes") out.push("developer");
  if (allows(role, "insights.reviewer") === "yes") out.push("reviewer");
  if (allows(role, "insights.view") === "yes") out.push("yours");
  return out;
}

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS people_daily (
     day TEXT NOT NULL, principal TEXT NOT NULL, PRIMARY KEY (day, principal))`,
  `CREATE TABLE IF NOT EXISTS app_daily (
     day TEXT NOT NULL, app_id TEXT NOT NULL, people INTEGER NOT NULL,
     PRIMARY KEY (day, app_id))`,
  `CREATE TABLE IF NOT EXISTS insights_daily (
     day TEXT NOT NULL, metric TEXT NOT NULL, key TEXT NOT NULL, n INTEGER NOT NULL,
     PRIMARY KEY (day, metric, key))`,
  `CREATE TABLE IF NOT EXISTS member_seen (principal TEXT PRIMARY KEY, day TEXT NOT NULL)`,
  // One app's opens, read by app then day (the developer's section).
  `CREATE INDEX IF NOT EXISTS usage_by_app ON usage_daily (app_id, day)`,
  // One person's opens (their own section).
  `CREATE INDEX IF NOT EXISTS usage_by_person ON usage_daily (principal, day)`,
];

const COLUMNS = [["key_devices", "last_seen", "INTEGER"]];

function median(list) {
  if (!list.length) return null;
  const s = [...list].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : Math.round((s[m - 1] + s[m]) / 2);
}

function parsed(text, fallback) {
  try {
    const v = JSON.parse(text || "null");
    return v == null ? fallback : v;
  } catch (_) {
    return fallback;
  }
}

const ops = {
  // ------------------------------------------------------------ counting

  insightsMeta(key) {
    const r = this.row("SELECT value FROM inventory_meta WHERE key = ?", key);
    return r ? r.value : null;
  },

  insightsSetMeta(key, value) {
    this.sql.exec(
      "INSERT INTO inventory_meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
      key,
      String(value),
    );
  },

  /// When the insights' own counts began: before it, "never seen" means
  /// "not counted yet", so nobody is flagged for it.
  insightsSince() {
    return Number(this.insightsMeta("insights_since")) || 0;
  },

  insightsCount(day, metric, key, by = 1) {
    this.sql.exec(
      `INSERT INTO insights_daily (day, metric, key, n) VALUES (?, ?, ?, ?)
       ON CONFLICT (day, metric, key) DO UPDATE SET n = n + excluded.n`,
      day,
      metric,
      String(key || ""),
      by,
    );
  },

  /// The app a refusal was about, when its target or arguments name one.
  insightsAppOf(ctx, target) {
    const t = String(target || "");
    const at = t.indexOf(":");
    const kind = at < 0 ? t : t.slice(0, at);
    const value = at < 0 ? "" : t.slice(at + 1);
    if (kind === "app") return value;
    let digest = kind === "release" || kind === "candidate" ? value : "";
    if (!digest && ctx.args && /^[0-9a-f]{64}$/.test(String(ctx.args.digest || ""))) digest = String(ctx.args.digest);
    if (!digest) return "";
    const c = this.row("SELECT name, app_id FROM candidates WHERE digest = ?", digest);
    return c ? appOf(c) : "";
  },

  /// The first command after this module arrives in a store: the opens
  /// already kept become the daily counts, so nothing starts at zero.
  insightsBackfill(ctx) {
    this.sql.exec("INSERT OR IGNORE INTO people_daily (day, principal) SELECT DISTINCT day, principal FROM usage_daily");
    this.sql.exec(
      `INSERT OR REPLACE INTO app_daily (day, app_id, people)
       SELECT day, app_id, COUNT(DISTINCT principal) FROM usage_daily GROUP BY day, app_id`,
    );
    this.sql.exec(
      `INSERT OR IGNORE INTO member_seen (principal, day)
       SELECT principal, MAX(day) FROM usage_daily GROUP BY principal`,
    );
    this.insightsSetMeta("insights_since", ctx.now);
  },

  // ------------------------------------------------------------- helpers

  /// This week's refusals by reason, the weekly average of the four weeks
  /// before, and whether this week is a spike: Security's numbers, and the
  /// notification an owner or admin gets for a spike (org-notify.js).
  refusalWeek(now) {
    const today = this.orgDay(now);
    const ago = (k) => dayBefore(today, k);
    const refusalsBetween = (from, to) => {
      const out = new Map();
      for (const r of this.rows("SELECT key, SUM(n) AS n FROM insights_daily WHERE metric = 'refusal' AND day >= ? AND day <= ? GROUP BY key", from, to)) {
        const reason = r.key.split("|")[0];
        out.set(reason, (out.get(reason) || 0) + r.n);
      }
      return out;
    };
    const week = refusalsBetween(ago(6), today);
    const weekTotal = [...week.values()].reduce((a, b) => a + b, 0);
    const before = [...refusalsBetween(ago(34), ago(7)).values()].reduce((a, b) => a + b, 0);
    const average = Math.round((before / 4) * 100) / 100;
    return { week, weekTotal, average, spike: weekTotal >= SPIKE_MIN && weekTotal > 2 * average };
  },

  /// How many versions were released by their own sender because nobody
  /// else could approve (IC-958). A store from before solo release has no
  /// such column, and none.
  soloReleases() {
    try {
      const r = this.row("SELECT COUNT(*) AS n FROM candidates WHERE solo = 1");
      return r ? Number(r.n) : 0;
    } catch {
      return 0;
    }
  },

  /// Every app id the organisation has, with its current release (or null)
  /// and its name, in one pass.
  insightsApps() {
    const all = this.rows("SELECT digest, name, version, app_id, state, uploaded_at, approved_at, uploaded_by, connections, rowid FROM candidates ORDER BY uploaded_at DESC, rowid DESC");
    const byApp = new Map();
    for (const c of all) {
      const app = appOf(c);
      if (!app) continue;
      if (!byApp.has(app)) byApp.set(app, []);
      byApp.get(app).push(c);
    }
    // What each app's link opens, as org-releases.js currentOf decides it --
    // the chosen release while it is released, else the newest approved --
    // in one pass rather than one scan per app. The tests compare the two.
    const chosen = new Map(this.rows("SELECT app_id, digest FROM current_releases").map((r) => [r.app_id, r.digest]));
    const newest = (list) =>
      list
        .filter((c) => c.state === "released")
        .sort((a, b) => (b.approved_at || 0) - (a.approved_at || 0) || b.rowid - a.rowid)[0] || null;
    const out = [];
    for (const [app, list] of byApp) {
      const pick = chosen.has(app) ? list.find((c) => c.digest === chosen.get(app) && c.state === "released") : null;
      const current = pick || newest(list);
      const named = current || list.find((c) => c.state === "released") || list[0];
      out.push({ app_id: app, name: named.name || app, current, list });
    }
    out.sort((a, b) => String(a.name).localeCompare(String(b.name)) || a.app_id.localeCompare(b.app_id));
    return out;
  },

  /// The last `n` days, oldest first, as ISO days.
  insightsDays(now, n) {
    const out = [];
    const today = this.orgDay(now);
    for (let k = n - 1; k >= 0; k--) out.push(dayBefore(today, k));
    return out;
  },

  // ----------------------------------------------- the organisation's

  insightsOrg(ctx) {
    const now = ctx.now;
    const today = this.orgDay(now);
    const ago = (k) => dayBefore(today, k);
    const since = this.insightsSince();
    const members = this.rows("SELECT principal, role, added_at FROM members WHERE state = 'active' ORDER BY added_at, rowid");
    const person = (p) => ({ principal: p, display: displayOf(p) });

    // Adoption: people who opened one of the apps, of today's members.
    const activeIn = (from) =>
      this.row(
        `SELECT COUNT(DISTINCT p.principal) AS n FROM people_daily p
         JOIN members m ON m.principal = p.principal AND m.state = 'active' WHERE p.day >= ?`,
        from,
      ).n;
    const perDay = new Map(
      this.rows(
        `SELECT p.day, COUNT(*) AS n FROM people_daily p
         JOIN members m ON m.principal = p.principal AND m.state = 'active' WHERE p.day >= ? GROUP BY p.day`,
        ago(29),
      ).map((r) => [r.day, r.n]),
    );
    const weeks = new Map(
      this.rows(
        `SELECT CAST((julianday(?) - julianday(p.day)) / 7 AS INTEGER) AS w, COUNT(DISTINCT p.principal) AS n
         FROM people_daily p JOIN members m ON m.principal = p.principal AND m.state = 'active'
         WHERE p.day >= ? GROUP BY w`,
        today,
        ago(83),
      ).map((r) => [r.w, r.n]),
    );
    const trend = [];
    for (let w = 11; w >= 0; w--) trend.push({ from: ago(7 * w + 6), to: ago(7 * w), people: weeks.get(w) || 0 });

    const apps = this.insightsApps();
    const live = apps.filter((a) => a.current);
    // Adoption counts today's members; an app's own history (the
    // developer's section) counts everyone who opened it.
    const opened = new Map(
      this.rows(
        `SELECT u.app_id, COUNT(DISTINCT u.principal) AS n FROM usage_daily u
         JOIN members m ON m.principal = u.principal AND m.state = 'active' WHERE u.day >= ? GROUP BY u.app_id`,
        ago(29),
      ).map((r) => [r.app_id, r.n]),
    );
    const owner = (app) => {
      const p = this.ownerOf(app);
      const m = p ? this.member(p) : null;
      return p ? { ...person(p), active: Boolean(m && m.state === "active") } : null;
    };
    const top = live
      .filter((a) => opened.get(a.app_id))
      .map((a) => ({ app_id: a.app_id, name: a.name, people: opened.get(a.app_id) }))
      .sort((a, b) => b.people - a.people || a.name.localeCompare(b.name))
      .slice(0, 5);
    // An app counts as unused only once it has been out for the whole
    // window: one released this morning has not had its 30 days.
    const unused = live
      .filter((a) => !opened.get(a.app_id) && a.current.approved_at <= now - 30 * DAY)
      .map((a) => ({
        app_id: a.app_id,
        name: a.name,
        version: a.current.version,
        digest: a.current.digest,
        released_at: a.current.approved_at,
        owner: owner(a.app_id),
      }));

    // People.
    const seen = new Map(this.rows("SELECT principal, day FROM member_seen").map((r) => [r.principal, r.day]));
    const cutoff = ago(INACTIVE_DAYS - 1);
    const inactive = members
      .filter((m) => m.added_at <= now - INACTIVE_DAYS * DAY)
      .filter((m) => {
        const day = seen.get(m.principal);
        if (day) return day < cutoff;
        return since <= now - INACTIVE_DAYS * DAY;
      })
      .map((m) => ({ ...person(m.principal), role: m.role, last_active: seen.get(m.principal) || null, since: m.added_at }));
    let sso = null;
    try {
      sso = this.row("SELECT issuer, domain, domain_verified_at, enforce FROM sso WHERE id = 1");
    } catch (_) {}
    let saml = false;
    try {
      saml = Boolean(this.row("SELECT 1 AS y FROM saml WHERE id = 1"));
    } catch (_) {}
    const company = sso && sso.domain_verified_at && (sso.issuer || saml) ? sso.domain : null;
    const viaCompany = (p) => Boolean(company) && p.startsWith("email:") && p.endsWith(`@${company}`);
    const bySignIn = { github: 0, email: 0, company: 0 };
    const byRole = { owner: 0, admin: 0, developer: 0, reviewer: 0, recipient: 0 };
    for (const m of members) {
      byRole[m.role] = (byRole[m.role] || 0) + 1;
      const kind = viaCompany(m.principal) ? "company" : (parsePrincipal(m.principal) || {}).kind;
      if (kind) bySignIn[kind] = (bySignIn[kind] || 0) + 1;
    }
    const withoutCompany = company ? members.filter((m) => !viaCompany(m.principal)).map((m) => ({ ...person(m.principal), role: m.role })) : [];
    const invites = this.rows("SELECT principal, role, issued_by, issued_at, expires_at FROM invites WHERE state = 'open' AND expires_at > ? ORDER BY issued_at", now).map(
      (i) => ({ ...person(i.principal), role: i.role, invited_by: person(i.issued_by), issued_at: i.issued_at, expires_at: i.expires_at }),
    );

    // Security.
    const key = this.currentKey();
    const devices = this.rows(
      `SELECT d.device_id, d.principal, d.name, d.added_at, d.last_seen, e.version AS holds
       FROM key_devices d JOIN members m ON m.principal = d.principal AND m.state = 'active'
       LEFT JOIN key_envelopes e ON e.device_id = d.device_id
       WHERE d.state = 'active' ORDER BY d.added_at, d.device_id`,
    ).map((d) => {
      const last = Math.max(d.last_seen || 0, d.added_at || 0, since);
      return {
        device_id: d.device_id,
        name: d.name || null,
        ...person(d.principal),
        added_at: d.added_at,
        last_seen: d.last_seen || null,
        holds_current: Boolean(key && d.holds === key.version),
        silent: now - last > SILENT_DAYS * DAY,
      };
    });
    const lastSent = new Map(this.rows("SELECT uploaded_by, MAX(uploaded_at) AS at FROM candidates GROUP BY uploaded_by").map((r) => [r.uploaded_by, r.at]));
    const publishers = members
      .filter((m) => allows(m.role, "candidate.publish") === "yes")
      .map((m) => ({ ...person(m.principal), role: m.role, since: m.added_at, last_sent_at: lastSent.get(m.principal) || null }));
    const approvers = members.filter((m) => allows(m.role, "candidate.approve") === "yes").length;
    const { week, weekTotal, average, spike } = this.refusalWeek(now);
    const withdrawnOffline = this.rows("SELECT digest, name, version, app_id, withdrawn_at FROM candidates WHERE state = 'withdrawn' ORDER BY withdrawn_at DESC")
      .map((c) => ({ c, held: this.leasesOutstanding(c.digest, now) }))
      .filter((x) => x.held.active > 0)
      .map(({ c, held }) => ({ app_id: appOf(c), name: c.name, version: c.version, digest: c.digest, withdrawn_at: c.withdrawn_at, people: held.active, until: held.until }));
    const tokens = this.rows("SELECT last_used_at FROM scim_tokens WHERE revoked_at IS NULL");
    const scimWeek = { ok: 0, failed: 0, refused: 0 };
    for (const r of this.rows("SELECT key, SUM(n) AS n FROM insights_daily WHERE metric = 'scim' AND day >= ? GROUP BY key", ago(6))) {
      scimWeek[r.key] = r.n;
    }
    const lastUsed = tokens.reduce((a, t) => Math.max(a, t.last_used_at || 0), 0) || null;
    const exp = this.exportView();

    // Releases, in the last 30 days.
    const monthAgo = now - 30 * DAY;
    const approvedMonth = this.row("SELECT COUNT(*) AS n FROM candidates WHERE approved_at >= ?", monthAgo).n;
    const waits90 = this.rows("SELECT uploaded_at, approved_at FROM candidates WHERE approved_at >= ?", now - KEEP_DAYS * DAY).map((c) => c.approved_at - c.uploaded_at);
    const waitingNow = this.rows("SELECT uploaded_at FROM candidates WHERE state = 'waiting-for-approval' ORDER BY uploaded_at");
    const rollbacks = this.row("SELECT COALESCE(SUM(n), 0) AS n FROM insights_daily WHERE metric = 'rollback' AND day >= ?", ago(29)).n;
    const withdrawals = this.row("SELECT COUNT(*) AS n FROM candidates WHERE withdrawn_at >= ?", monthAgo).n;

    // Cost.
    const billing = billingState(this, now);

    return {
      adoption: {
        members: members.length,
        active_week: activeIn(ago(6)),
        active_month: activeIn(ago(29)),
        // Of those who opened nothing in 30 days, how many joined inside
        // that window: they have not had a month to.
        joined_not_opened: this.row(
          `SELECT COUNT(*) AS n FROM members m WHERE m.state = 'active' AND m.added_at > ?
           AND NOT EXISTS (SELECT 1 FROM people_daily p WHERE p.principal = m.principal AND p.day >= ?)`,
          now - 30 * DAY,
          ago(29),
        ).n,
        daily: this.insightsDays(now, 30).map((day) => ({ day, people: perDay.get(day) || 0 })),
        trend,
        apps: live.length,
        apps_used: live.filter((a) => opened.get(a.app_id)).length,
        top,
        unused,
      },
      people: {
        by_role: byRole,
        by_sign_in: bySignIn,
        inactive,
        company_sign_in: company ? { domain: company, enforced: Boolean(sso.enforce) } : null,
        without_company_sign_in: withoutCompany,
        invites,
      },
      security: {
        key: key ? { version: key.version, state: key.state, created_at: key.created_at } : null,
        devices,
        publishers,
        approvers,
        // Solo release (IC-958): until a second person can approve, an
        // owner releases their own apps, and this says so.
        two_person: typeof this.twoPersonInForce === "function" ? this.twoPersonInForce() : true,
        solo_releases: this.soloReleases(),
        refusals: {
          week: [...week.entries()].map(([reason, n]) => ({ reason, n })).sort((a, b) => b.n - a.n || a.reason.localeCompare(b.reason)),
          week_total: weekTotal,
          weekly_average_before: average,
          spike,
        },
        withdrawn_offline: withdrawnOffline,
        sign_in: {
          company: company ? { domain: company, kind: sso.issuer ? "oidc" : "saml", enforced: Boolean(sso.enforce) } : null,
          domain_pending: Boolean(sso && !sso.domain_verified_at),
        },
        directory: tokens.length || scimWeek.ok || scimWeek.failed || scimWeek.refused ? { tokens: tokens.length, last_used_at: lastUsed, week: scimWeek } : null,
        audit_export: exp
          ? {
              host: exp.host,
              delivered: exp.delivery.delivered,
              given_up: exp.delivery.given_up,
              pending: exp.delivery.pending,
              oldest_pending_at: exp.delivery.oldest_pending_at,
              last_delivered_at: exp.delivery.last_delivered_at,
              last_error: exp.delivery.last_error,
              last_error_at: exp.delivery.last_error_at,
            }
          : null,
      },
      releases: {
        approved_month: approvedMonth,
        median_wait_seconds: median(waits90),
        approvals_counted: waits90.length,
        waiting: waitingNow.length,
        oldest_waiting_since: waitingNow.length ? waitingNow[0].uploaded_at : null,
        withdrawn_month: withdrawals,
        rollbacks_month: rollbacks,
      },
      cost: {
        plan: billing.row.plan || "pilot",
        state: billing.state,
        seats_used: seatsUsed(this),
        seats_paid: billing.row.seats_paid ?? null,
        free_seats: freeSeats(this.env),
        active_month: activeIn(ago(29)),
      },
    };
  },

  // ------------------------------------------------------- a developer's

  insightsDeveloper(ctx, names) {
    const now = ctx.now;
    const today = this.orgDay(now);
    const ago = (k) => dayBefore(today, k);
    const mine = new Set(ctx.principals);
    const count = (n) => (names || n === 0 || n >= 3 ? n : FEWER);
    const owned = new Set(this.rows("SELECT app_id, principal FROM app_owners").filter((o) => mine.has(o.principal)).map((o) => o.app_id));
    const apps = this.insightsApps().filter((a) => owned.has(a.app_id) || a.list.some((c) => mine.has(c.uploaded_by)));
    const quota = quotaOf(this.env);
    return {
      apps: apps.map((a) => {
        const app = a.app_id;
        const daily = new Map(this.rows("SELECT day, people FROM app_daily WHERE app_id = ? AND day >= ?", app, ago(29)).map((r) => [r.day, r.people]));
        const week = this.row("SELECT COUNT(DISTINCT principal) AS n FROM usage_daily WHERE app_id = ? AND day >= ?", app, ago(6)).n;
        const versions = this.rows(
          `SELECT digest, version, COUNT(DISTINCT principal) AS n FROM usage_daily
           WHERE app_id = ? AND day >= ? GROUP BY digest, version ORDER BY n DESC, version DESC`,
          app,
          ago(6),
        ).map((r) => ({ version: r.version, digest: r.digest, current: Boolean(a.current && a.current.digest === r.digest), people: count(r.n) }));
        const weeks = new Map(
          this.rows(
            `SELECT CAST((julianday(?) - julianday(day)) / 7 AS INTEGER) AS w, COUNT(DISTINCT principal) AS n
             FROM usage_daily WHERE app_id = ? AND day >= ? GROUP BY w`,
            today,
            app,
            ago(83),
          ).map((r) => [r.w, r.n]),
        );
        const trend = [];
        for (let w = 11; w >= 0; w--) trend.push({ from: ago(7 * w + 6), to: ago(7 * w), people: count(weeks.get(w) || 0) });
        const refusals = new Map();
        for (const r of this.rows("SELECT key, SUM(n) AS n FROM insights_daily WHERE metric = 'refusal' AND day >= ? AND key LIKE ? GROUP BY key", ago(29), `%|${app}`)) {
          const bar = r.key.indexOf("|");
          if (r.key.slice(bar + 1) !== app) continue;
          const reason = r.key.slice(0, bar);
          refusals.set(reason, (refusals.get(reason) || 0) + r.n);
        }
        const store = this.row("SELECT used FROM org_data_apps WHERE app_id = ?", app);
        const past = this.row("SELECT COALESCE(SUM(writes), 0) AS n FROM org_data_days WHERE app_id = ? AND day >= ?", app, ago(6)).n;
        const todays = this.row("SELECT COALESCE(SUM(writes), 0) AS n FROM org_data_daily WHERE app_id = ? AND day >= ?", app, ago(6)).n;
        const shown = a.current || a.list[0];
        const asked = parsed(shown.connections, []);
        const connections = (Array.isArray(asked) ? asked : []).map((name) => ({
          name,
          uses_week: this.row("SELECT COALESCE(SUM(n), 0) AS n FROM connection_uses WHERE name = ? AND day >= ?", name, ago(6)).n,
        }));
        const waits = a.list
          .filter((c) => mine.has(c.uploaded_by))
          .slice(0, 10)
          .map((c) => ({
            version: c.version,
            state: c.state === "waiting-for-approval" ? "waiting" : c.state,
            sent_at: c.uploaded_at,
            approved_at: c.approved_at || null,
            waited_seconds: (c.approved_at || now) - c.uploaded_at,
          }));
        return {
          app_id: app,
          name: a.name,
          version: a.current ? a.current.version : null,
          released: Boolean(a.current),
          people_week: count(week),
          daily: this.insightsDays(now, 30).map((day) => ({ day, people: count(daily.get(day) || 0) })),
          trend,
          versions,
          refusals: [...refusals.entries()].map(([reason, n]) => ({ reason, n })).sort((x, y) => y.n - x.n),
          data: store ? { bytes: store.used, quota, writes_week: past + todays } : null,
          connections,
          waits,
        };
      }),
    };
  },

  // -------------------------------------------------------- a reviewer's

  insightsReviewer(ctx, names) {
    const now = ctx.now;
    const mine = new Set(ctx.principals);
    const queue = this.rows("SELECT * FROM candidates WHERE state = 'waiting-for-approval' ORDER BY uploaded_at, rowid").map((c) => {
      const ch = changesOf(c, this.replacedBy(c));
      const out = {
        app_id: appOf(c),
        name: c.name,
        version: c.version,
        digest: c.digest,
        sent_at: c.uploaded_at,
        waited_seconds: now - c.uploaded_at,
        yours: mine.has(c.uploaded_by),
        first: !ch.against,
        replaces: ch.against ? ch.against.version : null,
        // For a first version, everything it asks for is new.
        added: ch.added.map((x) => ({ words: x.words || x.cap, asks: x.asks !== false })),
        removed: ch.removed.map((x) => ({ words: x.words || x.cap })),
        kept: ch.kept,
      };
      if (names) out.sent_by = { principal: c.uploaded_by, display: displayOf(c.uploaded_by) };
      return out;
    });
    const approved = this.rows("SELECT approved_by, uploaded_at, approved_at FROM candidates WHERE approved_at >= ?", now - 30 * DAY);
    return {
      queue,
      approved_by_you_month: approved.filter((c) => mine.has(c.approved_by)).length,
      approved_month: approved.length,
      median_wait_seconds_month: median(approved.map((c) => c.approved_at - c.uploaded_at)),
    };
  },

  // ---------------------------------------------------------- your own

  insightsYours(ctx, me) {
    const now = ctx.now;
    const key = this.currentKey();
    const devices = this.rows(
      `SELECT d.device_id, d.name, d.added_at, d.last_seen, e.version AS holds FROM key_devices d
       LEFT JOIN key_envelopes e ON e.device_id = d.device_id
       WHERE d.principal = ? AND d.state = 'active' ORDER BY d.added_at`,
      me.principal,
    ).map((d) => ({ device_id: d.device_id, name: d.name || null, added_at: d.added_at, last_seen: d.last_seen || null, holds_current: Boolean(key && d.holds === key.version) }));
    const names = new Map(this.insightsApps().map((a) => [a.app_id, a.name]));
    const marks = ctx.principals.map(() => "?").join(", ") || "''";
    const apps = this.rows(
      `SELECT app_id, COUNT(DISTINCT day) AS days, MAX(day) AS last FROM usage_daily
       WHERE principal IN (${marks}) AND day >= ? GROUP BY app_id ORDER BY days DESC, app_id`,
      ...ctx.principals,
      dayBefore(this.orgDay(now), 29),
    ).map((r) => ({ app_id: r.app_id, name: names.get(r.app_id) || r.app_id, days: r.days, last: r.last }));
    return { devices, apps };
  },

  /// GET /orgs/<slug>/insights: the sections this person's role reads, in
  /// one answer. Counts only, never app content; never audited (a read).
  op_insights(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "insights.view", target);
    if (refusal) return refusal;
    if (allows(me.role, "insights.view") !== "yes") {
      return this.deny(ctx, "insights.view", target, "role-cannot", `a ${me.role} cannot see ${ctx.slug}'s insights`);
    }
    const names = allows(me.role, "insights.people") === "yes";
    const sections = sectionsFor(me.role);
    const out = { v: 1, now: ctx.now, today: this.orgDay(ctx.now), time_zone: this.timeZone(), time_zone_chosen: Boolean(this.timeZoneRow()), role: me.role, names, since: this.insightsSince() || null, sections };
    if (sections.includes("org")) out.org = this.insightsOrg(ctx);
    if (sections.includes("developer")) out.developer = this.insightsDeveloper(ctx, names);
    if (sections.includes("reviewer")) out.reviewer = this.insightsReviewer(ctx, names);
    if (sections.includes("yours")) out.yours = this.insightsYours(ctx, me);
    return this.ok(out);
  },
};

/// Before every command: the first time, the backfill; then the day's
/// pruning (once a day) and the actor's last day here.
function tick(ctx) {
  if (!this.insightsMeta("insights_since")) this.insightsBackfill(ctx);
  const today = this.orgDay(ctx.now);
  if (this.insightsMeta("insights_pruned") !== today) {
    const oldest = dayBefore(this.orgDay(ctx.now), KEEP_DAYS);
    for (const t of ["people_daily", "app_daily", "insights_daily"]) this.sql.exec(`DELETE FROM ${t} WHERE day < ?`, oldest);
    this.insightsSetMeta("insights_pruned", today);
  }
  if (ctx.actor) {
    const m = this.member(ctx.actor);
    if (m && m.state === "active") {
      this.sql.exec(
        `INSERT INTO member_seen (principal, day) VALUES (?, ?)
         ON CONFLICT (principal) DO UPDATE SET day = excluded.day WHERE excluded.day > member_seen.day`,
        ctx.actor,
        today,
      );
    }
  }
}

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({ schema: SCHEMA, columns: COLUMNS, tick });
  const P = OrgClass.prototype;
  for (const name of ["noteUse", "deny", "op_set_current", "op_keys_view", "op_export_begin", "leasesOutstanding", "exportView", "currentKey", "ownerOf", "replacedBy"]) {
    if (typeof P[name] !== "function") throw new Error(`org-insights: ${name} is not installed`);
  }

  // An open: the person's day, and the app's people that day.
  const noteUse = P.noteUse;
  P.noteUse = function (ctx, c) {
    if (!c || !ctx.actor) return noteUse.call(this, ctx, c);
    const day = this.orgDay(ctx.now);
    const app = appOf(c);
    const already = this.row("SELECT 1 AS y FROM usage_daily WHERE day = ? AND app_id = ? AND principal = ? LIMIT 1", day, app, ctx.actor);
    noteUse.call(this, ctx, c);
    if (!already) {
      this.sql.exec(
        `INSERT INTO app_daily (day, app_id, people) VALUES (?, ?, 1)
         ON CONFLICT (day, app_id) DO UPDATE SET people = people + 1`,
        day,
        app,
      );
    }
    this.sql.exec("INSERT OR IGNORE INTO people_daily (day, principal) VALUES (?, ?)", day, ctx.actor);
  };

  // A refusal: counted by reason, and by app when it names one -- under
  // the same rule as its audit line (an authenticated actor, an org).
  const deny = P.deny;
  P.deny = function (ctx, action, target, reason, detail) {
    const answer = deny.call(this, ctx, action, target, reason, detail);
    if (ctx.actor && this.org()) {
      // An app "met" a refusal when someone was stopped using it --
      // opening, offline access, its data, a connection -- not when, say,
      // someone without the role tried to approve it.
      const app = OPENING.test(String(action || "")) ? this.insightsAppOf(ctx, target) : "";
      this.insightsCount(this.orgDay(ctx.now), "refusal", `${reason}|${app}`);
    }
    return answer;
  };

  // A rollback: an older approved release chosen as current.
  const setCurrent = P.op_set_current;
  P.op_set_current = function (ctx) {
    const app = String(ctx.args.app_id || "").trim().toLowerCase().slice(0, 120);
    const was = app ? this.currentOf(app) : null;
    const answer = setCurrent.call(this, ctx);
    if (answer.status === 200 && was) {
      const now = this.currentOf(app);
      if (now && now !== was) {
        const a = this.row("SELECT approved_at, rowid FROM candidates WHERE digest = ?", now);
        const b = this.row("SELECT approved_at, rowid FROM candidates WHERE digest = ?", was);
        if (a && b && (a.approved_at < b.approved_at || (a.approved_at === b.approved_at && a.rowid < b.rowid))) {
          this.insightsCount(this.orgDay(ctx.now), "rollback", app);
        }
      }
    }
    return answer;
  };

  // A device's look at the keyring is its check-in: when it was last seen.
  const keysView = P.op_keys_view;
  P.op_keys_view = function (ctx) {
    const answer = keysView.call(this, ctx);
    if (answer.status === 200 && answer.body && answer.body.device) {
      this.sql.exec(
        "UPDATE key_devices SET last_seen = ? WHERE device_id = ? AND (last_seen IS NULL OR last_seen <= ?)",
        ctx.now,
        answer.body.device.device_id,
        ctx.now - 3600,
      );
    }
    return answer;
  };

  // The organisation's export carries its insights.
  const begin = P.op_export_begin;
  P.op_export_begin = function (ctx) {
    const answer = begin.call(this, ctx);
    if (answer.status === 202 && answer.body && answer.body.data) {
      answer.body.data.insights = { since: this.insightsSince() || null, at: ctx.now, ...this.insightsOrg(ctx) };
    }
    return answer;
  };

  // The directory's requests, by outcome, counted after the command --
  // including one a SCIM abort rolled back -- in a transaction of their own.
  const fetch = P.fetch;
  P.fetch = async function (request) {
    let op = null;
    try {
      op = (await request.clone().json()).op;
    } catch (_) {}
    const res = await fetch.call(this, request);
    if (op && DIRECTORY_OPS.has(op)) {
      let status = 0;
      try {
        status = Number((await res.clone().json()).status) || 0;
      } catch (_) {}
      const outcome = status === 401 ? "refused" : status >= 400 ? "failed" : "ok";
      this.ctx.storage.transactionSync(() => {
        if (this.org()) this.insightsCount(this.orgDay(this.now()), "scim", outcome);
      });
    }
    return res;
  };
}

install(Org);

// ---------------------------------------------------------------- routes

/// GET /orgs/<slug>/insights. Answers a Response, or null.
export async function insightsRoute(h) {
  const { rest, method, askRaw, orgAnswer } = h;
  if (rest[0] === "insights" && rest.length === 1 && method === "GET") {
    const res = orgAnswer(await askRaw("insights"));
    res.headers.set("cache-control", "private, no-store");
    return res;
  }
  return null;
}
