// Audit export over a signed webhook (IC-955).
//
// An owner gives the organisation a webhook URL in Settings. From then on
// every event appended to the organisation's audit chain is POSTed there as
// JSON, one event per request, in chain order, so a SIEM (Splunk, Datadog,
// Sentinel, anything that takes an HTTP event) holds the same record the
// hub does and can check the chain itself: each event carries its
// `prev_hash` and `hash`.
//
// The request is signed the way Stripe signs its webhooks:
//
//   X-Krate-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256>
//
// where the MAC is over the bytes `<t>.<body>` with the secret as the key
// (its UTF-8 bytes, `whsec_...` included). The secret is shown once, when it
// is made, and stored encrypted with ORG_SECRET_KEY (AES-GCM, the
// organisation's slug bound in as additional data, as company sign-in's
// client secret is). A receiver checks the MAC and refuses a `t` more than a
// few minutes old, so a captured request cannot be replayed later.
// `X-Krate-Event-Id` (`<slug>:<seq>`) lets it drop a repeat.
//
// Delivery cannot happen inside the store's transaction (it cannot wait on
// the network), so an event is queued in the same transaction that appends
// it, and the store's Durable Object alarm delivers the queue: the oldest
// event first, the next only once it is accepted (any 2xx). A refusal, a
// redirect, a timeout or a network failure keeps it queued and retries with
// backoff (30 s, doubling, at most an hour apart). An event still not
// delivered 24 hours after it was queued is given up and counted, so a dead
// receiver never holds the queue forever. The state is on the dashboard;
// the audit itself is never touched by any of this.
//
// Presets (G19): the same events, in the order and with the retries above,
// shaped for a SIEM that has its own intake, so nothing sits in between:
//
// - "generic": the signed JSON above, to any https receiver.
// - "splunk": Splunk's HTTP Event Collector. POST <url>/services/collector/event
//   with `Authorization: Splunk <HEC token>` and the HEC envelope
//   {time, host, source, sourcetype, index?, event}; `event` is the generic body.
// - "sentinel": Microsoft Sentinel through the Azure Monitor Logs Ingestion
//   API, the way Microsoft recommends now (the older HTTP Data Collector API
//   is deprecated and stops in September 2026). The owner makes a Microsoft
//   Entra app (tenant id, client id, client secret), a data collection
//   endpoint and a data collection rule whose stream (Custom-KrateAudit_CL,
//   say) has the columns TimeGenerated (datetime), Org, Actor, Action,
//   Target, Result, Reason, PrevHash, Hash (string), Seq and Attempt (int),
//   and gives the app "Monitoring Metrics Publisher" on the rule. The hub
//   takes a token with the client credentials (scope
//   https://monitor.azure.com//.default) and POSTs a one-row JSON array to
//   <endpoint>/dataCollectionRules/<dcr id>/streams/<stream>?api-version=2023-01-01.
// - "datadog": Datadog's Logs intake for the chosen site,
//   https://http-intake.logs.<site>/api/v2/logs, with `DD-API-KEY`; one log
//   with ddsource "krate", the org as a tag, and the event under `krate`.
//
// Every preset's request carries X-Krate-Signature and X-Krate-Event-Id as
// the generic one does. Its token (HEC token, client secret, API key) is
// sealed like the signing secret and never shown back.

import { Org, extendOrg, allows } from "./org.js";
import { sealSecret, openSecret, reachable, devHub } from "./org-sso.js";

export const SIEM_PRESETS = ["generic", "splunk", "sentinel", "datadog"];
/// Datadog's sites, by the name its pages use; the intake is
/// http-intake.logs.<site>.
export const DATADOG_SITES = {
  "datadoghq.com": "US1",
  "us3.datadoghq.com": "US3",
  "us5.datadoghq.com": "US5",
  "datadoghq.eu": "EU",
  "ap1.datadoghq.com": "AP1",
  "ap2.datadoghq.com": "AP2",
  "ddog-gov.com": "US1-FED",
};
/// The Azure clouds a Logs Ingestion endpoint can be in: where its tokens
/// come from, and the scope they are for.
const AZURE_CLOUDS = {
  com: { login: "https://login.microsoftonline.com", scope: "https://monitor.azure.com//.default" },
  us: { login: "https://login.microsoftonline.us", scope: "https://monitor.azure.us//.default" },
  cn: { login: "https://login.chinacloudapi.cn", scope: "https://monitor.azure.cn//.default" },
};
const INGEST_HOST = /^[a-z0-9-]{1,63}(\.[a-z0-9-]{1,63}){0,4}\.ingest\.monitor\.azure\.(com|us|cn)$/;
const GUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export const EXPORT_GIVE_UP_SECONDS = 24 * 60 * 60;
export const EXPORT_BATCH = 25;
const BACKOFF_FIRST = 30;
const BACKOFF_MAX = 60 * 60;
const DELIVERY_TIMEOUT_MS = 10_000;
const URL_MAX = 2000;

/// How long after the `n`th failed attempt the next one waits.
export function backoffSeconds(n) {
  return Math.min(BACKOFF_MAX, BACKOFF_FIRST * 2 ** Math.max(0, n - 1));
}

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS audit_export (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     url TEXT NOT NULL, host TEXT NOT NULL, secret TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL,
     delivered INTEGER NOT NULL DEFAULT 0, given_up INTEGER NOT NULL DEFAULT 0,
     last_delivered_at INTEGER, last_delivered_seq INTEGER,
     last_error TEXT, last_error_at INTEGER, last_status INTEGER,
     preset TEXT, preset_fields TEXT, preset_secret TEXT)`,
  `CREATE TABLE IF NOT EXISTS audit_outbox (
     seq INTEGER PRIMARY KEY, queued_at INTEGER NOT NULL,
     attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL)`,
];

const ops = {
  exportConfig() {
    return this.row("SELECT * FROM audit_export WHERE id = 1");
  },

  exportView() {
    const c = this.exportConfig();
    if (!c) return null;
    // The next attempt is the oldest event's: the rest wait behind it.
    const q = this.row(
      "SELECT COUNT(*) AS n, MIN(queued_at) AS oldest, (SELECT next_at FROM audit_outbox ORDER BY seq LIMIT 1) AS next FROM audit_outbox",
    );
    return {
      url: c.url,
      host: c.host,
      preset: c.preset || "generic",
      fields: parsedFields(c.preset_fields),
      has_token: Boolean(c.preset_secret),
      has_secret: true,
      set_at: c.set_at,
      set_by: c.set_by,
      signature: "X-Krate-Signature: t=<unix>,v1=<hex HMAC-SHA256 of \"<t>.<body>\">",
      delivery: {
        pending: q.n,
        oldest_pending_at: q.n ? q.oldest : null,
        next_attempt_at: q.n ? q.next : null,
        delivered: c.delivered,
        given_up: c.given_up,
        last_delivered_at: c.last_delivered_at || null,
        last_delivered_seq: c.last_delivered_seq || null,
        last_error: c.last_error || null,
        last_error_at: c.last_error_at || null,
        last_status: c.last_status || null,
      },
    };
  },

  exportMay(ctx, action, rule) {
    const { me, refusal } = this.standing(ctx, action, "audit-export");
    if (refusal) return { refusal };
    if (allows(me.role, rule) !== "yes") {
      const words = rule === "audit-export.view" ? `only an owner or admin of ${ctx.slug} sees the audit export` : `only an owner of ${ctx.slug} sets up the audit export`;
      return { refusal: this.deny(ctx, action, "audit-export", "role-cannot", words) };
    }
    return { me };
  },

  /// GET /orgs/<slug>/audit-export (owner, admin): null, or the settings and
  /// the delivery state. Never the secret.
  "audit-export-get"(ctx) {
    const { refusal } = this.exportMay(ctx, "audit-export.view", "audit-export.view");
    if (refusal) return refusal;
    return this.ok(this.exportView());
  },

  /// PUT /orgs/<slug>/audit-export {url} (owner). The Worker checked the
  /// URL and, the first time, made and sealed the secret. Events from this
  /// one on are sent; the setting itself is the first.
  "audit-export-set"(ctx) {
    const { me, refusal } = this.exportMay(ctx, "audit-export.set", "audit-export.manage");
    if (refusal) return refusal;
    const before = this.exportConfig();
    const url = String(ctx.args.url || "");
    const host = String(ctx.args.host || "");
    const secret = ctx.args.secret ? JSON.stringify(ctx.args.secret) : before ? before.secret : null;
    if (!url || !host || !secret) return { status: 400, body: { error: "bad-request", message: "give the webhook's https URL" } };
    const preset = SIEM_PRESETS.includes(ctx.args.preset) ? ctx.args.preset : "generic";
    // A preset's token is kept while the preset stays the same; a new
    // preset needs its own.
    const samePreset = before && (before.preset || "generic") === preset;
    const token = preset === "generic" ? null : ctx.args.preset_secret ? JSON.stringify(ctx.args.preset_secret) : samePreset ? before.preset_secret : null;
    if (preset !== "generic" && !token) {
      return { status: 400, body: { error: "bad-request", message: `give the ${PRESET_WORDS[preset].token} for ${PRESET_WORDS[preset].name}` } };
    }
    const fields = JSON.stringify(ctx.args.fields && typeof ctx.args.fields === "object" ? ctx.args.fields : {});
    if (before) {
      this.sql.exec(
        "UPDATE audit_export SET url = ?, host = ?, secret = ?, set_at = ?, set_by = ?, preset = ?, preset_fields = ?, preset_secret = ? WHERE id = 1",
        url,
        host,
        secret,
        ctx.now,
        me.principal,
        preset,
        fields,
        token,
      );
    } else {
      this.sql.exec(
        "INSERT INTO audit_export (id, url, host, secret, set_at, set_by, preset, preset_fields, preset_secret) VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?)",
        url,
        host,
        secret,
        ctx.now,
        me.principal,
        preset,
        fields,
        token,
      );
    }
    // The host only: a URL may carry the receiver's own key in its query.
    this.audit(me.principal, before ? "audit-export.change" : "audit-export.set", `audit-export:${host}`, "allowed", preset);
    return this.ok(this.exportView(), before ? 200 : 201);
  },

  /// POST /orgs/<slug>/audit-export/secret (owner): a new secret, sealed by
  /// the Worker; the old one stops signing at once.
  "audit-export-secret"(ctx) {
    const { me, refusal } = this.exportMay(ctx, "audit-export.secret", "audit-export.manage");
    if (refusal) return refusal;
    const c = this.exportConfig();
    if (!c) return { status: 404, body: { error: "audit-export-off", message: `${ctx.slug} has no audit export yet -- set its webhook URL first` } };
    this.sql.exec("UPDATE audit_export SET secret = ? WHERE id = 1", JSON.stringify(ctx.args.secret));
    this.audit(me.principal, "audit-export.secret", `audit-export:${c.host}`, "allowed");
    return this.ok(this.exportView());
  },

  /// DELETE /orgs/<slug>/audit-export (owner): stop sending. What was still
  /// queued is not sent, and the audit says how much.
  "audit-export-clear"(ctx) {
    const { me, refusal } = this.exportMay(ctx, "audit-export.clear", "audit-export.manage");
    if (refusal) return refusal;
    const c = this.exportConfig();
    if (c) {
      const dropped = this.row("SELECT COUNT(*) AS n FROM audit_outbox").n;
      this.sql.exec("DELETE FROM audit_export WHERE id = 1");
      this.sql.exec("DELETE FROM audit_outbox");
      this.audit(me.principal, "audit-export.clear", `audit-export:${c.host}`, "allowed", `${dropped} not sent`);
    }
    return this.ok(null, 204);
  },

  /// POST /orgs/<slug>/audit-export/test (owner): "Send a test event". It is
  /// a real audit event, so it goes the way every other one does.
  "audit-export-test"(ctx) {
    const { me, refusal } = this.exportMay(ctx, "audit-export.test", "audit-export.manage");
    if (refusal) return refusal;
    const c = this.exportConfig();
    if (!c) return { status: 404, body: { error: "audit-export-off", message: `${ctx.slug} has no audit export yet -- set its webhook URL first` } };
    const event = this.audit(me.principal, "audit-export.test", `audit-export:${c.host}`, "allowed");
    return this.ok({ queued: event.seq, export: this.exportView() }, 202);
  },

  // ---------------------------------------------------- the delivery side

  /// Make sure the alarm will run when the next queued event is due. Called
  /// after every command, outside its transaction.
  async exportArm(force = false) {
    // When the oldest event is due: the rest wait behind it, so waking for
    // a later one that is "due" would only spin.
    const head = this.row("SELECT next_at AS t FROM audit_outbox ORDER BY seq LIMIT 1");
    if (!head) return;
    const next = head.t;
    const storage = this.ctx.storage;
    if (typeof storage.setAlarm !== "function") return;
    const want = next * 1000;
    // Inside the alarm itself the one firing may still read as set, so the
    // alarm sets the next one whatever it reads.
    const have = force ? null : await storage.getAlarm();
    if (have === null || have === undefined || have > want) await storage.setAlarm(want);
  },

  /// One delivery run (the Durable Object's alarm). Answers what it did, for
  /// the tests.
  async exportDeliver() {
    const done = { delivered: 0, failed: 0, given_up: 0 };
    const c = this.exportConfig();
    const o = this.org();
    if (!c || !o) {
      this.sql.exec("DELETE FROM audit_outbox");
      return done;
    }
    const now = this.now();
    // Given up: queued more than a day ago and still not taken.
    this.ctx.storage.transactionSync(() => {
      const old = this.row("SELECT COUNT(*) AS n FROM audit_outbox WHERE queued_at + ? <= ?", EXPORT_GIVE_UP_SECONDS, now).n;
      if (old) {
        this.sql.exec("DELETE FROM audit_outbox WHERE queued_at + ? <= ?", EXPORT_GIVE_UP_SECONDS, now);
        this.sql.exec("UPDATE audit_export SET given_up = given_up + ? WHERE id = 1", old);
        done.given_up = old;
      }
    });
    const secret = await openSecret(this.env, o.slug, JSON.parse(c.secret), `krate-org-webhook:${o.slug}`);
    const preset = c.preset || "generic";
    const fields = parsedFields(c.preset_fields);
    // The preset's credential, and for Sentinel the token it buys: once a run.
    let cred = null;
    let credProblem = null;
    if (preset !== "generic") {
      const token = c.preset_secret ? await openSecret(this.env, o.slug, JSON.parse(c.preset_secret), `krate-org-siem:${o.slug}`) : null;
      if (!token) credProblem = `the ${PRESET_WORDS[preset].token} cannot be opened on this hub -- give it again in Settings`;
      else if (preset === "sentinel") {
        const got = await sentinelAccess(this, fields, token);
        if (got.problem) credProblem = got.problem;
        else cred = got.token;
      } else cred = token;
    }
    for (let i = 0; i < EXPORT_BATCH; i++) {
      const head = this.row("SELECT * FROM audit_outbox ORDER BY seq LIMIT 1");
      if (!head || head.next_at > now) break;
      const event = this.row("SELECT seq, at, actor, action, target, result, reason, prev_hash, hash FROM audit WHERE seq = ?", head.seq);
      let ok = false;
      let status = null;
      let problem = null;
      if (!event) {
        ok = true; // Nothing to send (cannot happen: the audit is never edited).
      } else if (secret === null) {
        problem = "the signing secret cannot be opened on this hub -- make a new secret in Settings";
      } else if (credProblem) {
        problem = credProblem;
      } else {
        const shaped = presetRequest(preset, fields, o.slug, event, head.attempts + 1, cred);
        const body = shaped.body;
        const t = String(now);
        try {
          const res = await fetch(c.url, {
            method: "POST",
            headers: {
              "content-type": "application/json",
              "user-agent": "Krate-Audit-Export/1",
              "x-krate-event-id": `${o.slug}:${event.seq}`,
              "x-krate-signature": `t=${t},v1=${await hmacHex(secret, `${t}.${body}`)}`,
              ...shaped.headers,
            },
            body,
            redirect: "manual",
            signal: AbortSignal.timeout(DELIVERY_TIMEOUT_MS),
          });
          status = res.status;
          // Read and drop the answer, so the connection is freed.
          await res.arrayBuffer().catch(() => {});
          ok = status >= 200 && status < 300;
          if (!ok) problem = status >= 300 && status < 400 ? `the receiver answered a redirect (${status}); redirects are not followed` : `the receiver answered ${status}${status === 401 || status === 403 ? ` -- check the ${PRESET_WORDS[preset].token}` : ""}`;
          // A Sentinel token that ran out early is asked for again next run.
          if (status === 401 && preset === "sentinel") this.sentinelCache = null;
        } catch (err) {
          problem = err && err.name === "TimeoutError" ? "the receiver did not answer within 10 seconds" : `the receiver could not be reached (${String((err && err.message) || err).slice(0, 120)})`;
        }
      }
      const stillQueued = this.ctx.storage.transactionSync(() => {
        // The export may have been switched off or changed while this waited.
        const now2 = this.exportConfig();
        if (!now2 || now2.url !== c.url) return false;
        if (ok) {
          this.sql.exec("DELETE FROM audit_outbox WHERE seq = ?", head.seq);
          this.sql.exec(
            "UPDATE audit_export SET delivered = delivered + 1, last_delivered_at = ?, last_delivered_seq = ?, last_status = ? WHERE id = 1",
            now,
            head.seq,
            status,
          );
          return true;
        }
        const attempts = head.attempts + 1;
        this.sql.exec("UPDATE audit_outbox SET attempts = ?, next_at = ? WHERE seq = ?", attempts, now + backoffSeconds(attempts), head.seq);
        this.sql.exec("UPDATE audit_export SET last_error = ?, last_error_at = ?, last_status = ? WHERE id = 1", problem, now, status);
        return true;
      });
      if (!stillQueued) break;
      if (!ok) {
        done.failed += 1;
        break;
      }
      done.delivered += 1;
    }
    return done;
  },
};

const PRESET_WORDS = {
  generic: { name: "your receiver", token: "signing secret" },
  splunk: { name: "Splunk", token: "HEC token" },
  sentinel: { name: "Microsoft Sentinel", token: "client secret" },
  datadog: { name: "Datadog", token: "API key" },
};

function parsedFields(raw) {
  try {
    const v = JSON.parse(raw || "{}");
    return v && typeof v === "object" && !Array.isArray(v) ? v : {};
  } catch (_) {
    return {};
  }
}

const isoOf = (seconds) => new Date(seconds * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");

/// One event's request body and extra headers, in the preset's shape.
/// `cred` is the preset's token (Splunk, Datadog) or the access token
/// (Sentinel).
export function presetRequest(preset, fields, slug, event, attempt, cred) {
  const generic = { type: "krate.audit.event", v: 1, org: slug, attempt, event };
  if (preset === "splunk") {
    const envelope = { time: event.at, host: "krate", source: `krate:${slug}`, sourcetype: fields.sourcetype || "krate:audit", event: generic };
    if (fields.index) envelope.index = fields.index;
    return { body: JSON.stringify(envelope), headers: { authorization: `Splunk ${cred}` } };
  }
  if (preset === "sentinel") {
    const row = {
      TimeGenerated: isoOf(event.at),
      Org: slug,
      Seq: event.seq,
      Actor: event.actor || "",
      Action: event.action,
      Target: event.target || "",
      Result: event.result,
      Reason: event.reason || "",
      PrevHash: event.prev_hash,
      Hash: event.hash,
      Attempt: attempt,
    };
    return { body: JSON.stringify([row]), headers: { authorization: `Bearer ${cred}` } };
  }
  if (preset === "datadog") {
    const log = {
      ddsource: "krate",
      service: fields.service || "krate",
      hostname: "krate-hub",
      ddtags: `org:${slug},action:${event.action},result:${event.result}`,
      timestamp: event.at * 1000,
      message: [event.actor || "krate", event.action, event.target || "", event.result].filter(Boolean).join(" "),
      krate: generic,
    };
    return { body: JSON.stringify([log]), headers: { "dd-api-key": cred } };
  }
  return { body: JSON.stringify(generic), headers: {} };
}

/// A Logs Ingestion token for Sentinel, by the client credentials, kept on
/// the store for the run after it while it lasts. `{token}` or `{problem}`.
async function sentinelAccess(store, fields, clientSecret) {
  const key = `${fields.tenant_id}/${fields.client_id}`;
  const now = store.now();
  if (store.sentinelCache && store.sentinelCache.key === key && store.sentinelCache.until > now + 60) return { token: store.sentinelCache.token };
  const cloud = AZURE_CLOUDS[(String(fields.endpoint || "").match(/\.azure\.(com|us|cn)$/) || [])[1]] || AZURE_CLOUDS.com;
  try {
    const res = await fetch(`${cloud.login}/${encodeURIComponent(fields.tenant_id)}/oauth2/v2.0/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ client_id: fields.client_id, client_secret: clientSecret, scope: cloud.scope, grant_type: "client_credentials" }).toString(),
      redirect: "manual",
      signal: AbortSignal.timeout(DELIVERY_TIMEOUT_MS),
    });
    const got = await res.json().catch(() => ({}));
    if (!res.ok || !got.access_token) {
      return { problem: `Microsoft did not give a token (${res.status}${got.error ? `, ${String(got.error).slice(0, 60)}` : ""}) -- check the tenant id, client id and client secret` };
    }
    store.sentinelCache = { key, token: got.access_token, until: now + (Number(got.expires_in) || 3600) };
    return { token: got.access_token };
  } catch (err) {
    return { problem: `Microsoft's sign-in could not be reached (${String((err && err.message) || err).slice(0, 120)})` };
  }
}

const b64url = (bytes) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/// HMAC-SHA256 of `text` under `secret` (its UTF-8 bytes), as lower hex.
export async function hmacHex(secret, text) {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(text));
  return [...new Uint8Array(mac)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// Check a delivery the way a receiver should: the MAC over `<t>.<body>`,
/// in constant time, and `t` within `tolerance` seconds of now. For the
/// docs and the tests.
export async function verifySignature(secret, header, body, { now = Math.floor(Date.now() / 1000), tolerance = 300 } = {}) {
  const parts = Object.fromEntries(
    String(header || "")
      .split(",")
      .map((p) => p.trim().split("="))
      .filter((kv) => kv.length === 2),
  );
  const t = Number(parts.t);
  if (!Number.isInteger(t) || !/^[0-9a-f]{64}$/.test(parts.v1 || "")) return false;
  if (Math.abs(now - t) > tolerance) return false;
  const want = await hmacHex(secret, `${parts.t}.${body}`);
  let diff = 0;
  for (let i = 0; i < 64; i++) diff |= want.charCodeAt(i) ^ parts.v1.charCodeAt(i);
  return diff === 0;
}

/// A new webhook secret: `whsec_` and 256 random bits.
export function newWebhookSecret() {
  return `whsec_${b64url(crypto.getRandomValues(new Uint8Array(32)))}`;
}

function install(OrgClass) {
  const helpers = {};
  const commands = {};
  for (const [k, fn] of Object.entries(ops)) (k.startsWith("audit-export-") ? commands : helpers)[k] = fn;
  Object.assign(OrgClass.prototype, helpers);
  extendOrg({
    schema: SCHEMA,
    // Stores made before the presets keep their table.
    columns: [
      ["audit_export", "preset", "TEXT"],
      ["audit_export", "preset_fields", "TEXT"],
      ["audit_export", "preset_secret", "TEXT"],
    ],
    ops: commands,
    mutations: ["audit-export-clear"],
  });
  const P = OrgClass.prototype;

  // Every event appended while an export is set is queued in the same
  // transaction: if the command rolls back, so does its place in the queue.
  const audit = P.audit;
  P.audit = function (...args) {
    const event = audit.apply(this, args);
    if (this.exportConfig()) {
      this.sql.exec("INSERT INTO audit_outbox (seq, queued_at, attempts, next_at) VALUES (?, ?, 0, ?)", event.seq, event.at, event.at);
    }
    return event;
  };

  // After each command, outside its transaction: the alarm is set for
  // whatever it queued.
  const fetch = P.fetch;
  P.fetch = async function (request) {
    const res = await fetch.call(this, request);
    try {
      await this.exportArm();
    } catch (err) {
      console.error("audit export: could not set the alarm", err && err.message);
    }
    return res;
  };

  // The Durable Object's alarm: deliver, then wake again for what is left.
  const alarm = P.alarm;
  P.alarm = async function (...args) {
    if (typeof alarm === "function") await alarm.apply(this, args);
    const done = await this.exportDeliver();
    await this.exportArm(true);
    return done;
  };
}

install(Org);

// ---------------------------------------------------------------- routes

/// Private and loopback addresses written as IP literals. The hub runs on
/// Cloudflare's network, not the company's, so these name nothing a company
/// could mean; refusing them keeps the export from being pointed at
/// anything but a real receiver.
function privateLiteral(host) {
  const h = host.replace(/^\[|\]$/g, "").toLowerCase();
  if (h === "localhost" || h.endsWith(".localhost")) return true;
  const v4 = /^(\d+)\.(\d+)\.(\d+)\.(\d+)$/.exec(h);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    return a === 10 || a === 127 || a === 0 || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127);
  }
  return h.includes(":") && (h === "::1" || h.startsWith("fc") || h.startsWith("fd") || h.startsWith("fe80") || h === "::");
}

/// The webhook URL, checked: https (or plain http on this machine when the
/// hub itself is a local proof), no credentials in it, not a private
/// address. Answers `{url, host}` or `{problem}`.
export function webhookUrl(raw, env) {
  const s = String(raw || "").trim();
  if (!s || s.length > URL_MAX) return { problem: "give the receiver's https URL, such as https://siem.example.com/krate" };
  if (!reachable(s, env)) {
    return { problem: "the webhook is an https URL, with no user name or password in it (put a receiver's key in its query or rely on the signature)" };
  }
  const u = new URL(s);
  if (!devHub(env) && privateLiteral(u.hostname)) return { problem: "that is a private or loopback address -- the hub sends from the internet, so give the receiver's public https URL" };
  return { url: u.toString(), host: u.host };
}

/// What a PUT asks for, checked: the preset, where its events go, the fields
/// kept in the clear and the token to seal. Answers `{preset, url, host,
/// fields, token}` (token null to keep the one stored) or `{problem}`.
export function presetConfig(v, env) {
  const preset = v.preset == null ? "generic" : String(v.preset);
  if (!SIEM_PRESETS.includes(preset)) return { problem: `the destination is one of ${SIEM_PRESETS.join(", ")}` };
  const f = v.fields && typeof v.fields === "object" && !Array.isArray(v.fields) ? v.fields : {};
  const raw = v.token == null ? "" : String(v.token).trim();
  const token = raw || null;
  const str = (x) => (x == null ? "" : String(x).trim());
  if (preset === "generic") {
    const c = webhookUrl(v.url, env);
    return c.problem ? c : { preset, url: c.url, host: c.host, fields: {}, token: null };
  }
  if (preset === "splunk") {
    const c = webhookUrl(v.url, env);
    if (c.problem) return { problem: "give your HTTP Event Collector's https address, such as https://http-inputs-acme.splunkcloud.com" };
    const u = new URL(c.url);
    const p = u.pathname.replace(/\/+$/, "");
    if (p === "" || p === "/services/collector") u.pathname = "/services/collector/event";
    else if (p !== "/services/collector/event") return { problem: "the HEC address ends in /services/collector/event, or give just its host" };
    const index = str(f.index);
    const sourcetype = str(f.sourcetype);
    if (index && !/^[a-z0-9_][a-z0-9_-]{0,79}$/.test(index)) return { problem: "a Splunk index name is lower-case letters, digits, _ and -" };
    if (sourcetype && !/^[A-Za-z0-9:_.-]{1,80}$/.test(sourcetype)) return { problem: "a sourcetype is letters, digits and : _ . -" };
    if (token && !/^[A-Za-z0-9-]{8,128}$/.test(token)) return { problem: "the HEC token is the long id Splunk shows for the collector, such as 1b2c3d4e-..." };
    return { preset, url: u.toString(), host: u.host, fields: { ...(index ? { index } : {}), ...(sourcetype ? { sourcetype } : {}) }, token };
  }
  if (preset === "sentinel") {
    let u;
    try {
      u = new URL(str(f.endpoint));
    } catch (_) {
      u = null;
    }
    if (!u || u.protocol !== "https:" || u.username || u.password || (u.port && u.port !== "443") || !INGEST_HOST.test(u.hostname.toLowerCase()) || (u.pathname !== "/" && u.pathname !== "")) {
      return { problem: "the logs ingestion endpoint is the https address Azure shows on the data collection endpoint or rule, ending in .ingest.monitor.azure.com" };
    }
    const tenant = str(f.tenant_id).toLowerCase();
    const client = str(f.client_id).toLowerCase();
    const dcr = str(f.dcr_id).toLowerCase();
    const stream = str(f.stream);
    if (!GUID.test(tenant) || !GUID.test(client)) return { problem: "the tenant id and client id are the app's two GUIDs from Microsoft Entra" };
    if (!/^dcr-[0-9a-f]{32}$/.test(dcr)) return { problem: "the rule's immutable id starts with dcr- and is on its JSON view in Azure" };
    if (!/^(Custom|Microsoft)-[A-Za-z0-9_]{1,100}$/.test(stream)) return { problem: "the stream is the rule's stream name, such as Custom-KrateAudit_CL" };
    if (token && (token.length > 200 || /\s/.test(token))) return { problem: "the client secret is the value Microsoft Entra showed once, with no spaces" };
    const endpoint = `https://${u.hostname.toLowerCase()}`;
    const url = `${endpoint}/dataCollectionRules/${dcr}/streams/${encodeURIComponent(stream)}?api-version=2023-01-01`;
    return { preset, url, host: u.hostname.toLowerCase(), fields: { endpoint, tenant_id: tenant, client_id: client, dcr_id: dcr, stream }, token };
  }
  // datadog
  const site = str(f.site).toLowerCase() || "datadoghq.com";
  if (!Object.prototype.hasOwnProperty.call(DATADOG_SITES, site)) return { problem: `the Datadog site is one of ${Object.keys(DATADOG_SITES).join(", ")}` };
  const service = str(f.service);
  if (service && !/^[a-z0-9_.-]{1,60}$/.test(service)) return { problem: "a Datadog service name is lower-case letters, digits and _ . -" };
  if (token && !/^[0-9a-f]{32}$/.test(token)) return { problem: "a Datadog API key is 32 letters and digits, from Organization Settings > API Keys" };
  const host = `http-intake.logs.${site}`;
  return { preset, url: `https://${host}/api/v2/logs`, host, fields: { site, ...(service ? { service } : {}) }, token };
}

/// /orgs/<slug>/audit-export[/secret|/test]. Answers a Response, or null.
export async function auditExportRoute(h) {
  const { rest, method, env, slug } = h;
  if (rest[0] !== "audit-export") return null;
  const off = () => h.json({ error: "audit-export-off", message: "audit export is not switched on on this hub (it has no ORG_SECRET_KEY)" }, 503);
  if (rest.length === 1 && method === "GET") return h.orgAnswer(await h.askRaw("audit-export-get"));
  if (rest.length === 1 && method === "DELETE") return h.orgAnswer(await h.mutateRaw("audit-export-clear", () => ({})));
  if (rest.length === 1 && method === "PUT") {
    const body = await h.readBody();
    if (body.bad || body.tooLarge) return h.json({ error: "bad-request", message: "the body is JSON, under 64 KiB" }, 400);
    const checked = presetConfig(body.value, env);
    if (checked.problem) return h.json({ error: "bad-request", message: checked.problem }, 400);
    // Who may, before a secret is made: a stranger costs no crypto.
    const current = await h.askRaw("audit-export-get");
    if (current.status !== 200) return h.orgAnswer(current);
    let secret;
    let sealed;
    if (!current.body) {
      secret = newWebhookSecret();
      sealed = await sealSecret(env, slug, secret, `krate-org-webhook:${slug}`);
      if (!sealed) return off();
    }
    let presetSecret;
    if (checked.token) {
      presetSecret = await sealSecret(env, slug, checked.token, `krate-org-siem:${slug}`);
      if (!presetSecret) return off();
    }
    const answer = await h.askRaw("audit-export-set", {
      url: checked.url,
      host: checked.host,
      secret: sealed,
      preset: checked.preset,
      fields: checked.fields,
      preset_secret: presetSecret,
    });
    if (answer.status >= 300 || !secret) return h.orgAnswer(answer);
    const res = h.json({ ...answer.body, secret }, answer.status);
    res.headers.set("cache-control", "no-store");
    return res;
  }
  if (rest.length === 2 && rest[1] === "secret" && method === "POST") {
    const current = await h.askRaw("audit-export-get");
    if (current.status !== 200) return h.orgAnswer(current);
    const secret = newWebhookSecret();
    const sealed = await sealSecret(env, slug, secret, `krate-org-webhook:${slug}`);
    if (!sealed) return off();
    const answer = await h.askRaw("audit-export-secret", { secret: sealed });
    if (answer.status !== 200) return h.orgAnswer(answer);
    const res = h.json({ ...answer.body, secret }, 200);
    res.headers.set("cache-control", "no-store");
    return res;
  }
  if (rest.length === 2 && rest[1] === "test" && method === "POST") return h.orgAnswer(await h.askRaw("audit-export-test"));
  return null;
}
