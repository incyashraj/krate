// Notifications: what is waiting on this person in the organisation, and
// what happened to what they sent -- worked out from the organisation's own
// store each time it is asked, so nothing is stored per item and nothing
// is marked read here (the page remembers when it last looked).
//
//   GET /orgs/<slug>/notifications[?since=<seconds>]
//       -> 200 {items: [{id, kind, at, text, tab, target}], counts: {unread_hint}}
//       newest first, at most 50. unread_hint: how many are newer than
//       `since` (the page's last look), or all of them without it.
//   GET /orgs/<slug>/me/notify  -> 200 {prefs}
//   PUT /orgs/<slug>/me/notify {prefs: {"approval-waiting": {email}, ...}}
//       -> 200 {prefs}, every kind filled in
//
// kind "approval-waiting"  a version waits that this person may approve
//                          and did not send                    tab "waiting"
//      "version-decided"   their own version was approved, declined or
//                          withdrawn in the last 30 days        tab "apps" / "waiting"
//      "request"           an open app request (owners, admins,
//                          developers), or the answer to one this
//                          person asked for (anyone)            tab "requests"
//      "security"          owners and admins: the team data key needs
//                          renewing, refusals spiked this week, a computer
//                          was added in the last 7 days         tab "security"
//
// Mail: when a version starts waiting, each member who may approve it (not
// its sender) whose preferences allow; when it is approved or declined, its
// sender; when someone asks for an app, the people who answer requests who
// turned that on. Defaults: on, except requests. Through the hub's Gmail
// path (org-mail.js), at most 100 a day per organisation, counted in its
// store -- never in KV -- and each event mailed once. Nothing is mailed
// when the hub has no mail set up, and a mail that fails changes nothing.
// Nothing about mail is audited.
import { Org, extendOrg, allows, displayOf } from "./org.js";
import { gmail, addressFor } from "./org-mail.js";
import "./org-insights.js";
import "./org-keys.js";

export const NOTIFY_KINDS = ["approval-waiting", "version-decided", "request", "security"];
export const NOTIFY_DEFAULTS = {
  "approval-waiting": { email: true },
  "version-decided": { email: true },
  request: { email: false },
  security: { email: true },
};
export const NOTIFY_MAX = 50;
export const MAIL_PER_ORG_PER_DAY = 100;
const DAY = 86400;

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS notify_mail (day TEXT PRIMARY KEY, n INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS notify_sent (key TEXT PRIMARY KEY, at INTEGER NOT NULL)`,
];

/// How a person is named in a sentence: @login for GitHub, the address
/// for an inbox.
export function personWords(principal) {
  const p = String(principal || "");
  return p.startsWith("github:") ? `@${displayOf(p)}` : displayOf(p) || "someone";
}

const appWords = (c) => `${c.name || "An app"}${c.version ? ` ${c.version}` : ""}`;
const clip = (t, n) => {
  const s = String(t || "").replace(/\s+/g, " ").trim();
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
};

/// The stored preferences, every kind filled in from the defaults.
export function prefsOf(raw) {
  let stored = {};
  try {
    stored = JSON.parse(raw || "{}") || {};
  } catch {
    stored = {};
  }
  const out = {};
  for (const kind of NOTIFY_KINDS) {
    const s = stored[kind];
    out[kind] = { email: s && typeof s.email === "boolean" ? s.email : NOTIFY_DEFAULTS[kind].email };
  }
  return out;
}

const ops = {
  notifications(ctx) {
    const { me, refusal } = this.standing(ctx, "org.view", `org:${ctx.slug}`);
    if (refusal) return refusal;
    const now = ctx.now;
    const mine = (p) => ctx.principals.includes(p);
    const items = [];

    if (allows(me.role, "candidate.approve") === "yes") {
      for (const c of this.rows("SELECT * FROM candidates WHERE state = 'waiting-for-approval' ORDER BY uploaded_at DESC LIMIT 100")) {
        if (mine(c.uploaded_by)) continue;
        items.push({ id: `waiting:${c.digest}`, kind: "approval-waiting", at: c.uploaded_at, text: `${appWords(c)} waits for your approval`, tab: "waiting", target: c.digest });
      }
    }

    const since = now - 30 * DAY;
    for (const c of this.rows("SELECT * FROM candidates WHERE uploaded_by = ? ORDER BY uploaded_at DESC LIMIT 200", me.principal)) {
      // Released by its sender alone (solo) is their own act: nothing to hear.
      if (c.approved_at && c.approved_at >= since && c.approved_by && !mine(c.approved_by)) {
        items.push({ id: `approved:${c.digest}`, kind: "version-decided", at: c.approved_at, text: `${personWords(c.approved_by)} approved ${appWords(c)}. It is released`, tab: "apps", target: c.digest });
      }
      if (c.state === "declined" && c.declined_at >= since) {
        items.push({ id: `declined:${c.digest}`, kind: "version-decided", at: c.declined_at, text: `${personWords(c.declined_by)} declined ${appWords(c)}: ${clip(c.decline_reason, 200)}`, tab: "waiting", target: c.digest });
      }
      if (c.state === "withdrawn" && c.withdrawn_at >= since && c.withdrawn_by && !mine(c.withdrawn_by)) {
        items.push({ id: `withdrawn:${c.digest}`, kind: "version-decided", at: c.withdrawn_at, text: `${personWords(c.withdrawn_by)} withdrew ${appWords(c)}${c.withdraw_reason ? `: ${clip(c.withdraw_reason, 200)}` : ""}`, tab: "apps", target: c.digest });
      }
    }

    if (allows(me.role, "request.manage") === "yes") {
      for (const r of this.rows("SELECT * FROM app_requests WHERE state = 'open' ORDER BY asked_at DESC LIMIT 100")) {
        if (mine(r.asked_by)) continue;
        items.push({ id: `request:${r.id}`, kind: "request", at: r.asked_at, text: `${personWords(r.asked_by)} asked for an app: "${clip(r.text, 120)}"`, tab: "requests", target: r.id });
      }
    }

    // What happened to what this person asked for: answered by someone
    // else in the last 30 days. Shown here only; nothing is mailed for it.
    const VERB = { planned: "plans to make", done: "made", declined: "does not plan to make" };
    for (const r of this.rows("SELECT * FROM app_requests WHERE asked_by = ? AND replied_at >= ? ORDER BY replied_at DESC LIMIT 50", me.principal, since)) {
      if (!r.replied_by || mine(r.replied_by) || !VERB[r.state]) continue;
      items.push({ id: `answered:${r.id}:${r.replied_at}`, kind: "request", at: r.replied_at, text: `${personWords(r.replied_by)} ${VERB[r.state]} what you asked for, "${clip(r.text, 80)}"${r.reply ? `: ${clip(r.reply, 160)}` : ""}`, tab: "requests", target: r.id });
    }

    if (me.role === "owner" || me.role === "admin") {
      const key = this.currentKey();
      if (key && key.state === "stale") {
        items.push({ id: `key:v${key.version}`, kind: "security", at: key.stale_since || key.created_at, text: "The team data key is due for renewing: someone left since it was made. A computer that holds it renews it the next time Krate runs there", tab: "security", target: `v${key.version}` });
      }
      const week = this.refusalWeek(now);
      if (week.spike) {
        const last = this.row("SELECT MAX(at) AS at FROM audit WHERE result = 'denied'");
        items.push({ id: `refusals:${this.orgDay(now)}`, kind: "security", at: (last && last.at) || now, text: `${week.weekTotal} refused attempts this week, more than twice the usual`, tab: "security", target: null });
      }
      for (const d of this.rows("SELECT device_id, principal, name, added_at FROM key_devices WHERE state = 'active' AND added_at >= ? ORDER BY added_at DESC LIMIT 20", now - 7 * DAY)) {
        if (mine(d.principal)) continue;
        items.push({ id: `device:${d.device_id}`, kind: "security", at: d.added_at, text: `${personWords(d.principal)} added a computer${d.name ? `, ${clip(d.name, 60)}` : ""}`, tab: "security", target: d.device_id });
      }
    }

    items.sort((a, b) => (b.at || 0) - (a.at || 0) || a.id.localeCompare(b.id));
    const shown = items.slice(0, NOTIFY_MAX);
    const seen = Number(ctx.args.since);
    const unread = Number.isFinite(seen) && seen > 0 ? shown.filter((i) => (i.at || 0) > seen).length : shown.length;
    return this.ok({ items: shown, counts: { unread_hint: unread } });
  },

  "notify-prefs-show"(ctx) {
    const { me, refusal } = this.standing(ctx, "org.view", `org:${ctx.slug}`);
    if (refusal) return refusal;
    return this.ok({ prefs: prefsOf(me.notify) });
  },

  "notify-prefs-set"(ctx) {
    const { me, refusal } = this.standing(ctx, "org.view", `org:${ctx.slug}`);
    if (refusal) return refusal;
    const given = ctx.args.prefs;
    const bad = { status: 400, body: { error: "bad-request", message: `prefs names any of ${NOTIFY_KINDS.join(", ")}, each as {email: true or false}` } };
    if (!given || typeof given !== "object" || Array.isArray(given)) return bad;
    const next = prefsOf(me.notify);
    for (const [kind, v] of Object.entries(given)) {
      if (!NOTIFY_KINDS.includes(kind) || !v || typeof v !== "object" || typeof v.email !== "boolean") return bad;
      next[kind] = { email: v.email };
    }
    this.sql.exec("UPDATE members SET notify = ? WHERE principal = ?", JSON.stringify(next), me.principal);
    return this.ok({ prefs: next });
  },

  /// Who to mail about one event that just committed, and what: called by
  /// the door after the change, never by a person. Each event is mailed
  /// once, and the organisation's daily allowance is taken here.
  "notify-plan"(ctx) {
    if (!this.org()) return this.ok({ mails: [] });
    const { event } = ctx.args;
    const site = String(ctx.args.site || "https://krate.tech").replace(/\/$/, "");
    const orgName = this.org().name;
    const link = (tab, digest) => `${site}/org/?o=${encodeURIComponent(ctx.slug)}&tab=${tab}${digest ? `&c=${digest}` : ""}`;
    const footer = (why) => `\n\nYou get this because ${why} in ${orgName} on Krate. Turn it off under Notifications on that page.`;
    const wants = (principal, kind) => {
      const m = this.member(principal);
      return Boolean(m && m.state === "active" && prefsOf(m.notify)[kind].email);
    };
    let key = null;
    let mails = [];
    if (event === "waiting" || event === "approved" || event === "declined") {
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", String(ctx.args.digest || ""));
      if (!c) return this.ok({ mails: [] });
      key = `${event}:${c.digest}`;
      if (event === "waiting" && c.state === "waiting-for-approval") {
        mails = this.rows("SELECT principal, role FROM members WHERE state = 'active' ORDER BY added_at, rowid")
          .filter((m) => allows(m.role, "candidate.approve") === "yes" && m.principal !== c.uploaded_by && wants(m.principal, "approval-waiting"))
          // Under approval rules, only those whose approval would count (org-approvals.js).
          .filter((m) => typeof this.approvalView !== "function" || this.policyIsDefault() || (this.approvalView(c, [m.principal]).you || { can: true }).can)
          .map((m) => ({
            principal: m.principal,
            subject: `${appWords(c)} waits for your approval`,
            text: `${personWords(c.uploaded_by)} sent ${appWords(c)} to ${orgName}. It waits for someone who approves to look at it.${c.note ? `\n\nTheir note: ${clip(c.note, 1000)}` : ""}\n\nSee its code and what it can do, then approve or decline it: ${link("waiting", c.digest)}${footer("you approve apps")}`,
          }));
      } else if (event === "approved" && c.state === "released" && c.approved_by && c.approved_by !== c.uploaded_by && wants(c.uploaded_by, "version-decided")) {
        mails = [{
          principal: c.uploaded_by,
          subject: `${appWords(c)} is approved`,
          text: `${personWords(c.approved_by)} approved ${appWords(c)}. It is released: everyone it is for in ${orgName} can open it now.\n\nSee it: ${link("apps")}${footer("you sent it")}`,
        }];
      } else if (event === "declined" && c.state === "declined" && wants(c.uploaded_by, "version-decided")) {
        mails = [{
          principal: c.uploaded_by,
          subject: `${appWords(c)} was declined`,
          text: `${personWords(c.declined_by)} declined ${appWords(c)}: ${clip(c.decline_reason, 500)}\n\nSend a new version once it is fixed. See it: ${link("waiting")}${footer("you sent it")}`,
        }];
      }
    } else if (event === "request") {
      const r = this.row("SELECT * FROM app_requests WHERE id = ?", String(ctx.args.id || ""));
      if (!r) return this.ok({ mails: [] });
      key = `request:${r.id}`;
      mails = this.rows("SELECT principal, role FROM members WHERE state = 'active' ORDER BY added_at, rowid")
        .filter((m) => allows(m.role, "request.manage") === "yes" && m.principal !== r.asked_by && wants(m.principal, "request"))
        .map((m) => ({
          principal: m.principal,
          subject: `${personWords(r.asked_by)} asked ${orgName} for an app`,
          text: `${personWords(r.asked_by)} asked for an app: "${clip(r.text, 1000)}"\n\nAnswer it: ${link("requests")}${footer("you turned on mail for app requests")}`,
        }));
    }
    if (!key || !mails.length) return this.ok({ mails: [] });
    if (this.row("SELECT key FROM notify_sent WHERE key = ?", key)) return this.ok({ mails: [] });
    const day = this.orgDay(ctx.now);
    const used = (this.row("SELECT n FROM notify_mail WHERE day = ?", day) || { n: 0 }).n;
    const room = Math.max(0, MAIL_PER_ORG_PER_DAY - used);
    mails = mails.slice(0, room);
    this.sql.exec("INSERT INTO notify_sent (key, at) VALUES (?, ?)", key, ctx.now);
    if (mails.length) {
      this.sql.exec("INSERT INTO notify_mail (day, n) VALUES (?, ?) ON CONFLICT(day) DO UPDATE SET n = n + excluded.n", day, mails.length);
    }
    this.sql.exec("DELETE FROM notify_sent WHERE at < ?", ctx.now - 90 * DAY);
    return this.ok({ mails });
  },
};

extendOrg({ schema: SCHEMA, columns: [["members", "notify", "TEXT"]], ops, mutations: ["notify-prefs-set"] });
void Org;

/// Mail the people one committed event concerns. Answers how many mails
/// went. Never throws: a mail is not worth failing anything for.
/// `n`: {env, smtpSend, askRaw, site}; `event`: {event, digest?, id?}.
export async function sendNotices(n, event) {
  try {
    if (!n || !n.env || !n.env.GMAIL_APP_PASSWORD) return 0;
    const plan = await n.askRaw("notify-plan", { ...event, site: n.site });
    let sent = 0;
    for (const m of (plan && plan.body && plan.body.mails) || []) {
      const to = await addressFor(n.env, m.principal);
      if (!to) continue;
      const problem = await gmail(n.env, n.smtpSend, { to, subject: m.subject, text: m.text, cc: null });
      if (!problem) sent += 1;
    }
    return sent;
  } catch (e) {
    console.error("a notification mail did not go:", e && e.message ? e.message : String(e));
    return 0;
  }
}

/// GET /orgs/<slug>/notifications, GET and PUT /orgs/<slug>/me/notify.
export async function orgNotifyRoute(h) {
  const { url, rest, method, askRaw, mutateRaw, orgAnswer } = h;
  if (rest[0] === "notifications" && rest.length === 1 && method === "GET") {
    return orgAnswer(await askRaw("notifications", { since: url.searchParams.get("since") || "" }));
  }
  if (rest[0] === "me" && rest[1] === "notify" && rest.length === 2) {
    if (method === "GET") return orgAnswer(await askRaw("notify-prefs-show"));
    if (method === "PUT") return orgAnswer(await mutateRaw("notify-prefs-set", (b) => ({ prefs: b.prefs })));
  }
  return null;
}
