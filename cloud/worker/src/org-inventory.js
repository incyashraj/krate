// Inventory, provenance and ownership (IC-954): every app an organisation
// has, with the record IT asks for.
//
// - **Provenance.** A candidate keeps the build record its bundle carries
//   (`ext/krate/provenance/provenance.json`, covered by the signature and
//   judged by the validator): which tool made it, with which AI provider
//   and model, when, and the request when the builder chose to keep it.
//   An app with none was made by hand.
// - **Inventory.** One row per app id: its name, the version that opens
//   now, its owner, who last sent it, how it was made, what it can do (the
//   IC-950 words), the company connections it uses (IC-952), who it is for,
//   how many people opened it in the last 30 days (and, for owners and
//   admins, who), and its state. Owners, admins and reviewers read it;
//   it is also a CSV.
// - **Ownership.** Each app id has an owner: by default whoever first sent
//   it. The owner is who IT asks about the app, not a permission: the role
//   matrix is unchanged. When the owner is removed the app needs an owner,
//   and an owner or admin assigns one; both are audited.
// - **Usage.** The hub sees every check-in of an organisation's app (the
//   look a device makes each time it opens one). It keeps, per day, which
//   people opened which release of which app -- never anything from inside
//   the app -- for 90 days. The inventory counts people from it, and
//   `/orgs/<slug>/usage` answers the daily counts by app and release for
//   the analysis that is built on them (IC-959).

import { Org, extendOrg, addReasons, allows, parsePrincipal, displayOf } from "./org.js";
// The release module decides what "current" is, and wraps the look this
// one counts; it must be installed first.
import { appOf } from "./org-releases.js";
import "./org-review.js";
// Days are the organisation's own (org-time.js, F-319).
import { dayBefore } from "./org-time.js";

const KEEP_DAYS = 90;
const RECENT_DAYS = 30;
const DAY = 86400;
const PROMPT_MAX = 16 * 1024;
const TEXT_MAX = 200;

addReasons({
  "no-such-app": {
    status: 404,
    message: (o, d) => d || `${o} has no app with that id`,
  },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS app_owners (
     app_id TEXT PRIMARY KEY, principal TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  // One row per person per release per app per day: the day's distinct
  // people, already aggregated. Nothing about what they did in the app.
  `CREATE TABLE IF NOT EXISTS usage_daily (
     day TEXT NOT NULL, app_id TEXT NOT NULL, digest TEXT NOT NULL,
     version TEXT NOT NULL, principal TEXT NOT NULL,
     PRIMARY KEY (day, app_id, digest, principal))`,
  `CREATE TABLE IF NOT EXISTS inventory_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)`,
];

const COLUMNS = [["candidates", "provenance", "TEXT"]];

const clip = (v, n = TEXT_MAX) => (v == null ? null : String(v).slice(0, n));

/// The build record a judgement carries, bounded, or null for an app made
/// by hand. The validator has already read it strictly; this only keeps
/// the hub's copy small.
export function provenanceArg(raw) {
  if (!raw || typeof raw !== "object") return null;
  const n = (v) => (Number.isFinite(Number(v)) ? Math.max(0, Math.floor(Number(v))) : 0);
  return {
    v: 1,
    tool: clip(raw.tool, 40) || "krate create",
    krate_version: clip(raw.krate_version, 40) || "",
    provider: clip(raw.provider, 40),
    model: clip(raw.model, 120),
    started_at: n(raw.started_at),
    finished_at: n(raw.finished_at),
    prompt_sha256: clip(raw.prompt_sha256, 64) || "",
    prompt: raw.prompt == null ? null : clip(raw.prompt, PROMPT_MAX),
    revisions: n(raw.revisions),
  };
}

function parsed(text, fallback) {
  try {
    const v = JSON.parse(text || "null");
    return v == null ? fallback : v;
  } catch (_) {
    return fallback;
  }
}

/// "built with Claude (claude-opus-5)", "by hand": how an app was made,
/// as a person reads it, the same words on every screen.
const PROVIDER_NAMES = {
  claude: "Claude",
  codex: "Codex",
  gemini: "Gemini",
  copilot: "Copilot",
  grok: "Grok",
  anthropic: "Anthropic's API",
  openai: "OpenAI's API",
  "author-cmd": "a command of the builder's own",
};
/// The app's rings, when it has any: a pilot group or a pinned group.
function rolloutOf(store, app) {
  if (typeof store.rolloutView !== "function") return null;
  const r = store.rolloutView(app);
  return r.pilot || r.pins.length ? r : null;
}

export function builtWords(p) {
  if (!p) return "by hand";
  const who = p.provider ? PROVIDER_NAMES[p.provider] || p.provider : null;
  const tool = p.tool === "krate studio" ? "Krate Studio" : "krate create";
  if (!who) return `${tool}, from a built-in template`;
  return `${tool} with ${who}${p.model ? ` (${p.model})` : ""}`;
}

export const csvCell = (v) => {
  const s = v == null ? "" : String(v);
  // A cell a spreadsheet would run as a formula is kept as text.
  const safe = /^[=+\-@\t\r]/.test(s) ? `'${s}` : s;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
};

export const CSV_COLUMNS = [
  "app_id",
  "name",
  "version",
  "state",
  "owner",
  "needs_owner",
  "last_sent_by",
  "built_with",
  "model",
  "can",
  "connections",
  "audience",
  "people_30_days",
];

const ops = {
  /// Keep the day's distinct people for a release, and let go of what is
  /// older than 90 days (at most once a day).
  noteUse(ctx, c) {
    if (!c || !ctx.actor) return;
    const day = this.orgDay(ctx.now);
    this.sql.exec(
      "INSERT OR IGNORE INTO usage_daily (day, app_id, digest, version, principal) VALUES (?, ?, ?, ?, ?)",
      day,
      appOf(c),
      c.digest,
      String(c.version || ""),
      ctx.actor,
    );
    const last = this.row("SELECT value FROM inventory_meta WHERE key = 'pruned'");
    if (!last || last.value !== day) {
      this.sql.exec("DELETE FROM usage_daily WHERE day < ?", dayBefore(this.orgDay(ctx.now), KEEP_DAYS));
      this.sql.exec(
        "INSERT INTO inventory_meta (key, value) VALUES ('pruned', ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        day,
      );
    }
  },

  /// The app's owner: the one assigned, else whoever first sent it.
  ownerOf(app) {
    const set = this.row("SELECT principal FROM app_owners WHERE app_id = ?", app);
    if (set) return set.principal;
    const first = this.rows("SELECT uploaded_by, app_id, name FROM candidates ORDER BY uploaded_at, rowid").find((c) => appOf(c) === app);
    return first ? first.uploaded_by : null;
  },

  inventoryRows(ctx, me) {
    const names = allows(me.role, "inventory.people") === "yes";
    const since = dayBefore(this.orgDay(ctx.now), RECENT_DAYS - 1);
    const all = this.rows("SELECT *, rowid FROM candidates ORDER BY uploaded_at DESC, rowid DESC");
    const byApp = new Map();
    for (const c of all) {
      const app = appOf(c);
      if (!app) continue;
      if (!byApp.has(app)) byApp.set(app, []);
      byApp.get(app).push(c);
    }
    const out = [];
    for (const [app, list] of byApp) {
      const currentDigest = this.currentOf(app);
      const current = currentDigest ? list.find((c) => c.digest === currentDigest) : null;
      const newest = list[0];
      // What the row is about: what opens now, else the newest version.
      const shown = current || newest;
      const state = current
        ? "released"
        : list.some((c) => c.state === "waiting-for-approval")
          ? "waiting"
          : list.some((c) => c.state === "withdrawn")
            ? "withdrawn"
            : "none";
      const owner = this.ownerOf(app);
      const ownerRow = this.member(owner);
      const ownerActive = Boolean(ownerRow && ownerRow.state === "active");
      const provenance = parsed(shown.provenance, null);
      const caps = parsed(shown.caps, []);
      const people = this.rows(
        "SELECT DISTINCT principal FROM usage_daily WHERE app_id = ? AND day >= ? ORDER BY principal",
        app,
        since,
      ).map((r) => r.principal);
      out.push({
        app_id: app,
        name: shown.name,
        version: shown.version,
        digest: shown.digest,
        state,
        owner: owner ? { principal: owner, display: displayOf(owner), active: ownerActive } : null,
        needs_owner: !ownerActive,
        last_sent_by: newest.uploaded_by,
        last_sent_at: newest.uploaded_at,
        built: provenance
          ? {
              tool: provenance.tool,
              provider: provenance.provider || null,
              model: provenance.model || null,
              finished_at: provenance.finished_at || null,
              revisions: provenance.revisions || 0,
              prompt: provenance.prompt || null,
              words: builtWords(provenance),
            }
          : null,
        built_words: builtWords(provenance),
        capabilities: (Array.isArray(caps) ? caps : []).map((x) => ({ cap: x.cap, words: x.words || x.cap })),
        connections: parsed(shown.connections, []),
        audience: parsed(shown.audience, null),
        // Who gets which version, when a pilot group or a pin is set
        // (org-rollout.js); null when everyone gets the current one.
        rollout: rolloutOf(this, app),
        people_30_days: people.length,
        ...(names ? { people: people.map((p) => ({ principal: p, display: displayOf(p) })) } : {}),
      });
    }
    out.sort((a, b) => Number(b.needs_owner) - Number(a.needs_owner) || String(a.name || a.app_id).localeCompare(String(b.name || b.app_id)));
    return out;
  },

  /// GET /orgs/<slug>/inventory: every app id, for owners, admins and
  /// reviewers; names of the people who opened each only for owners and
  /// admins.
  op_inventory(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "inventory.view", target);
    if (refusal) return refusal;
    if (allows(me.role, "inventory.view") !== "yes") {
      return this.deny(ctx, "inventory.view", target, "role-cannot", `a ${me.role} cannot see ${ctx.slug}'s inventory -- ask an owner or admin of ${ctx.slug}`);
    }
    return this.ok({ apps: this.inventoryRows(ctx, me), days: RECENT_DAYS, names: allows(me.role, "inventory.people") === "yes" });
  },

  /// The inventory as CSV, one row per app, the columns of CSV_COLUMNS.
  op_inventory_csv(ctx) {
    const answer = this.op_inventory(ctx);
    if (answer.status !== 200) return answer;
    const lines = [CSV_COLUMNS.join(",")];
    for (const a of answer.body.apps) {
      lines.push(
        [
          a.app_id,
          a.name,
          a.version,
          a.state,
          a.owner ? a.owner.display : "",
          a.needs_owner ? "yes" : "no",
          displayOf(a.last_sent_by),
          a.built_words,
          a.built ? a.built.model || "" : "",
          a.capabilities.map((c) => c.words).join("; "),
          (a.connections || []).join("; "),
          a.audience ? a.audience.join("; ") : "everyone",
          a.people_30_days,
        ]
          .map(csvCell)
          .join(","),
      );
    }
    return this.ok({ csv: lines.join("\r\n") + "\r\n" });
  },

  /// PUT /orgs/<slug>/apps/<app-id>/owner {principal}: an owner or admin
  /// names who IT asks about the app. Only an active member can own one.
  op_set_owner(ctx) {
    const app = String(ctx.args.app_id || "").trim().toLowerCase().slice(0, 120);
    const target = `app:${app}`;
    const { me, refusal } = this.standing(ctx, "app.owner", target);
    if (refusal) return refusal;
    if (allows(me.role, "app.owner") !== "yes") {
      return this.deny(ctx, "app.owner", target, "role-cannot", `a ${me.role} cannot choose who owns ${ctx.slug}'s apps -- ask an owner or admin of ${ctx.slug}`);
    }
    if (!this.rows("SELECT name, app_id FROM candidates").some((c) => appOf(c) === app)) {
      return this.deny(ctx, "app.owner", target, "no-such-app", `${ctx.slug} has no app called ${app || "that"}`);
    }
    const p = parsePrincipal(ctx.args.principal);
    const them = p ? this.member(p.principal) : null;
    if (!them || them.state !== "active") {
      return this.deny(ctx, "app.owner", target, "not-a-member", `${p ? displayOf(p.principal) : "that person"} is not a member of ${ctx.slug} -- invite them first`);
    }
    if (this.ownerOf(app) !== p.principal || !this.row("SELECT 1 AS y FROM app_owners WHERE app_id = ?", app)) {
      this.sql.exec(
        `INSERT INTO app_owners (app_id, principal, set_at, set_by) VALUES (?, ?, ?, ?)
         ON CONFLICT (app_id) DO UPDATE SET principal = excluded.principal, set_at = excluded.set_at, set_by = excluded.set_by`,
        app,
        p.principal,
        ctx.now,
        me.principal,
      );
      this.audit(me.principal, "app.owner", target, "allowed", p.principal);
    }
    return this.ok({ app_id: app, owner: { principal: p.principal, display: displayOf(p.principal), active: true } });
  },

  /// GET /orgs/<slug>/usage?days=N (N up to 90): per day, per app, how
  /// many people opened it, and how many on each release -- the daily
  /// counts the analysis panel reads (IC-959). Owners and admins.
  op_usage(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "usage.view", target);
    if (refusal) return refusal;
    if (allows(me.role, "usage.view") !== "yes") {
      return this.deny(ctx, "usage.view", target, "role-cannot", `a ${me.role} cannot see how ${ctx.slug}'s apps are used`);
    }
    const days = Math.min(KEEP_DAYS, Math.max(1, Math.floor(Number(ctx.args.days) || RECENT_DAYS)));
    const since = dayBefore(this.orgDay(ctx.now), days - 1);
    const rows = this.rows(
      `SELECT day, app_id, digest, version, COUNT(DISTINCT principal) AS people
       FROM usage_daily WHERE day >= ? GROUP BY day, app_id, digest, version ORDER BY day, app_id, version`,
      since,
    );
    const totals = this.rows(
      "SELECT day, app_id, COUNT(DISTINCT principal) AS people FROM usage_daily WHERE day >= ? GROUP BY day, app_id ORDER BY day, app_id",
      since,
    );
    const out = totals.map((t) => ({
      day: t.day,
      app_id: t.app_id,
      people: t.people,
      releases: rows.filter((r) => r.day === t.day && r.app_id === t.app_id).map((r) => ({ digest: r.digest, version: r.version, people: r.people })),
    }));
    return this.ok({ days, since, usage: out });
  },
};

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({ schema: SCHEMA, columns: COLUMNS, mutations: ["set-owner"] });
  const P = OrgClass.prototype;

  // A candidate keeps its build record; the first one of an app makes its
  // sender the owner.
  const add = P.op_add_candidate;
  P.op_add_candidate = function (ctx) {
    const answer = add.call(this, ctx);
    if (answer.status === 201) {
      const digest = String(ctx.args.digest || "");
      const record = provenanceArg(ctx.args.provenance);
      if (record) this.sql.exec("UPDATE candidates SET provenance = ? WHERE digest = ? AND provenance IS NULL", JSON.stringify(record), digest);
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
      const app = appOf(c);
      if (app && !this.row("SELECT 1 AS y FROM app_owners WHERE app_id = ?", app) && !this.rows("SELECT name, app_id, digest FROM candidates").some((o) => o.digest !== digest && appOf(o) === app)) {
        this.sql.exec("INSERT INTO app_owners (app_id, principal, set_at, set_by) VALUES (?, ?, ?, ?)", app, ctx.actor, ctx.now, ctx.actor);
      }
    }
    return answer;
  };

  // The candidates list says how each was made, for the approver.
  const list = P.op_candidates;
  P.op_candidates = function (ctx) {
    const answer = list.call(this, ctx);
    if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
    answer.body = answer.body.map((r) => {
      const c = this.row("SELECT provenance FROM candidates WHERE digest = ?", r.digest);
      const p = parsed(c && c.provenance, null);
      return { ...r, provenance: p, built_words: builtWords(p) };
    });
    return answer;
  };

  // Every look a device makes when it opens an app is a check-in: the
  // day's people for that release are counted (nothing else is kept).
  const view = P.op_view;
  P.op_view = function (ctx) {
    const answer = view.call(this, ctx);
    if (answer.status === 200) {
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", String(ctx.args.digest || ""));
      if (c && c.state === "released") this.noteUse(ctx, c);
    }
    return answer;
  };

  // Removing someone leaves the apps they owned needing an owner: said in
  // the audit, and shown first in the inventory until one is assigned.
  const remove = P.op_remove;
  P.op_remove = function (ctx) {
    const who = this.targetOf(ctx);
    const answer = remove.call(this, ctx);
    if (answer.status === 204 && who) {
      const apps = new Set(this.rows("SELECT name, app_id FROM candidates").map(appOf).filter(Boolean));
      for (const app of apps) {
        if (this.ownerOf(app) === who) {
          this.audit(ctx.actor, "app.owner", `app:${app}`, "allowed", `needs an owner: ${who} was removed`);
        }
      }
    }
    return answer;
  };
}

install(Org);

// ---------------------------------------------------------------- routes

/// /orgs/<slug>/inventory, /orgs/<slug>/inventory.csv,
/// /orgs/<slug>/apps/<app-id>/owner and /orgs/<slug>/usage. Answers a
/// Response, or null.
export async function inventoryRoute(h) {
  const { url, rest, method, askRaw, mutateRaw, orgAnswer } = h;
  if (rest[0] === "inventory" && rest.length === 1 && method === "GET") return orgAnswer(await askRaw("inventory"));
  if (rest[0] === "inventory.csv" && rest.length === 1 && method === "GET") {
    const answer = await askRaw("inventory-csv");
    if (answer.status !== 200) return orgAnswer(answer);
    return new Response(answer.body.csv, {
      status: 200,
      headers: {
        "content-type": "text/csv; charset=utf-8",
        "content-disposition": `attachment; filename="${h.slug}-apps.csv"`,
        "cache-control": "no-store",
      },
    });
  }
  if (rest[0] === "apps" && rest.length === 3 && rest[2] === "owner" && method === "PUT") {
    return orgAnswer(await mutateRaw("set-owner", (b) => ({ app_id: rest[1], principal: b.principal })));
  }
  if (rest[0] === "usage" && rest.length === 1 && method === "GET") return orgAnswer(await askRaw("usage", { days: url.searchParams.get("days") }));
  return null;
}
