// Deleting a Krate account, and taking a copy of it first.
//
//   GET  /me/delete    what deleting would remove and what stays: the
//                      confirm step the account page shows
//   POST /me/delete    {confirm: "delete my account", apps: "remove"|"keep"}
//                      or {resume: "<account id>.<key>"} to carry on
//   GET  /me/export    one JSON file: everything the hub keeps about you
//   desk POST /admin/api/user/delete-preview {id}
//        POST /admin/api/user/delete         {id, confirm: <email or id>, apps}
//
// The Terms promise deletion "from your account page or through support";
// until this there was no deletion anywhere in the code (K-1245).
//
// One job, in one place, for the person and for support. It is written
// first (`acctdel:<account id>`, holding what it needs once the account
// record is gone), then runs a fixed list of steps. Every step is
// idempotent -- a key already gone is simply gone again -- so a run that
// dies anywhere is finished by running again: the person pressing the
// button again, the page's own continuation (`resume`), support from the
// desk, or the nightly run (resumeAccountDeletions). A request does at
// most ACCOUNT_DELETE_OPS storage operations (a Worker gets 1,000) and
// answers `running` with a key to carry on, so a large account is deleted
// over several requests rather than failing in the middle of one.
//
// What it refuses, before anything is touched:
// - the last owner of an active organisation: deleting would orphan it.
//   The answer names each organisation and what to do (make someone else
//   an owner, or close it). A closing or closed organisation is not
//   blocked: its owner already chose to end it.
// - a paid subscription this hub cannot cancel (no Stripe key). With the
//   key, the subscription is cancelled at Stripe first.
//
// What stays, and why, is in STAYS below and on the confirm step.

import { extendOrg } from "./org.js";
import { endForDeletion } from "./account-sessions.js";
import { eraseSync, syncSummary } from "./account-sync.js";
import { webPackKey } from "./web-player.js";

export const CONFIRM_PHRASE = "delete my account";
/// The sign-in a deletion needs: made within the last half hour.
export const FRESH_SECONDS = 30 * 60;
const DEFAULT_OPS = 800;
const TOMBSTONE_SECONDS = 90 * 24 * 3600;
const JOB = (id) => `acctdel:${id}`;
const ID = /^[A-Za-z0-9_.@-]{1,80}$/;

/// The order a deletion runs in. Organisations first (the one refusal that
/// can still appear mid-way), then money, then the person's things, and
/// the sign-ins, the ways in and the account record itself last -- so an
/// interrupted run leaves an account that can still be found and finished.
export const STEPS = [
  "orgs",
  "billing",
  "apps",
  "sync",
  "studio",
  "support",
  "mail",
  "keys",
  "referrals",
  "orgs-index",
  "sessions",
  "identity",
  "account",
];

const DELETES = [
  "Your account: name, email address, picture and sign-in methods",
  "Every sign-in, on every browser and computer",
  "Your conversations in Krate Studio on the web",
  "Your apps' data kept on all your devices (account sync), and its files",
  "Your messages to support, the reports and requests you sent us",
  "Your API key, your build history and your email settings",
  "Your invite link and referral record",
];
const STAYS = [
  "Apps and files on your own computers: they were always yours",
  "Organisations keep their own records of what you did there: their activity log, and what you uploaded or approved, under your GitHub login or email address",
  "Reports other people made about your apps, and any takedown record",
  "Payment records, without your name or address: Stripe keeps the receipts",
  "Counts with nothing about you in them: how many apps were opened, how many people signed in",
];

// ------------------------------------------------- the organisation's part

/// Leave every organisation one of `principals` is an active member of,
/// in that organisation's store (one transaction). `dry` only answers what
/// would happen. Refused for the last owner of an active organisation.
extendOrg({
  ops: {
    "account-leave"(ctx) {
      const org = this.org();
      if (!org) return this.ok({ exists: false, memberships: [] });
      let state = "active";
      try {
        const life = this.row("SELECT state FROM lifecycle WHERE id = 1");
        if (life && life.state) state = life.state;
      } catch (_) {
        // No lifecycle table: an organisation that never closed.
      }
      const mine = ctx.principals.map((p) => this.member(p)).filter((m) => m && m.state === "active");
      const owners = mine.filter((m) => m.role === "owner").length;
      const blocked = owners > 0 && state === "active" && this.activeOwners() - owners < 1;
      const view = {
        exists: true,
        slug: org.slug,
        name: org.name,
        state,
        memberships: mine.map((m) => ({ principal: m.principal, role: m.role })),
        blocked,
      };
      if (ctx.args.dry || blocked || !mine.length) return this.ok(view);
      for (const m of mine) {
        // Kept, marked removed, like any removal: their uploads and
        // approvals keep naming them in the organisation's own record.
        this.sql.exec(
          "UPDATE members SET state = 'removed', removed_at = ?, removed_by = ? WHERE principal = ?",
          ctx.now,
          "krate:account-deleted",
          m.principal,
        );
        this.audit(m.principal, "member.remove", `member:${m.principal}`, "allowed", "account deleted");
        for (const invite of this.rows("SELECT code, principal, role FROM invites WHERE issued_by = ? AND state = 'open'", m.principal)) {
          this.sql.exec(
            "UPDATE invites SET state = 'revoked', closed_at = ?, closed_by = ? WHERE code = ?",
            ctx.now,
            m.principal,
            invite.code,
          );
          this.audit(m.principal, "invite.revoke", `invite:${invite.principal}/${invite.role}`, "allowed", "issuer deleted their account");
        }
      }
      return this.ok({ ...view, left: true });
    },
  },
});

// ------------------------------------------------------------- the budget

class Yield extends Error {
  constructor() {
    super("this request's share of the work is done");
    this.yielded = true;
  }
}

class Refusal extends Error {
  constructor(status, body) {
    super(body.message);
    this.status = status;
    this.body = body;
  }
}

/// `env` with every storage call counted: past `limit`, each call throws
/// Yield, and the run saves where it got to and answers `running`.
function metered(env, limit) {
  const meter = { used: 0, over: false };
  const spend = () => {
    meter.used += 1;
    if (meter.used > limit) {
      meter.over = true;
      throw new Yield();
    }
  };
  const wrap = (store, methods) =>
    store &&
    new Proxy(store, {
      get(target, prop) {
        const value = Reflect.get(target, prop);
        if (typeof value !== "function") return value;
        if (!methods.includes(prop)) return value.bind(target);
        return (...args) => {
          spend();
          return value.apply(target, args);
        };
      },
    });
  const namespace = (ns) =>
    ns && {
      idFromName: (name) => ns.idFromName(name),
      get: (id) => {
        const stub = ns.get(id);
        return {
          fetch: (...args) => {
            spend();
            return stub.fetch(...args);
          },
        };
      },
    };
  meter.env = {
    ...env,
    APPS: wrap(env.APPS, ["get", "put", "delete", "list", "getWithMetadata"]),
    BUNDLES: wrap(env.BUNDLES, ["get", "put", "delete", "list", "head"]),
    ORGS: namespace(env.ORGS),
    SYNC_ACCOUNTS: namespace(env.SYNC_ACCOUNTS),
  };
  return meter;
}

// --------------------------------------------------------------- helpers

const json = (body, status = 200, headers = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "private, no-store", ...headers },
  });

function parse(raw, fallback = null) {
  try {
    const v = JSON.parse(raw);
    return v === undefined || v === null ? fallback : v;
  } catch (_) {
    return fallback;
  }
}

async function getJSON(env, key, fallback = null) {
  return parse(await env.APPS.get(key), fallback);
}

async function sha256Hex(text) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// Every key under `prefix`, a page at a time. With a job, the pages are
/// small enough for one request's share and the page it is on is
/// remembered, so a run that stops in the middle of a long listing carries
/// on from that page rather than starting it again.
async function eachKey(env, prefix, visit, job = null, tag = prefix) {
  let cursor = job && job.cursors ? job.cursors[tag] : undefined;
  const limit = job && job.page ? job.page : 1000;
  for (let page = 0; page < 100000; page += 1) {
    const listing = await env.APPS.list({ prefix, limit, cursor });
    for (const k of listing.keys || []) await visit(k.name);
    if (listing.list_complete || !listing.cursor) break;
    cursor = listing.cursor;
    if (job) job.cursors[tag] = cursor;
  }
  if (job && job.cursors) delete job.cursors[tag];
}

const lower = (s) => String(s || "").trim().toLowerCase();
const unique = (list) => [...new Set(list.filter(Boolean))];

/// Who an account is, as everything else names it: its id, the GitHub
/// logins that sign in to it (`ghlogin:` records), its addresses, the
/// principals it owns published apps by and is known to organisations by.
/// Kept in the job, because the account record goes before the job ends.
async function whoOf(env, user, githubLogin = "") {
  const id = String(user.id);
  const logins = new Set();
  await eachKey(env, "ghlogin:", async (key) => {
    if ((await env.APPS.get(key)) === id) logins.add(key.slice("ghlogin:".length));
  });
  if (githubLogin) logins.add(lower(githubLogin));
  const verified = unique((Array.isArray(user.verified_emails) ? user.verified_emails : []).map(lower));
  const emails = unique([lower(user.email), ...verified, lower(user.unverified_email)]);
  const github = [...logins].sort();
  return {
    id,
    login: String(user.login || ""),
    name: String(user.name || ""),
    email: String(user.email || ""),
    github,
    emails,
    owners: [`account:${id}`, ...github.map((l) => `github:${l}`)],
    org_principals: [...github.map((l) => `github:${l}`), ...verified.map((e) => `email:${e}`)],
    refcode: String(user.refcode || ""),
  };
}

/// Every app build this account published, and the records around them.
/// With a job, what was found so far is kept in it, and each listing goes
/// a page at a time, so the search survives a run that stops part-way.
async function publishedBy(env, deps, who, job = null) {
  const found = job
    ? (job.found ||= { builds: {}, lines: [], channels: [], bigversions: [], complete: false })
    : { builds: {}, lines: [], channels: [], bigversions: [] };
  const result = () => ({ builds: Object.values(found.builds), lines: found.lines, channels: found.channels, bigversions: found.bigversions });
  if (found.complete) return result();
  const owners = new Set(who.owners);
  const add = (hash, info = {}) => {
    if (!/^[0-9a-f]{64}$/.test(hash)) return;
    found.builds[hash] = { hash, ...(found.builds[hash] || {}), ...info };
  };
  const once = (list, value, same) => {
    if (!list.some(same)) list.push(value);
  };
  await eachKey(env, "app:", async (key) => {
    const row = await getJSON(env, key);
    if (!row || !owners.has(deps.ownerOfRecord(row))) return;
    add(key.slice(4), {
      name: row.name || "Untitled app",
      unlisted: Boolean(row.unlisted),
      superseded: Boolean(row.superseded),
      published: row.published || 0,
      line: row.line || "",
      listed: true,
    });
  }, job, "find:app");
  await eachKey(env, "owner:", async (key) => {
    const hash = key.slice(6);
    if (found.builds[hash]) return;
    const record = await getJSON(env, key);
    if (record && owners.has(deps.ownerOfRecord(record))) add(hash, { listed: false });
  }, job, "find:owner");
  await eachKey(env, "line:", async (key) => {
    const line = await getJSON(env, key);
    if (!line || !owners.has(line.owner)) return;
    once(found.lines, { id: line.id, owner: line.owner, app_id: line.app_id, name: line.name, current: line.current, versions: line.versions.length }, (l) => l.id === line.id);
    for (const v of line.versions || []) add(v.hash);
  }, job, "find:line");
  // Builds a GitHub publisher's moving names have pointed at, and big
  // apps' version records: earlier builds whose listings were retired.
  for (const login of who.github) {
    await eachKey(env, `channel:${login}/`, async (key) => {
      once(found.channels, key, (k) => k === key);
      const channel = await getJSON(env, key);
      if (!channel) return;
      add(String(channel.current || ""));
      for (const h of channel.history || []) {
        add(String((h && h.hash) || ""));
        add(String((h && h.previous) || ""));
      }
    }, job, `find:channel:${login}`);
    await eachKey(env, `bigversions:${login}:`, async (key) => {
      once(found.bigversions, key, (k) => k === key);
      for (const h of await getJSON(env, key, [])) add(String(h));
    }, job, `find:big:${login}`);
  }
  if (job) found.complete = true;
  return result();
}

/// The apps a person sees on the confirm step: one row per app.
function appRows(found, base) {
  const rows = found.builds.filter((b) => b.listed && !b.superseded);
  return rows
    .sort((a, b) => (b.published || 0) - (a.published || 0))
    .map((b) => ({
      name: b.name,
      url: `${base}/a/${b.line || b.hash.slice(0, 16)}`,
      unlisted: b.unlisted,
      published: b.published,
    }));
}

/// Remove one build, unless it is not this account's after all: another
/// owner's listing or ownership record keeps it whole, and an
/// organisation's private copy of the same bytes keeps the bytes.
async function removeBuild(env, deps, who, hash) {
  const row = await getJSON(env, `app:${hash}`);
  const record = await getJSON(env, `owner:${hash}`);
  for (const r of [row, record]) {
    const owner = r ? deps.ownerOfRecord(r) : null;
    if (owner && !who.owners.includes(owner)) return "theirs";
  }
  // The big tier's bytes and attachments, read from the listing: first.
  await deps.deleteBigParts(env, hash);
  const head = await env.BUNDLES.head(hash);
  const org = head && head.customMetadata && head.customMetadata.org;
  if (!org) await env.BUNDLES.delete(hash);
  await env.BUNDLES.delete(`shot:${hash}`);
  await env.BUNDLES.delete(`icon:${hash}`);
  await env.BUNDLES.delete(webPackKey(hash));
  for (const length of [12, 16]) {
    const alias = `alias:${hash.slice(0, length)}`;
    if ((await env.APPS.get(alias)) === hash) await env.APPS.delete(alias);
  }
  await env.APPS.delete(`told-opened:${hash}`);
  await env.APPS.delete(`app:${hash}`);
  await env.APPS.delete(`owner:${hash}`);
  return org ? "bytes-kept" : "removed";
}

/// A live paid subscription: one Stripe still charges for.
function liveSubscription(ent) {
  return Boolean(ent && ent.sub && ent.via !== "referral" && ["active", "trialing", "past_due", "unpaid", "incomplete"].includes(ent.status));
}

async function cancelAtStripe(env, sub) {
  try {
    const res = await fetch(`https://api.stripe.com/v1/subscriptions/${encodeURIComponent(sub)}`, {
      method: "DELETE",
      headers: { authorization: `Bearer ${env.STRIPE_SECRET_KEY}` },
    });
    if (res.ok) return true;
    const body = await res.json().catch(() => ({}));
    // Already cancelled, or never there: nothing left to charge.
    return res.status === 404 || (body.error && body.error.code === "resource_missing");
  } catch (_) {
    return false;
  }
}

// ----------------------------------------------------------------- steps

const STEP = {
  async orgs(env, deps, job) {
    const plan = await orgPlan(env, deps, job.who, { dry: false });
    if (plan.blocked.length) throw new Refusal(409, lastOwner(plan.blocked));
    job.counts.orgs_left = plan.orgs.filter((o) => o.left).length;
  },

  async billing(env, deps, job) {
    const id = job.who.id;
    const ent = await getJSON(env, `ent:${id}`);
    if (liveSubscription(ent)) {
      if (!env.STRIPE_SECRET_KEY || !(await cancelAtStripe(env, ent.sub))) {
        throw new Refusal(502, {
          error: "billing",
          message: "We could not cancel your subscription at Stripe just now, so nothing more was deleted. Try again in a few minutes, or write to yashraj@krate.tech.",
        });
      }
      job.counts.subscription_cancelled = 1;
    }
    if (ent) await env.APPS.delete(`ent:${id}`);
    const customers = new Set(ent && ent.customer ? [ent.customer] : []);
    await eachKey(env, "cust:", async (key) => {
      if ((await env.APPS.get(key)) === id) customers.add(key.slice(5));
    }, job, "cust");
    for (const c of customers) {
      if ((await env.APPS.get(`cust:${c}`)) === id) await env.APPS.delete(`cust:${c}`);
    }
    // Payment records stay for the books, without who paid: the invoice
    // id finds the receipt at Stripe.
    await eachKey(env, "pay:", async (key) => {
      const p = await getJSON(env, key);
      if (!p || !(p.userId === id || (p.email && job.who.emails.includes(lower(p.email))))) return;
      await env.APPS.put(key, JSON.stringify({ ...p, userId: "", email: "", detached: Date.now() }));
      job.counts.payments_detached = (job.counts.payments_detached || 0) + 1;
    }, job, "pay");
  },

  async apps(env, deps, job) {
    if (job.apps !== "remove") return;
    const found = await publishedBy(env, deps, job.who, job);
    if (job.counts.apps_removed === undefined) job.counts.apps_removed = appRows(found, "").length;
    job.removed = job.removed || [];
    for (const b of found.builds) {
      if (job.removed.includes(b.hash)) continue;
      const how = await removeBuild(env, deps, job.who, b.hash);
      if (how !== "theirs") job.counts.app_builds_removed = (job.counts.app_builds_removed || 0) + 1;
      job.removed.push(b.hash);
    }
    for (const line of found.lines) {
      const pointer = `lineof:${await sha256Hex(`${line.owner}\n${line.app_id}`)}`;
      if ((await env.APPS.get(pointer)) === line.id) await env.APPS.delete(pointer);
      await env.APPS.delete(`told-opened:${line.id}`);
      await env.APPS.delete(`line:${line.id}`);
    }
    for (const key of [...found.channels, ...found.bigversions]) await env.APPS.delete(key);
  },

  async sync(env, deps, job) {
    const erased = await eraseSync(env, job.who.id);
    job.counts.sync_files = (job.counts.sync_files || 0) + erased.bodies;
  },

  async studio(env, deps, job) {
    const owner = job.session_owner;
    if (!owner) return;
    await eachKey(env, `sess:${owner}:`, async (key) => {
      await env.APPS.delete(key);
      job.counts.conversations = (job.counts.conversations || 0) + 1;
    }, job, "sess");
  },

  async support(env, deps, job) {
    const { id, emails, owners, github, login } = job.who;
    const tickets = new Set(await getJSON(env, `utick:${id}`, []));
    await eachKey(env, "tick:", async (key) => {
      const t = await getJSON(env, key);
      if (t && (t.userId === id || (t.email && emails.includes(lower(t.email))))) tickets.add(key.slice(5));
    }, job, "tick");
    for (const t of tickets) {
      if (await env.APPS.get(`tick:${t}`)) job.counts.tickets = (job.counts.tickets || 0) + 1;
      await env.APPS.delete(`tick:${t}`);
    }
    await env.APPS.delete(`utick:${id}`);
    // Studio's support reports: by the owner recorded since K-1245, and
    // before that by the GitHub login they carried.
    await eachKey(env, "report:", async (key) => {
      const r = await getJSON(env, key);
      if (!r) return;
      const mine = r.owner ? owners.includes(r.owner) : r.from && github.includes(lower(r.from));
      if (!mine) return;
      await env.BUNDLES.delete(key);
      await env.APPS.delete(key);
      job.counts.reports = (job.counts.reports || 0) + 1;
    }, job, "report");
    await eachKey(env, "makeit:", async (key) => {
      const m = await getJSON(env, key);
      if (!m || !emails.includes(lower(m.email))) return;
      await env.APPS.delete(key);
      job.counts.requests = (job.counts.requests || 0) + 1;
    }, job, "makeit");
    for (const e of emails) await env.APPS.delete(`founding:${e}`);
    // Reports this person made about other people's apps stay -- they are
    // the evidence for a takedown -- without the login that named them.
    const names = new Set([...github, lower(login)].filter(Boolean));
    await eachKey(env, "app-report:", async (key) => {
      const record = await getJSON(env, key);
      if (!record || !Array.isArray(record.reports)) return;
      let changed = false;
      for (const r of record.reports) {
        if (r && r.login && names.has(lower(r.login))) {
          delete r.login;
          changed = true;
        }
      }
      if (changed) await env.APPS.put(key, JSON.stringify(record));
    }, job, "app-report");
  },

  async mail(env, deps, job) {
    await env.APPS.delete(`welcome-due:${job.who.id}`);
    // An announcement's per-inbox mark names the account it went to.
    await eachKey(env, "announced:", async (key) => {
      if ((await env.APPS.get(key)) === job.who.id) await env.APPS.delete(key);
    }, job, "announced");
  },

  async keys(env, deps, job) {
    const id = job.who.id;
    if (await env.APPS.get(`keys:${id}`)) job.counts.api_keys = 1;
    for (const key of [`keys:${id}`, `spend:${id}`, `mkacct:${id}`]) await env.APPS.delete(key);
  },

  async referrals(env, deps, job) {
    const { id, refcode } = job.who;
    if (refcode && (await env.APPS.get(`rcode:${refcode}`)) === id) await env.APPS.delete(`rcode:${refcode}`);
    await env.APPS.delete(`refs:${id}`);
    await env.APPS.delete(`refawards:${id}`);
    // Whoever invited them keeps the credit, not the name.
    const by = await env.APPS.get(`refby:${id}`);
    if (by) {
      const refs = await getJSON(env, `refs:${by}`, []);
      if (Array.isArray(refs) && refs.includes(id)) {
        await env.APPS.put(`refs:${by}`, JSON.stringify(refs.map((r) => (r === id ? "deleted" : r))));
      }
      await env.APPS.delete(`refby:${id}`);
    }
  },

  async "orgs-index"(env, deps, job) {
    for (const p of job.who.org_principals) await env.APPS.delete(`orgs-of:${p}`);
  },

  async sessions(env, deps, job) {
    const ended = await endForDeletion(env, job.who.id, deps.githubClientId, job.github_tokens || []);
    job.github_tokens = [];
    job.counts.sign_ins = (job.counts.sign_ins || 0) + ended.krate + ended.github;
    job.counts.github_not_revoked = (job.counts.github_not_revoked || 0) + ended.github_left;
  },

  async identity(env, deps, job) {
    const id = job.who.id;
    for (const prefix of ["ident:", "ghlogin:"]) {
      await eachKey(env, prefix, async (key) => {
        if ((await env.APPS.get(key)) === id) await env.APPS.delete(key);
      }, job, prefix);
    }
  },

  async account(env, deps, job) {
    await env.APPS.delete(`user:${job.who.id}`);
  },
};

// ------------------------------------------------------------ the plan

/// Each organisation the account is in, asked in its own store. `dry`
/// changes nothing.
async function orgPlan(env, deps, who, { dry }) {
  const out = { orgs: [], blocked: [] };
  if (!env.ORGS || !who.org_principals.length) return out;
  const slugs = new Set();
  for (const p of who.org_principals) {
    for (const o of await getJSON(env, `orgs-of:${p}`, [])) if (o && o.slug) slugs.add(o.slug);
  }
  for (const slug of [...slugs].sort()) {
    const answer = await deps.orgAsk(env, slug, { op: "account-leave", principals: who.org_principals, args: { dry } });
    const v = answer && answer.status === 200 ? answer.body : null;
    if (!v || !v.exists || !v.memberships.length) continue;
    const row = { slug: v.slug, name: v.name, roles: [...new Set(v.memberships.map((m) => m.role))], state: v.state, blocked: v.blocked, left: Boolean(v.left) };
    out.orgs.push(row);
    if (v.blocked) out.blocked.push(row);
  }
  return out;
}

function lastOwner(blocked) {
  const names = blocked.map((o) => (o.name && o.name !== o.slug ? `${o.name} (${o.slug})` : o.slug));
  const list = names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return {
    error: "last-owner",
    message:
      `You are the only owner of ${list}. Deleting your account would leave ${blocked.length === 1 ? "it" : "them"} with nobody in charge. ` +
      `Make someone else an owner, or close the organisation, then delete your account.`,
    orgs: blocked.map((o) => ({ slug: o.slug, name: o.name, url: `/orgs/${o.slug}/` })),
  };
}

/// What deleting would do, without doing any of it. Also what support sees.
async function preview(env, deps, user, who) {
  const base = deps.siteBase(env);
  const found = await publishedBy(env, deps, who);
  const plan = await orgPlan(env, deps, who, { dry: true });
  const ent = await getJSON(env, `ent:${who.id}`);
  const apps = appRows(found, deps.hubBase(env));
  return {
    account: { name: who.name, login: who.login, email: who.email },
    confirm_phrase: CONFIRM_PHRASE,
    apps,
    app_builds: found.builds.length,
    orgs: plan.orgs.map((o) => ({ slug: o.slug, name: o.name, roles: o.roles, state: o.state, blocked: o.blocked })),
    blocked: plan.blocked.length ? lastOwner(plan.blocked) : null,
    subscription: liveSubscription(ent)
      ? { cancels: Boolean(env.STRIPE_SECRET_KEY), message: env.STRIPE_SECRET_KEY ? "Your paid plan is cancelled at Stripe first." : "Your paid plan has to be cancelled first: write to yashraj@krate.tech." }
      : null,
    deletes: DELETES,
    stays: STAYS,
    keep_apps_means:
      "Your apps stay at their links, with your name on them. Once your account is gone, nobody can change or remove them from the website" +
      (who.github.length ? " -- unless you sign in again with the same GitHub account." : "; write to yashraj@krate.tech if you change your mind."),
    orgs_url: `${base}/orgs/`,
  };
}

function preflightRefusal(env, view) {
  if (view.blocked) return [409, view.blocked];
  if (view.subscription && !view.subscription.cancels) {
    return [409, { error: "billing", message: "Your account has a paid plan this hub cannot cancel by itself. Write to yashraj@krate.tech and we will cancel it and delete your account." }];
  }
  return null;
}

// ------------------------------------------------------------------ run

function newJob(who, { by, apps, sessionOwner, githubTokens }) {
  const key = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");
  return {
    id: who.id,
    state: "running",
    by,
    apps,
    started: Date.now(),
    touched: Date.now(),
    resume_key: key,
    session_owner: sessionOwner,
    github_tokens: githubTokens,
    done: [],
    cursors: {},
    counts: {},
    who,
  };
}

const save = (env, job) => env.APPS.put(JOB(job.id), JSON.stringify(job));

/// Run a job as far as this request's share allows. Answers
/// `{state: "done", counts}`, `{state: "running", resume}`, or throws a
/// Refusal (nothing more was done) or the error that stopped it (its
/// progress saved, so running it again carries on).
export async function runJob(env, deps, job, opsLimit) {
  const meter = metered(env, opsLimit);
  // A page of keys, each read and perhaps deleted, fits a share with room.
  job.page = Math.max(5, Math.min(200, Math.floor(opsLimit / 4)));
  try {
    for (const step of STEPS) {
      if (job.done.includes(step)) continue;
      await STEP[step](meter.env, deps, job);
      if (meter.over) throw new Yield();
      job.done.push(step);
      job.touched = Date.now();
      await save(env, job);
    }
  } catch (err) {
    job.touched = Date.now();
    if (err instanceof Refusal) {
      // Refused before anything was done: as if never asked.
      if (!job.done.length) await env.APPS.delete(JOB(job.id));
      else {
        job.problem = err.body.error;
        await save(env, job);
      }
      throw err;
    }
    if (err instanceof Yield || (err && err.yielded)) {
      await save(env, job);
      return { state: "running", resume: `${job.id}.${job.resume_key}`, done: job.done.length, of: STEPS.length };
    }
    job.problem = String((err && err.message) || err).slice(0, 300);
    await save(env, job).catch(() => {});
    throw err;
  }
  // Done. What stays is a marker with no personal data, so support can
  // answer "was it deleted, and when", and a second request is answered.
  const done = { id: job.id, state: "done", by: job.by, at: Date.now(), apps: job.apps, counts: job.counts };
  await env.APPS.put(JOB(job.id), JSON.stringify(done), { expirationTtl: TOMBSTONE_SECONDS });
  return { state: "done", counts: job.counts };
}

function opsLimit(env) {
  const n = Number(env.ACCOUNT_DELETE_OPS);
  return Number.isFinite(n) && n >= 20 ? n : DEFAULT_OPS;
}

function doneAnswer(result) {
  return json({
    state: "done",
    deleted: true,
    counts: result.counts,
    message: "Your account is deleted.",
    github_note:
      result.counts && result.counts.github_not_revoked
        ? "GitHub did not confirm that Krate's access was removed. Check GitHub Settings, Applications, and revoke Krate there."
        : null,
  });
}

async function runAndAnswer(env, deps, job) {
  try {
    const result = await runJob(env, deps, job, opsLimit(env));
    if (result.state === "running") return json(result, 202);
    return doneAnswer(result);
  } catch (err) {
    if (err instanceof Refusal) return json(err.body, err.status);
    console.error(`account deletion ${job.id}: ${err && err.stack ? err.stack : err}`);
    return json(
      {
        error: "unfinished",
        message: "Deleting stopped part of the way. Press Delete again to finish; if you cannot, it finishes by itself within a day.",
        resume: `${job.id}.${job.resume_key}`,
      },
      500,
    );
  }
}

/// The signed-in account behind a request, and the token it used.
async function signedIn(request, env, deps) {
  const bearer = (request.headers.get("authorization") || "").replace(/^Bearer\s+/i, "").trim();
  if (!bearer) return null;
  const user = await deps.authedUser(request, env);
  if (!user || !user.id) return null;
  let githubLogin = "";
  if (!bearer.startsWith("krs_")) {
    const identity = await deps.verifyGitHub(request, env).catch(() => null);
    if (identity && identity.github === true) githubLogin = identity.login || "";
  }
  return { user, bearer, githubLogin };
}

/// A sign-in made in the last half hour, by its own index entry.
async function freshSignIn(env, userId, bearer) {
  const meta = await getJSON(env, `usess:${userId}:${bearer}`);
  const at = meta && Number(meta.created_at);
  return Boolean(at && Date.now() / 1000 - at <= FRESH_SECONDS);
}

/// GET /me/delete, POST /me/delete, GET /me/export. A Response, or null
/// for any other path.
export async function accountDeleteRoute(request, url, env, deps) {
  const { pathname } = url;
  if (pathname === "/me/export" && request.method === "GET") return exportRoute(request, env, deps);
  if (pathname !== "/me/delete") return null;
  if (request.method !== "GET" && request.method !== "POST") return null;

  let body = {};
  if (request.method === "POST") {
    body = parse(await request.text().catch(() => ""), {});
    if (!body || typeof body !== "object") body = {};
  }

  // Carrying on a job this page started: the key it was given is the
  // whole permission, because by the last steps the sign-in is gone.
  if (request.method === "POST" && typeof body.resume === "string") {
    const [id, key] = body.resume.split(".");
    const job = ID.test(id || "") ? await getJSON(env, JOB(id)) : null;
    if (job && job.state === "done") return doneAnswer(job);
    if (!job || !key || job.resume_key !== key) return json({ error: "no-such-deletion", message: "There is no deletion to carry on." }, 404);
    return runAndAnswer(env, deps, job);
  }

  const who = await signedIn(request, env, deps);
  if (!who) return json({ error: "not-signed-in", message: "Sign in first." }, 401);
  const { user, bearer, githubLogin } = who;
  const running = await getJSON(env, JOB(user.id));
  const fresh = await freshSignIn(env, user.id, bearer);

  if (request.method === "GET") {
    const view = await preview(env, deps, user, await whoOf(env, user, githubLogin));
    return json({ ...view, fresh, fresh_minutes: FRESH_SECONDS / 60, running: Boolean(running && running.state === "running") });
  }

  if (running && running.state === "running") return runAndAnswer(env, deps, running);
  if (!fresh) {
    return json(
      { error: "sign-in-again", message: "For your safety, sign in again first. Then you have half an hour to delete your account." },
      403,
    );
  }
  if (lower(body.confirm) !== CONFIRM_PHRASE) {
    return json({ error: "confirm", message: `Type "${CONFIRM_PHRASE}" to confirm.` }, 400);
  }
  const identity = await whoOf(env, user, githubLogin);
  const view = await preview(env, deps, user, identity);
  const apps = body.apps === "keep" ? "keep" : body.apps === "remove" ? "remove" : view.app_builds ? null : "remove";
  if (!apps) return json({ error: "apps", message: "Say whether to remove your published apps or leave them up." }, 400);
  const refused = preflightRefusal(env, view);
  if (refused) return json(refused[1], refused[0]);
  const job = newJob(identity, {
    by: "self",
    apps,
    sessionOwner: deps.sessionOwner(user),
    githubTokens: bearer.startsWith("krs_") ? [] : [bearer],
  });
  await save(env, job);
  return runAndAnswer(env, deps, job);
}

/// The support desk's deletion (adminApi, behind KRATE_ADMINS): the same
/// job, the same refusals. `confirm` is the account's email address (or
/// its id when it has none), typed by the person at the desk.
export async function deskDelete(env, deps, body, admin, { dryRun }) {
  const id = String((body && body.id) || "");
  if (!ID.test(id)) return json({ error: "no-such-account", message: "No such account." }, 404);
  const running = await getJSON(env, JOB(id));
  if (running && running.state === "done") return json({ state: "done", deleted: true, at: running.at, counts: running.counts });
  if (running && running.state === "running" && !dryRun) return runAndAnswer(env, deps, running);
  const user = await getJSON(env, `user:${id}`);
  if (!user) return json({ error: "no-such-account", message: "No such account." }, 404);
  if (!user.id) user.id = id;
  const who = await whoOf(env, user);
  const view = await preview(env, deps, user, who);
  if (dryRun) return json({ ...view, id, running: Boolean(running) });
  const expected = user.email ? lower(user.email) : id;
  if (lower(body.confirm) !== expected) {
    return json({ error: "confirm", message: `Type the account's ${user.email ? "email address" : "id"} to confirm.` }, 400);
  }
  const apps = body.apps === "keep" ? "keep" : "remove";
  const refused = preflightRefusal(env, view);
  if (refused) return json(refused[1], refused[0]);
  const job = newJob(who, { by: `support:${admin.login}`, apps, sessionOwner: deps.sessionOwner(user), githubTokens: [] });
  await save(env, job);
  return runAndAnswer(env, deps, job);
}

/// The nightly run: finish every deletion a request left part-way and
/// nobody carried on for ten minutes.
export async function resumeAccountDeletions(env, deps, now = Date.now()) {
  let finished = 0;
  await eachKey(env, "acctdel:", async (key) => {
    const job = await getJSON(env, key);
    if (!job || job.state !== "running" || now - (job.touched || 0) < 10 * 60 * 1000) return;
    try {
      const result = await runJob(env, deps, job, opsLimit(env));
      if (result.state === "done") finished += 1;
    } catch (err) {
      console.error(`account deletion ${job.id} (nightly): ${err && err.message ? err.message : err}`);
    }
  });
  return { finished };
}

// ---------------------------------------------------------------- export

/// GET /me/export: one JSON file of everything the hub keeps about the
/// signed-in account. Never a token, a key's ciphertext, a ticket's
/// secret or a deletion's resume key; never the contents of synced data,
/// which the hub cannot read.
async function exportRoute(request, env, deps) {
  const who = await signedIn(request, env, deps);
  if (!who) return json({ error: "not-signed-in", message: "Sign in first." }, 401);
  const { user, githubLogin } = who;
  const id = String(user.id);
  const identity = await whoOf(env, user, githubLogin);
  const base = deps.hubBase(env);

  const sessions = [];
  await eachKey(env, `usess:${id}:`, async (key) => {
    const token = key.slice(`usess:${id}:`.length);
    if (token.startsWith("krs_") && !(await env.APPS.get(`session:${token}`))) return;
    const meta = await getJSON(env, key, {});
    sessions.push({ id: await deps.sessionId(token), kind: meta.kind || "", label: meta.label || "", created_at: meta.created_at || null });
  });

  const found = await publishedBy(env, deps, identity);
  const apps = [];
  for (const b of found.builds.filter((x) => x.listed)) {
    const row = await getJSON(env, `app:${b.hash}`, {});
    apps.push({
      hash: b.hash,
      url: `${base}/a/${b.hash.slice(0, 16)}`,
      name: row.name || "",
      description: row.description || "",
      category: row.category || "",
      unlisted: Boolean(row.unlisted),
      superseded: Boolean(row.superseded),
      published: row.published || 0,
      size: row.size || 0,
      line: row.line || null,
      version: row.version_n || null,
    });
  }

  const ent = await getJSON(env, `ent:${id}`);
  const payments = [];
  await eachKey(env, "pay:", async (key) => {
    const p = await getJSON(env, key);
    if (p && p.userId === id) payments.push({ amount: p.amount, currency: p.currency, invoice: p.invoice, at: p.at });
  });
  const keys = await getJSON(env, `keys:${id}`, {});

  const conversations = [];
  const sessOwner = deps.sessionOwner(user);
  if (sessOwner) {
    await eachKey(env, `sess:${sessOwner}:`, async (key) => {
      const s = await getJSON(env, key);
      if (s) conversations.push(s);
    });
  }

  const tickets = [];
  for (const t of await getJSON(env, `utick:${id}`, [])) {
    const ticket = await getJSON(env, `tick:${t}`);
    if (!ticket) continue;
    const { key: _secret, userId: _id, ...rest } = ticket;
    tickets.push(rest);
  }
  const reports = [];
  await eachKey(env, "report:", async (key) => {
    const r = await getJSON(env, key);
    const mine = r && (r.owner ? identity.owners.includes(r.owner) : r.from && identity.github.includes(lower(r.from)));
    if (mine) reports.push({ id: r.id, received: r.received, note: r.note, krate: r.krate, os: r.os, size: r.size, state: r.state });
  });
  const requests = [];
  await eachKey(env, "makeit:", async (key) => {
    const m = await getJSON(env, key);
    if (m && identity.emails.includes(lower(m.email))) requests.push({ at: m.at, email: m.email, request: m.request, answers: m.answers });
  });
  const founding = [];
  for (const e of identity.emails) {
    const f = await getJSON(env, `founding:${e}`);
    if (f) founding.push({ email: e, at: f.at });
  }

  const orgs = new Map();
  for (const p of identity.org_principals) {
    for (const o of await getJSON(env, `orgs-of:${p}`, [])) if (o && o.slug && !orgs.has(o.slug)) orgs.set(o.slug, { slug: o.slug, name: o.name, role: o.role, as: p });
  }

  const out = {
    format: "krate-account-export/1",
    exported_at: new Date().toISOString(),
    about:
      "Everything Krate's hub keeps about your account. Your apps' synced data is encrypted on your devices with a key the hub never holds, so only its counts and sizes are here. Organisations keep their own records; an owner can take an organisation's export.",
    account: {
      id,
      name: user.name || "",
      login: user.login || "",
      email: user.email || "",
      avatar_url: user.avatar_url || "",
      created: user.created || null,
      sign_in_methods: Array.isArray(user.providers) ? user.providers : [],
      proven_emails: Array.isArray(user.verified_emails) ? user.verified_emails : [],
      github_logins: identity.github,
      welcome_seen: Boolean(user.onboarded),
    },
    email_settings: { news: deps.wantsNews(user) },
    sign_ins: sessions,
    plan: ent ? { plan: ent.plan, status: ent.status || "", until: ent.until || 0, via: ent.via || "", stripe_customer: ent.customer || "" } : { plan: "free" },
    payments,
    api_keys: Object.entries(keys || {}).map(([vendor, k]) => ({ vendor, ends_with: (k && k.tail) || "", added: (k && k.added) || null })),
    build_costs: await getJSON(env, `spend:${id}`, []),
    organisations: [...orgs.values()],
    published_apps: apps,
    app_links: found.lines.map((l) => ({ url: `${base}/a/${l.id}`, name: l.name, app_id: l.app_id, versions: l.versions, current: l.current })),
    studio_conversations: conversations,
    support_tickets: tickets,
    support_reports: reports,
    requests_sent: requests,
    founding_list: founding,
    referral: { code: user.refcode || "", people_joined: (await getJSON(env, `refs:${id}`, [])).length },
    account_sync: await syncSummary(env, id).catch(() => null),
  };
  const day = new Date().toISOString().slice(0, 10);
  return json(out, 200, { "content-disposition": `attachment; filename="krate-account-${day}.json"` });
}
