// Notifications in Slack and Microsoft Teams (G12).
//
// An owner or admin gives the organisation one or more channel webhooks in
// Settings, each with the kinds it posts:
//
//   "approval-waiting"  a version waits for approval
//   "version-decided"   a version was approved, declined or withdrawn
//   "request"           someone asked for an app
//   "security"          the team data key needs renewing, refusals spiked
//                       this week, a computer was added
//
// Each post is one short plain sentence and a link to the console page it
// is about (the same /org/?o=<slug>&tab=... links the mail uses).
//
// Only two services, and only their own webhook hosts: Slack's incoming
// webhooks and workflow triggers on hooks.slack.com, and Microsoft Teams'
// incoming webhooks (*.webhook.office.com) and Workflows (Power Automate:
// *.logic.azure.com, *.environment.api.powerplatform.com). Nothing else is
// accepted, so the hub can never be pointed at an address of someone's
// choosing; the URL is checked when it is given and again before every post.
//
// A webhook URL is its own credential (anyone holding it can post to the
// channel), so it is sealed with ORG_SECRET_KEY (AES-GCM, the slug bound in,
// as the audit export's secret is) and never shown back: the page reads its
// host and last four characters. Adding, changing, testing and removing one
// is audited by host; a post is not.
//
// Posts go from the same moments org-notify.js mails from, and from the
// moments a security item appears (a computer added, a member removed) and
// the nightly run. Each item is posted once, at most CHAT_PER_ORG_PER_DAY
// posts a day per organisation, counted in its own store. A post that fails
// changes nothing and fails nothing: the hook records its last problem.

import { Org, MATRIX, extendOrg, addReasons, allows, displayOf } from "./org.js";
import { sealSecret, openSecret } from "./org-sso.js";
import { appOf } from "./org-releases.js";
import "./org-insights.js";
import "./org-keys.js";

MATRIX["chat.manage"] = { owner: "yes", admin: "yes" };

export const CHAT_KINDS = ["approval-waiting", "version-decided", "request", "security"];
export const CHAT_PER_ORG_PER_DAY = 50;
export const CHAT_HOOKS_MAX = 5;
const URL_MAX = 2000;
const LABEL_MAX = 60;
const POST_TIMEOUT_MS = 10_000;
const DAY = 86400;

addReasons({
  "chat-off": { status: 503, message: () => "Slack and Teams posts are not switched on on this hub (it has no ORG_SECRET_KEY)" },
  "no-such-channel": { status: 404, message: (o, d) => d || `${o} has no channel by that id -- it may have been removed` },
  "too-many-channels": { status: 409, message: (o, d) => d || `${o} already posts to ${CHAT_HOOKS_MAX} channels -- remove one first` },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS chat_hooks (
     id TEXT PRIMARY KEY, service TEXT NOT NULL, host TEXT NOT NULL, last4 TEXT NOT NULL,
     url TEXT NOT NULL, kinds TEXT NOT NULL, label TEXT,
     added_at INTEGER NOT NULL, added_by TEXT NOT NULL,
     last_ok_at INTEGER, last_error TEXT, last_error_at INTEGER)`,
  `CREATE TABLE IF NOT EXISTS chat_posts (day TEXT PRIMARY KEY, n INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS chat_sent (key TEXT PRIMARY KEY, at INTEGER NOT NULL)`,
];

// ------------------------------------------------------------ the URL check

const SLACK_PATHS = /^\/(services|triggers|workflows)\//;
const TEAMS_HOSTS = [
  // Incoming webhooks (Office 365 connectors): <tenant>.webhook.office.com/webhookb2/...
  /^[a-z0-9-]{1,63}\.webhook\.office\.com$/,
  // Workflows (Power Automate), older: prod-NN.<region>.logic.azure.com/workflows/...
  /^[a-z0-9-]{1,63}\.[a-z0-9-]{1,63}\.logic\.azure\.com$/,
  // Workflows, newer: <env>.environment.api.powerplatform.com/powerautomate/...
  /^[a-z0-9-]{1,63}(\.[a-z0-9-]{1,63}){0,3}\.environment\.api\.powerplatform\.com$/,
];

/// Which service a webhook URL belongs to, checked strictly: https on port
/// 443, no user name or password, and a Slack or Teams webhook host and
/// path. Answers `{service, url, host, last4}` or `{problem}`.
export function chatHookUrl(raw) {
  const s = String(raw || "").trim();
  const nope = {
    problem: "paste the channel's webhook URL from Slack (https://hooks.slack.com/...) or Microsoft Teams (a Workflows or incoming webhook URL)",
  };
  if (!s || s.length > URL_MAX) return nope;
  let u;
  try {
    u = new URL(s);
  } catch (_) {
    return nope;
  }
  if (u.protocol !== "https:" || u.username || u.password || u.hash || (u.port && u.port !== "443")) return nope;
  const host = u.hostname.toLowerCase();
  let service = null;
  if (host === "hooks.slack.com" && SLACK_PATHS.test(u.pathname)) service = "slack";
  else if (TEAMS_HOSTS.some((re) => re.test(host)) && u.pathname.length > 1) service = "teams";
  if (!service) return nope;
  const url = u.toString();
  return { service, url, host, last4: url.replace(/[^A-Za-z0-9]/g, "").slice(-4) };
}

/// The body one post sends: Slack's `text`, or a Teams Adaptive Card that
/// both incoming webhooks and Workflows show.
export function chatBody(service, text, link) {
  if (service === "slack") {
    const esc = (t) => String(t).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    return { text: `${esc(text)}${link ? ` <${link}|Open in Krate>` : ""}` };
  }
  return {
    type: "message",
    attachments: [
      {
        contentType: "application/vnd.microsoft.card.adaptive",
        contentUrl: null,
        content: {
          $schema: "http://adaptivecards.io/schemas/adaptive-card.json",
          type: "AdaptiveCard",
          version: "1.4",
          body: [{ type: "TextBlock", text: String(text), wrap: true }],
          actions: link ? [{ type: "Action.OpenUrl", title: "Open in Krate", url: link }] : [],
        },
      },
    ],
  };
}

// ----------------------------------------------------------------- words

const person = (p) => {
  const s = String(p || "");
  return s.startsWith("github:") ? `@${displayOf(s)}` : displayOf(s) || "Someone";
};
const appWords = (c) => `${c.name || "An app"}${c.version ? ` ${c.version}` : ""}`;
const clip = (t, n) => {
  const s = String(t || "").replace(/\s+/g, " ").trim();
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
};

function kindsArg(raw) {
  if (!Array.isArray(raw) || !raw.length) return null;
  const set = [...new Set(raw.map(String))];
  return set.every((k) => CHAT_KINDS.includes(k)) ? CHAT_KINDS.filter((k) => set.includes(k)) : null;
}

const ops = {
  chatHookView(h) {
    let kinds = [];
    try {
      kinds = JSON.parse(h.kinds);
    } catch (_) {}
    return {
      id: h.id,
      service: h.service,
      host: h.host,
      last4: h.last4,
      label: h.label || null,
      kinds,
      added_at: h.added_at,
      added_by: h.added_by,
      last_ok_at: h.last_ok_at || null,
      last_error: h.last_error || null,
      last_error_at: h.last_error_at || null,
    };
  },

  chatMay(ctx, action, target) {
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return { refusal };
    if (allows(me.role, "chat.manage") !== "yes") {
      return { refusal: this.deny(ctx, action, target, "role-cannot", `only an owner or admin of ${ctx.slug} chooses where its notices are posted`) };
    }
    return { me };
  },

  chatUsedToday(now) {
    return (this.row("SELECT n FROM chat_posts WHERE day = ?", this.orgDay(now)) || { n: 0 }).n;
  },

  chatTake(now, n) {
    if (n > 0) this.sql.exec("INSERT INTO chat_posts (day, n) VALUES (?, ?) ON CONFLICT(day) DO UPDATE SET n = n + excluded.n", this.orgDay(now), n);
  },

  /// The security items an owner or admin hears about, as org-notify.js
  /// lists them, each with the key it is posted once under.
  chatSecurityItems(now, link) {
    const items = [];
    const key = typeof this.currentKey === "function" ? this.currentKey() : null;
    if (key && key.state === "stale") {
      items.push({ key: `key:v${key.version}`, kind: "security", text: `The team data key of ${this.org().name} is due for renewing: someone left since it was made. A computer that holds it renews it the next time Krate runs there.`, link: link("security") });
    }
    if (typeof this.refusalWeek === "function") {
      const week = this.refusalWeek(now);
      const recent = this.row("SELECT 1 AS y FROM chat_sent WHERE key LIKE 'refusals:%' AND at > ?", now - 7 * DAY);
      if (week.spike && !recent) {
        items.push({ key: `refusals:${this.orgDay(now)}`, kind: "security", text: `${week.weekTotal} refused attempts in ${this.org().name} this week, more than twice the usual.`, link: link("security") });
      }
    }
    for (const d of this.rows("SELECT device_id, principal, name FROM key_devices WHERE state = 'active' AND added_at >= ? ORDER BY added_at LIMIT 20", now - 7 * DAY)) {
      items.push({ key: `device:${d.device_id}`, kind: "security", text: `${person(d.principal)} added a computer to ${this.org().name}${d.name ? `: ${clip(d.name, 60)}` : ""}.`, link: link("computers") });
    }
    return items;
  },

  // -------------------------------------------------------------- commands

  /// GET /orgs/<slug>/chat (owner, admin).
  "chat-list"(ctx) {
    const { refusal } = this.chatMay(ctx, "chat.view", "chat");
    if (refusal) return refusal;
    return this.ok({
      hooks: this.rows("SELECT * FROM chat_hooks ORDER BY added_at, id").map((h) => this.chatHookView(h)),
      kinds: CHAT_KINDS,
      per_day: CHAT_PER_ORG_PER_DAY,
      used_today: this.chatUsedToday(ctx.now),
    });
  },

  /// POST /orgs/<slug>/chat: the Worker checked the URL and sealed it.
  "chat-add"(ctx) {
    const { me, refusal } = this.chatMay(ctx, "chat.add", "chat");
    if (refusal) return refusal;
    const a = ctx.args;
    // Checked again here, from what the Worker says it checked.
    const service = a.service === "slack" || a.service === "teams" ? a.service : null;
    const hostOk = service && chatHookUrl(`https://${a.host}/${service === "slack" ? "services/" : ""}x`).service === service;
    const kinds = kindsArg(a.kinds);
    if (!hostOk || !a.sealed || !/^[A-Za-z0-9]{1,4}$/.test(String(a.last4 || ""))) {
      return { status: 400, body: { error: "bad-request", message: chatHookUrl("").problem } };
    }
    if (!kinds) return { status: 400, body: { error: "bad-request", message: `kinds names one or more of ${CHAT_KINDS.join(", ")}` } };
    if (this.row("SELECT COUNT(*) AS n FROM chat_hooks").n >= CHAT_HOOKS_MAX) return this.deny(ctx, "chat.add", "chat", "too-many-channels");
    const id = [...crypto.getRandomValues(new Uint8Array(6))].map((b) => b.toString(16).padStart(2, "0")).join("");
    const label = a.label ? clip(a.label, LABEL_MAX) : null;
    this.sql.exec(
      "INSERT INTO chat_hooks (id, service, host, last4, url, kinds, label, added_at, added_by) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
      id,
      service,
      a.host,
      a.last4,
      JSON.stringify(a.sealed),
      JSON.stringify(kinds),
      label,
      ctx.now,
      me.principal,
    );
    this.audit(me.principal, "chat.add", `chat:${a.host}`, "allowed", kinds.join(","));
    // A new channel starts from now: the security items already open are
    // on the Security page, not posted as a backlog.
    for (const item of this.chatSecurityItems(ctx.now, () => "")) this.sql.exec("INSERT OR IGNORE INTO chat_sent (key, at) VALUES (?, ?)", item.key, ctx.now);
    return this.ok({ hook: this.chatHookView(this.row("SELECT * FROM chat_hooks WHERE id = ?", id)), first: this.row("SELECT COUNT(*) AS n FROM chat_hooks").n === 1 }, 201);
  },

  /// PUT /orgs/<slug>/chat/<id> {kinds?, label?}.
  "chat-change"(ctx) {
    const { me, refusal } = this.chatMay(ctx, "chat.change", "chat");
    if (refusal) return refusal;
    const h = this.row("SELECT * FROM chat_hooks WHERE id = ?", String(ctx.args.id || ""));
    if (!h) return this.deny(ctx, "chat.change", "chat", "no-such-channel");
    const kinds = ctx.args.kinds === undefined ? JSON.parse(h.kinds) : kindsArg(ctx.args.kinds);
    if (!kinds) return { status: 400, body: { error: "bad-request", message: `kinds names one or more of ${CHAT_KINDS.join(", ")}` } };
    const label = ctx.args.label === undefined ? h.label : ctx.args.label ? clip(ctx.args.label, LABEL_MAX) : null;
    this.sql.exec("UPDATE chat_hooks SET kinds = ?, label = ? WHERE id = ?", JSON.stringify(kinds), label, h.id);
    this.audit(me.principal, "chat.change", `chat:${h.host}`, "allowed", kinds.join(","));
    return this.ok({ hook: this.chatHookView(this.row("SELECT * FROM chat_hooks WHERE id = ?", h.id)) });
  },

  /// DELETE /orgs/<slug>/chat/<id>.
  "chat-remove"(ctx) {
    const { me, refusal } = this.chatMay(ctx, "chat.remove", "chat");
    if (refusal) return refusal;
    const h = this.row("SELECT * FROM chat_hooks WHERE id = ?", String(ctx.args.id || ""));
    if (h) {
      this.sql.exec("DELETE FROM chat_hooks WHERE id = ?", h.id);
      this.audit(me.principal, "chat.remove", `chat:${h.host}`, "allowed");
    }
    return this.ok({ left: this.row("SELECT COUNT(*) AS n FROM chat_hooks").n });
  },

  /// POST /orgs/<slug>/chat/<id>/test: the hook to post a test to, counted
  /// in the day's posts. The Worker posts and says what happened.
  "chat-test"(ctx) {
    const { me, refusal } = this.chatMay(ctx, "chat.test", "chat");
    if (refusal) return refusal;
    const h = this.row("SELECT * FROM chat_hooks WHERE id = ?", String(ctx.args.id || ""));
    if (!h) return this.deny(ctx, "chat.test", "chat", "no-such-channel");
    if (this.chatUsedToday(ctx.now) >= CHAT_PER_ORG_PER_DAY) {
      return { status: 429, body: { error: "chat-full", message: `${ctx.slug} has posted ${CHAT_PER_ORG_PER_DAY} times today, the most in one day -- try the test again tomorrow` } };
    }
    this.chatTake(ctx.now, 1);
    this.audit(me.principal, "chat.test", `chat:${h.host}`, "allowed");
    const site = String(ctx.args.site || "https://krate.tech").replace(/\/$/, "");
    return this.ok({
      slug: this.org().slug,
      post: {
        hook: h.id,
        service: h.service,
        sealed: JSON.parse(h.url),
        text: `Krate is connected: ${this.org().name} posts here when ${JSON.parse(h.kinds).map((k) => KIND_WORDS[k]).join(", ")}.`,
        link: `${site}/org/?o=${encodeURIComponent(this.org().slug)}&tab=settings`,
      },
    });
  },

  /// After posting: what each hook answered. Called by the Worker only.
  "chat-noted"(ctx) {
    for (const r of Array.isArray(ctx.args.results) ? ctx.args.results.slice(0, 100) : []) {
      if (r.ok) this.sql.exec("UPDATE chat_hooks SET last_ok_at = ?, last_error = NULL, last_error_at = NULL WHERE id = ?", ctx.now, String(r.hook));
      else this.sql.exec("UPDATE chat_hooks SET last_error = ?, last_error_at = ? WHERE id = ?", clip(r.error, 200), ctx.now, String(r.hook));
    }
    return this.ok(null, 204);
  },

  /// What to post about one event that just committed, and where: called by
  /// the Worker after the change, never by a person. Every pending security
  /// item rides along. Each item is posted once; the day's allowance is
  /// taken here.
  "chat-plan"(ctx) {
    if (!this.org()) return this.ok({ posts: [] });
    const hooks = this.rows("SELECT * FROM chat_hooks ORDER BY added_at, id");
    if (!hooks.length) return this.ok({ posts: [] });
    const site = String(ctx.args.site || "https://krate.tech").replace(/\/$/, "");
    const o = this.org();
    const link = (tab) => `${site}/org/?o=${encodeURIComponent(o.slug)}&tab=${tab}`;
    const items = [];
    const { event } = ctx.args;
    if (["waiting", "approved", "declined", "withdrawn"].includes(event)) {
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", String(ctx.args.digest || ""));
      if (c && event === "waiting" && c.state === "waiting-for-approval") {
        items.push({ key: `waiting:${c.digest}`, kind: "approval-waiting", text: `${appWords(c)} waits for approval in ${o.name}. ${person(c.uploaded_by)} sent it.`, link: link("waiting") });
      } else if (c && event === "approved" && c.state === "released") {
        const pilot = typeof this.pilotDigestOf === "function" && this.pilotDigestOf(appOf(c)) === c.digest ? this.rolloutRow(appOf(c)).grp : null;
        const how = pilot ? `It goes to the ${pilot} group first.` : "Everyone it is for can open it now.";
        items.push({ key: `approved:${c.digest}`, kind: "version-decided", text: `${person(c.approved_by)} approved ${appWords(c)} in ${o.name}. ${how}`, link: link("releases") });
      } else if (c && event === "declined" && c.state === "declined") {
        items.push({ key: `declined:${c.digest}`, kind: "version-decided", text: `${person(c.declined_by)} declined ${appWords(c)} in ${o.name}: ${clip(c.decline_reason, 300)}`, link: link("waiting") });
      } else if (c && event === "withdrawn" && c.state === "withdrawn") {
        items.push({ key: `withdrawn:${c.digest}`, kind: "version-decided", text: `${person(c.withdrawn_by)} withdrew ${appWords(c)} in ${o.name}: ${clip(c.withdraw_reason, 300)}`, link: link("releases") });
      }
    } else if (event === "request") {
      const r = this.row("SELECT * FROM app_requests WHERE id = ?", String(ctx.args.id || ""));
      if (r) items.push({ key: `request:${r.id}`, kind: "request", text: `${person(r.asked_by)} asked ${o.name} for an app: "${clip(r.text, 300)}"`, link: link("requests") });
    }
    items.push(...this.chatSecurityItems(ctx.now, link));

    let room = Math.max(0, CHAT_PER_ORG_PER_DAY - this.chatUsedToday(ctx.now));
    const posts = [];
    for (const item of items) {
      if (this.row("SELECT 1 AS y FROM chat_sent WHERE key = ?", item.key)) continue;
      const to = hooks.filter((h) => JSON.parse(h.kinds).includes(item.kind));
      if (!to.length) continue;
      // Over the day's allowance: left for later, not marked sent, so a
      // security item still goes tomorrow; an event's own item is dropped.
      if (to.length > room) {
        if (item.kind === "security") continue;
        this.sql.exec("INSERT INTO chat_sent (key, at) VALUES (?, ?)", item.key, ctx.now);
        continue;
      }
      this.sql.exec("INSERT INTO chat_sent (key, at) VALUES (?, ?)", item.key, ctx.now);
      room -= to.length;
      for (const h of to) posts.push({ hook: h.id, service: h.service, sealed: JSON.parse(h.url), text: item.text, link: item.link });
    }
    this.chatTake(ctx.now, posts.length);
    this.sql.exec("DELETE FROM chat_sent WHERE at < ?", ctx.now - 90 * DAY);
    return this.ok({ slug: o.slug, posts });
  },
};

const KIND_WORDS = {
  "approval-waiting": "a version waits for approval",
  "version-decided": "a version is approved, declined or withdrawn",
  request: "someone asks for an app",
  security: "something about security needs a look",
};

function install(OrgClass) {
  const helpers = {};
  const commands = {};
  for (const [k, fn] of Object.entries(ops)) (k.startsWith("chat-") ? commands : helpers)[k] = fn;
  Object.assign(OrgClass.prototype, helpers);
  extendOrg({ schema: SCHEMA, ops: commands, mutations: ["chat-change", "chat-remove"] });
}
install(Org);

// ------------------------------------------------------------- posting

/// Post one message. Answers null, or the problem in words. Never throws.
export async function chatPost(env, slug, post) {
  try {
    const url = await openSecret(env, slug, post.sealed, `krate-org-chat:${slug}`);
    if (!url) return "the webhook cannot be opened on this hub -- remove the channel and add it again";
    // Checked again before every post: only a Slack or Teams webhook.
    const checked = chatHookUrl(url);
    if (!checked.service || checked.service !== post.service) return "that address is not a Slack or Teams webhook";
    const res = await fetch(checked.url, {
      method: "POST",
      headers: { "content-type": "application/json", "user-agent": "Krate-Notify/1" },
      body: JSON.stringify(chatBody(checked.service, post.text, post.link)),
      redirect: "manual",
      signal: AbortSignal.timeout(POST_TIMEOUT_MS),
    });
    await res.arrayBuffer().catch(() => {});
    if (res.status >= 200 && res.status < 300) return null;
    if (res.status === 404 || res.status === 410 || res.status === 403) return `the channel's webhook answered ${res.status}: it may have been removed in ${checked.service === "slack" ? "Slack" : "Teams"} -- add it again`;
    return `the channel's webhook answered ${res.status}`;
  } catch (e) {
    return e && e.name === "TimeoutError" ? "the webhook did not answer within 10 seconds" : `the webhook could not be reached (${String((e && e.message) || e).slice(0, 120)})`;
  }
}

/// Post what one committed event concerns to the organisation's channels.
/// `n`: {env, askRaw, site}. Answers how many posts went. Never throws.
export async function postChat(n, event) {
  try {
    if (!n || !n.env || !n.env.ORG_SECRET_KEY) return 0;
    const plan = await n.askRaw("chat-plan", { ...event, site: n.site });
    const posts = (plan && plan.body && plan.body.posts) || [];
    if (!posts.length) return 0;
    const results = [];
    for (const p of posts) {
      const problem = await chatPost(n.env, plan.body.slug, p);
      results.push({ hook: p.hook, ok: !problem, error: problem });
    }
    await n.askRaw("chat-noted", { results });
    return results.filter((r) => r.ok).length;
  } catch (e) {
    console.error("a channel post did not go:", e && e.message ? e.message : String(e));
    return 0;
  }
}

/// The nightly run: the security items of every organisation with a
/// channel (marked `org-chat:<slug>` when its first channel is added).
export async function chatSweep(env, orgAsk, site) {
  if (!env.ORGS || !env.ORG_SECRET_KEY) return 0;
  let listed;
  try {
    listed = await env.APPS.list({ prefix: "org-chat:" });
  } catch (_) {
    return 0;
  }
  let sent = 0;
  for (const { name } of listed.keys || []) {
    const slug = name.slice("org-chat:".length);
    const askRaw = (op, args = {}) => orgAsk(env, slug, { op, principals: [], args });
    sent += await postChat({ env, askRaw, site }, { event: "security" });
  }
  return sent;
}

// ---------------------------------------------------------------- routes

/// /orgs/<slug>/chat[/<id>[/test]]. Answers a Response, or null.
export async function orgChatRoute(h) {
  const { rest, method, env, slug, askRaw, mutateRaw, orgAnswer, json, readBody, site } = h;
  if (rest[0] !== "chat") return null;
  const off = () => json({ error: "chat-off", message: "Slack and Teams posts are not switched on on this hub (it has no ORG_SECRET_KEY)" }, 503);
  if (rest.length === 1 && method === "GET") return orgAnswer(await askRaw("chat-list"));
  if (rest.length === 1 && method === "POST") {
    const body = await readBody();
    if (body.bad || body.tooLarge) return json({ error: "bad-request", message: "the body is JSON, under 64 KiB" }, 400);
    const checked = chatHookUrl(body.value.url);
    if (checked.problem) return json({ error: "bad-request", message: checked.problem }, 400);
    // Who may, before anything is sealed: a stranger costs no crypto.
    const may = await askRaw("chat-list");
    if (may.status !== 200) return orgAnswer(may);
    const sealed = await sealSecret(env, slug, checked.url, `krate-org-chat:${slug}`);
    if (!sealed) return off();
    const answer = await askRaw("chat-add", { service: checked.service, host: checked.host, last4: checked.last4, sealed, kinds: body.value.kinds, label: body.value.label });
    if (answer.status === 201) await env.APPS.put(`org-chat:${slug}`, "1");
    return orgAnswer(answer);
  }
  if (rest.length === 2 && method === "PUT") return orgAnswer(await mutateRaw("chat-change", (b) => ({ id: rest[1], kinds: b.kinds, label: b.label })));
  if (rest.length === 2 && method === "DELETE") {
    const answer = await mutateRaw("chat-remove", () => ({ id: rest[1] }));
    if (answer.status === 200 && answer.body && answer.body.left === 0) await env.APPS.delete(`org-chat:${slug}`);
    return orgAnswer(answer.status === 200 ? { status: 204, body: null } : answer);
  }
  if (rest.length === 3 && rest[2] === "test" && method === "POST") {
    if (!env.ORG_SECRET_KEY) return off();
    const answer = await askRaw("chat-test", { id: rest[1], site });
    if (answer.status !== 200) return orgAnswer(answer);
    const p = answer.body.post;
    const problem = await chatPost(env, answer.body.slug, p);
    await askRaw("chat-noted", { results: [{ hook: p.hook, ok: !problem, error: problem }] });
    if (problem) return json({ error: "chat-failed", message: `the test did not arrive: ${problem}` }, 502);
    return json({ ok: true, posted: true });
  }
  return null;
}
