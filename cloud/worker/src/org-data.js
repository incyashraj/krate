// Team data for an organisation's own apps: store.org (IC-951), end-to-end
// encrypted (F-316).
//
// An internal app built on Krate has no backend: what it keeps for the team
// lives here, in the organisation's own store. The hub holds ciphertext
// only. Per field it stores what a device sent it:
//
// - `r` and `k`: opaque names of the record and the field, HMAC-SHA256
//   under the app's data key (a key the hub never has), so the same field
//   has the same name on every device and nothing here says what it is;
// - `kind`: a value, a removed field, or the record's tombstone -- so the
//   hub can bound records, drop fields a delete outlived and let tombstones
//   go after thirty days;
// - `s`: the hybrid-logical-clock stamp, in the clear, so the hub keeps the
//   latest write per field and pages by cursor;
// - `ver`: the organisation key version it is sealed under;
// - `ct`: XChaCha20-Poly1305 of {collection, id, field, value, who}, bound
//   to r, k, kind and ver. The hub cannot read or change it unnoticed.
//
// The data key is derived from the organisation key (org-keys.js), which
// exists only on member devices. What the hub still sees: which app, how
// many records and fields and their sizes, which fields share a record,
// when people sync and who, and that a record was deleted. Not what any of
// it says.
//
// Who may sync is the same question a download asks, on every sync:
// standing, a release of this organisation (not waiting, not withdrawn),
// inside its audience. The app id is the release's signed manifest's. Then
// the rows merge by the rule in org-data-merge.js, the one the device runs:
// per field the greater stamp wins, after the push's stamps are moved onto
// the hub's clock.
//
// The audit gets counts, never content: one line per app per day.

import { Org, extendOrg, addReasons, allows, displayOf } from "./org.js";
// The release checks (withdrawn, audience) are org-releases.js's, the key
// version org-keys.js's, the export and the sweep the lifecycle store's.
import { appOf } from "./org-releases.js";
import "./org-keys.js";
import "./org-lifecycle-store.js";
import { parseStamp, formatStamp, rebase, wins } from "./org-data-merge.js";
// Days are the organisation's own (org-time.js, F-319).
import { dayBefore } from "./org-time.js";

const DAY = 24 * 60 * 60;
/// Deleted records keep their tombstone this long (the contract's 30 days).
export const TOMBSTONE_DAYS = 30;
/// The most one push may carry, and the most rows or bytes one answer gives.
export const MAX_PUSH_BYTES = 1024 * 1024;
export const MAX_WRITES = 1000;
export const PAGE_ROWS = 500;
export const PAGE_BYTES = 512 * 1024;
/// A device keeps a record's plaintext to 64 KiB. Sealed, each field adds a
/// nonce, a tag and its names, so the hub's bound is 64 KiB plus 512 bytes
/// per field, of ciphertext.
export const MAX_RECORD_CT = 64 * 1024;
export const FIELD_CT_ALLOWANCE = 512;
/// The most one sealed field may be.
export const MAX_FIELD_CT = MAX_RECORD_CT + FIELD_CT_ALLOWANCE;
/// What a row costs against the quota beyond its ciphertext: names, stamp,
/// writer.
export const ROW_OVERHEAD = 160;

/// The per-app quota in one organisation (default 50 MiB of ciphertext). An
/// owner setting later; a hub setting now, so a test can make it small.
export const quotaOf = (env) => {
  const n = Number(env && env.ORG_DATA_QUOTA_BYTES);
  return Number.isFinite(n) && n > 0 ? n : 50 * 1024 * 1024;
};

addReasons({
  "not-released": {
    status: 403,
    message: (o, d) => d || `this app is not a release of ${o}, so it cannot use ${o}'s data -- open the version ${o} released`,
  },
  "too-large": {
    status: 413,
    message: (o, d) => d || "a record is at most 64 KiB",
  },
  "over-quota": {
    status: 413,
    message: (o, d) => d || `this app's data in ${o} is full -- remove some records, or ask an owner of ${o}`,
  },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS org_data (
     app_id TEXT NOT NULL, rec TEXT NOT NULL, fkey TEXT NOT NULL, kind TEXT NOT NULL,
     ct TEXT NOT NULL, ver INTEGER NOT NULL, stamp TEXT NOT NULL, by TEXT NOT NULL,
     size INTEGER NOT NULL, seq INTEGER NOT NULL,
     PRIMARY KEY (app_id, rec, fkey))`,
  `CREATE INDEX IF NOT EXISTS org_data_by_seq ON org_data (app_id, seq)`,
  `CREATE TABLE IF NOT EXISTS org_data_apps (
     app_id TEXT PRIMARY KEY, seq INTEGER NOT NULL,
     purged_seq INTEGER NOT NULL, used INTEGER NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS org_data_daily (
     day TEXT NOT NULL, app_id TEXT NOT NULL, writes INTEGER NOT NULL,
     PRIMARY KEY (day, app_id))`,
  `CREATE TABLE IF NOT EXISTS org_data_readers (
     day TEXT NOT NULL, app_id TEXT NOT NULL, principal TEXT NOT NULL,
     PRIMARY KEY (day, app_id, principal))`,
  // Each finished day's line, kept 90 days for the insights (IC-959):
  // the same two counts the audit line says, nothing else.
  `CREATE TABLE IF NOT EXISTS org_data_days (
     day TEXT NOT NULL, app_id TEXT NOT NULL, writes INTEGER NOT NULL, readers INTEGER NOT NULL,
     PRIMARY KEY (day, app_id))`,
];

/// How long a finished day's counts are kept.
export const DATA_DAYS_KEPT = 90;

const HEX64 = /^[0-9a-f]{64}$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;
const KINDS = new Set(["v", "n", "t"]);
const bad = (message) => ({ status: 400, body: { error: "bad-request", message } });

/// The bytes a base64 string stands for.
export function ctBytes(b64) {
  const pad = b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0;
  return (b64.length / 4) * 3 - pad;
}

function rowView(r) {
  return { r: r.rec, k: r.fkey, kind: r.kind, s: r.stamp, ver: r.ver, ct: r.ct, by: displayOf(r.by) };
}

/// The writes a push carries, checked, or the words for what is wrong.
function readWrites(raw) {
  if (raw == null) return { writes: [] };
  if (!Array.isArray(raw)) return { problem: "writes is a list" };
  if (raw.length > MAX_WRITES) return { problem: `one sync carries at most ${MAX_WRITES} writes` };
  const writes = [];
  for (const w of raw) {
    if (!w || typeof w !== "object") return { problem: "each write is an object" };
    if (!HEX64.test(String(w.r)) || !HEX64.test(String(w.k))) return { problem: "a write names its record and field by 64-hex opaque keys" };
    if (!KINDS.has(w.kind)) return { problem: "a write's kind is v, n or t" };
    if (!parseStamp(w.s)) return { problem: "a write's stamp is not one" };
    if (!Number.isSafeInteger(w.ver) || w.ver < 1) return { problem: "a write names the key version it is sealed under" };
    if (typeof w.ct !== "string" || w.ct.length % 4 !== 0 || !B64.test(w.ct)) return { problem: "a write's ciphertext is base64" };
    const n = ctBytes(w.ct);
    if (n < 40) return { problem: "a write's ciphertext is too short to be sealed" };
    if (n > MAX_FIELD_CT) return { problem: `one sealed field is at most ${MAX_FIELD_CT} bytes` };
    writes.push({ r: w.r, k: w.k, kind: w.kind, s: w.s, ver: w.ver, ct: w.ct, n });
  }
  return { writes };
}

const ops = {
  nowMs() {
    return Date.now();
  },

  dataApp(app) {
    let r = this.row("SELECT * FROM org_data_apps WHERE app_id = ?", app);
    if (!r) {
      this.sql.exec("INSERT INTO org_data_apps (app_id, seq, purged_seq, used) VALUES (?, 0, 0, 0)", app);
      r = { app_id: app, seq: 0, purged_seq: 0, used: 0 };
    }
    return r;
  },

  /// The member and the app a sync is for, or the refusal: standing,
  /// then a release of this organisation, not withdrawn, then audience.
  dataStanding(ctx, action) {
    const digest = String(ctx.args.digest || "");
    const target = `release:${digest}`;
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return { refusal };
    if (allows(me.role, "data.use") !== "yes") return { refusal: this.deny(ctx, action, target, "role-cannot") };
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "withdrawn") return { refusal: this.withdrawnRefusal(ctx, c, action, target) };
    if (!c || c.state !== "released") {
      return {
        refusal: this.deny(
          ctx,
          action,
          target,
          "not-released",
          c ? `this version of ${c.name || "the app"} is waiting for approval in ${this.org().name || ctx.slug}, and only a released version uses its data` : undefined,
        ),
      };
    }
    const outside = this.audienceRefusal(ctx, me, c, action, target);
    if (outside) return { refusal: outside };
    return { me, app: appOf(c) };
  },

  /// POST /orgs/<slug>/data/<digest>/sync: push this device's sealed
  /// writes, pull what changed since its cursor. One transaction.
  op_data_sync(ctx) {
    const { me, app, refusal } = this.dataStanding(ctx, "data.sync");
    if (refusal) return refusal;
    const device = String(ctx.args.device || "");
    if (!/^[0-9a-f]{16}$/.test(device)) return bad("device is 16 lower-case hex digits");
    const deviceNow = ctx.args.now;
    if (!Number.isSafeInteger(deviceNow) || deviceNow < 0) return bad("now is the device's clock, in milliseconds");
    const sinceRaw = ctx.args.since;
    if (sinceRaw != null && !/^\d{1,15}$/.test(String(sinceRaw))) return bad("since is a cursor this hub gave");
    const read = readWrites(ctx.args.writes);
    if (read.problem) return bad(read.problem);
    const keyVersion = this.currentKeyVersion();
    if (read.writes.length) {
      if (keyVersion == null) return this.deny(ctx, "data.sync", `app:${app}`, "no-org-key");
      // New writes go under the current key: a device still on an older
      // version (one a removed member also holds) fetches the new one first.
      const old = read.writes.find((w) => w.ver !== keyVersion);
      if (old) {
        return this.deny(
          ctx,
          "data.sync",
          `app:${app}`,
          "key-rotated",
          old.ver < keyVersion ? `write under version ${keyVersion} of the key, not ${old.ver}` : `there is no version ${old.ver} of the key`,
        );
      }
    }
    const hubNow = this.nowMs();
    const quota = quotaOf(this.env);
    const state = this.dataApp(app);

    // Every record a write touches, as it is now.
    const records = new Map();
    for (const w of read.writes) {
      if (records.has(w.r)) continue;
      const cells = {};
      for (const r of this.rows("SELECT fkey, kind, stamp, size FROM org_data WHERE app_id = ? AND rec = ?", app, w.r)) {
        cells[r.fkey] = { kind: r.kind, s: r.stamp, size: r.size, held: true };
      }
      records.set(w.r, { cells, changed: new Map(), gone: new Set() });
    }
    const tombOf = (rec) => {
      let t = null;
      for (const cell of Object.values(rec.cells)) if (cell.kind === "t" && (!t || cell.s > t)) t = cell.s;
      return t;
    };

    // Apply in memory first, so a refusal leaves everything as it was.
    let accepted = 0;
    let delta = 0;
    const lost = new Map();
    for (const w of read.writes) {
      const rec = records.get(w.r);
      const s = rebase(w.s, deviceNow, hubNow);
      const tomb = tombOf(rec);
      const held = rec.cells[w.k];
      const deadOnArrival = w.kind !== "t" && tomb && !(s > tomb);
      if (deadOnArrival || !wins(s, held ? held.s : null)) {
        // It lost: the device is sent what won, so both hold the same.
        lost.set(`${w.r}/${w.k}`, { r: w.r, k: deadOnArrival ? null : w.k });
        continue;
      }
      const size = w.n + ROW_OVERHEAD;
      delta += size - (held ? held.size : 0);
      rec.cells[w.k] = { kind: w.kind, s, size, w };
      rec.changed.set(w.k, { ...w, s, size });
      rec.gone.delete(w.k);
      accepted += 1;
      if (w.kind === "t") {
        // Fields older than the tombstone are dead: they go now.
        for (const [k, cell] of Object.entries(rec.cells)) {
          if (cell.kind !== "t" && !(cell.s > s)) {
            delta -= cell.size;
            delete rec.cells[k];
            rec.changed.delete(k);
            if (cell.held) rec.gone.add(k);
          }
        }
      }
    }
    for (const [r, rec] of records) {
      if (!rec.changed.size) continue;
      const live = Object.values(rec.cells).filter((c) => c.kind === "v");
      const bytes = live.reduce((n, c) => n + c.size - ROW_OVERHEAD, 0);
      const bound = MAX_RECORD_CT + FIELD_CT_ALLOWANCE * live.length;
      if (bytes > bound) {
        return this.deny(
          ctx,
          "data.sync",
          `app:${app}`,
          "too-large",
          `one record would be ${bytes} bytes sealed, and a record is at most 64 KiB (${bound} sealed) -- remove some of its fields (record ${r.slice(0, 12)})`,
        );
      }
    }
    if (delta > 0 && state.used + delta > quota) {
      return this.deny(
        ctx,
        "data.sync",
        `app:${app}`,
        "over-quota",
        `this app's data in ${this.org().name || ctx.slug} would be ${state.used + delta} bytes, and it may hold ${quota} -- remove some records, or ask an owner`,
      );
    }

    // Commit.
    let seq = state.seq;
    for (const [r, rec] of records) {
      for (const k of rec.gone) this.sql.exec("DELETE FROM org_data WHERE app_id = ? AND rec = ? AND fkey = ?", app, r, k);
      for (const [k, w] of rec.changed) {
        seq += 1;
        this.sql.exec(
          `INSERT INTO org_data (app_id, rec, fkey, kind, ct, ver, stamp, by, size, seq) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT (app_id, rec, fkey) DO UPDATE SET
             kind = excluded.kind, ct = excluded.ct, ver = excluded.ver, stamp = excluded.stamp,
             by = excluded.by, size = excluded.size, seq = excluded.seq`,
          app, r, k, w.kind, w.ct, w.ver, w.s, me.principal, w.size, seq,
        );
      }
    }
    let used = state.used + delta;
    let purgedSeq = state.purged_seq;

    // Tombstones and removed fields past their thirty days go; a device
    // whose cursor is older than that starts over from the hub's copy.
    const horizon = formatStamp({ wall: Math.max(0, hubNow - TOMBSTONE_DAYS * DAY * 1000), counter: 0, device: "0".repeat(16) });
    const expired = this.row(
      "SELECT COUNT(*) AS n, COALESCE(SUM(size), 0) AS bytes, COALESCE(MAX(seq), 0) AS top FROM org_data WHERE app_id = ? AND kind IN ('n', 't') AND stamp < ?",
      app,
      horizon,
    );
    if (expired.n) {
      this.sql.exec("DELETE FROM org_data WHERE app_id = ? AND kind IN ('n', 't') AND stamp < ?", app, horizon);
      used -= expired.bytes;
      purgedSeq = Math.max(purgedSeq, expired.top);
    }
    this.sql.exec("UPDATE org_data_apps SET seq = ?, used = ?, purged_seq = ? WHERE app_id = ?", seq, Math.max(0, used), purgedSeq, app);
    // Something new for this app: the live sockets hear the cursor, after
    // the commit (org-data-live.js). Never what changed.
    if (seq > state.seq) this.dataChanged = { app, cursor: seq, device };

    // Counts for the day's audit line: writes, and who synced.
    const day = this.orgDay(ctx.now);
    this.sql.exec(
      `INSERT INTO org_data_daily (day, app_id, writes) VALUES (?, ?, ?)
       ON CONFLICT (day, app_id) DO UPDATE SET writes = writes + excluded.writes`,
      day,
      app,
      accepted,
    );
    this.sql.exec("INSERT OR IGNORE INTO org_data_readers (day, app_id, principal) VALUES (?, ?, ?)", day, app, me.principal);

    // What changed since the device's cursor, oldest first, bounded.
    const since = sinceRaw == null ? 0 : Number(sinceRaw);
    const full = sinceRaw != null && since < purgedSeq;
    const from = full ? 0 : since;
    const rows = [];
    let bytes = 0;
    let cursor = from;
    let more = false;
    for (const r of this.rows("SELECT * FROM org_data WHERE app_id = ? AND seq > ? ORDER BY seq LIMIT ?", app, from, PAGE_ROWS + 1)) {
      if (rows.length >= PAGE_ROWS || (rows.length && bytes + r.size > PAGE_BYTES)) {
        more = true;
        break;
      }
      rows.push(rowView(r));
      bytes += r.size;
      cursor = r.seq;
    }
    // For each write that lost, what won it, so the device holds the same.
    for (const l of lost.values()) {
      const won = l.k
        ? this.rows("SELECT * FROM org_data WHERE app_id = ? AND rec = ? AND fkey = ?", app, l.r, l.k)
        : this.rows("SELECT * FROM org_data WHERE app_id = ? AND rec = ? AND kind = 't'", app, l.r);
      for (const r of won) rows.push(rowView(r));
    }
    return this.ok({
      v: 1,
      now: hubNow,
      accepted,
      key_version: keyVersion,
      rows,
      cursor: String(cursor),
      more,
      full,
      me: { name: displayOf(me.principal), role: me.role },
      org: { slug: this.org().slug, name: this.org().name },
      quota: { used: Math.max(0, used), limit: quota },
      // A one-use ticket for this app's live socket, when the device asks
      // (org-data-live.js): the same checks as this sync just passed.
      ...(ctx.args.live === true && typeof this.liveTicket === "function" ? { live: this.liveTicket(me, app, device, ctx) } : {}),
    });
  },

  /// GET /orgs/<slug>/data (owner, admin): each app's fields, bytes and
  /// quota, and today's counts. Counts, never content.
  op_data_usage(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "data.usage", target);
    if (refusal) return refusal;
    if (allows(me.role, "data.usage") !== "yes") {
      return this.deny(ctx, "data.usage", target, "role-cannot", `a ${me.role} cannot see how ${ctx.slug}'s apps use its data`);
    }
    const day = this.orgDay(ctx.now);
    const quota = quotaOf(this.env);
    const apps = this.rows("SELECT * FROM org_data_apps ORDER BY app_id").map((a) => {
      const records = this.row("SELECT COUNT(DISTINCT rec) AS n FROM org_data WHERE app_id = ? AND kind = 'v'", a.app_id).n;
      const today = this.row("SELECT writes FROM org_data_daily WHERE day = ? AND app_id = ?", day, a.app_id);
      const readers = this.row("SELECT COUNT(*) AS n FROM org_data_readers WHERE day = ? AND app_id = ?", day, a.app_id).n;
      return { app_id: a.app_id, records, bytes: a.used, quota, today: { writes: today ? today.writes : 0, readers } };
    });
    return this.ok({ apps });
  },
};

/// The day's audit line for every app that synced on an earlier day:
/// counts, never content. Runs before any command.
function tick(ctx) {
  const today = this.orgDay(ctx.now);
  for (const d of this.rows("SELECT * FROM org_data_daily WHERE day < ? ORDER BY day, app_id", today)) {
    const readers = this.row("SELECT COUNT(*) AS n FROM org_data_readers WHERE day = ? AND app_id = ?", d.day, d.app_id).n;
    this.audit("system:krate", "data.daily", `app:${d.app_id}`, "allowed", `${d.day}: ${d.writes} writes, ${readers} readers`);
    this.sql.exec(
      "INSERT OR REPLACE INTO org_data_days (day, app_id, writes, readers) VALUES (?, ?, ?, ?)",
      d.day,
      d.app_id,
      d.writes,
      readers,
    );
    this.sql.exec("DELETE FROM org_data_days WHERE day < ?", dayBefore(this.orgDay(ctx.now), DATA_DAYS_KEPT));
    this.sql.exec("DELETE FROM org_data_daily WHERE day = ? AND app_id = ?", d.day, d.app_id);
    this.sql.exec("DELETE FROM org_data_readers WHERE day = ? AND app_id = ?", d.day, d.app_id);
  }
}

/// What the standard export carries of the apps' data: every row as it is
/// stored, sealed. Reading it needs a member device or the recovery code.
function exportRows(store) {
  return store.rows("SELECT * FROM org_data ORDER BY app_id, rec, fkey").map((r) => ({
    app_id: r.app_id,
    record: r.rec,
    field: r.fkey,
    kind: r.kind,
    key_version: r.ver,
    stamp: r.stamp,
    ciphertext: r.ct,
    by: r.by,
  }));
}

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({ schema: SCHEMA, tick });
  const P = OrgClass.prototype;
  // Loud, not skipped: without these, a withdrawn or out-of-audience
  // release would reach the data, writes would go under any key, or the
  // export would leave the data out.
  for (const name of ["withdrawnRefusal", "audienceRefusal", "currentKeyVersion", "op_export_begin", "op_swept"]) {
    if (typeof P[name] !== "function") throw new Error(`org-data: ${name} is not installed`);
  }

  const begin = P.op_export_begin;
  P.op_export_begin = function (ctx) {
    const answer = begin.call(this, ctx);
    if (answer.status === 202 && answer.body && answer.body.data) answer.body.data.app_data = exportRows(this);
    return answer;
  };

  // A closed organisation's sweep deletes its apps' data with its files.
  const swept = P.op_swept;
  P.op_swept = function (ctx) {
    const answer = swept.call(this, ctx);
    if (answer.status === 200) {
      const n = this.row("SELECT COUNT(*) AS n FROM org_data").n;
      for (const t of ["org_data", "org_data_apps", "org_data_daily", "org_data_readers", "org_data_days"]) this.sql.exec(`DELETE FROM ${t}`);
      this.audit("system:krate", "data.deleted", `org:${ctx.slug}`, "allowed", `${n} rows`);
    }
    return answer;
  };
}

// Importing this module teaches the store team data.
install(Org);

// ---------------------------------------------------------------- routes

const DIGEST = /^[0-9a-f]{64}$/;

/// POST /orgs/<slug>/data/<digest>/sync and GET /orgs/<slug>/data.
/// Answers a Response, or null.
export async function orgDataRoute(h) {
  const { request, rest, method, askRaw, orgAnswer, readBounded, json } = h;
  if (rest[0] !== "data") return null;
  if (rest.length === 1 && method === "GET") return orgAnswer(await askRaw("data-usage"));
  if (rest.length === 3 && rest[2] === "sync" && method === "POST") {
    const digest = String(rest[1]).toLowerCase();
    if (!DIGEST.test(digest)) {
      return orgAnswer({ status: 403, body: { error: "not-released", message: "a digest is the release's 64-hex sha256" } });
    }
    const declared = request.headers.get("content-length");
    if (declared !== null && Number(declared) > MAX_PUSH_BYTES) {
      return json({ error: "too-large", message: `one sync carries at most ${MAX_PUSH_BYTES} bytes -- send the rest in the next` }, 413);
    }
    const read = await readBounded(request, MAX_PUSH_BYTES);
    if (read.tooLarge) return json({ error: "too-large", message: `one sync carries at most ${MAX_PUSH_BYTES} bytes -- send the rest in the next` }, 413);
    let body;
    try {
      const text = new TextDecoder().decode(read.body);
      body = text ? JSON.parse(text) : {};
    } catch (_) {
      return json({ error: "bad-request", message: "the body is JSON" }, 400);
    }
    if (!body || typeof body !== "object") return json({ error: "bad-request", message: "the body is a JSON object" }, 400);
    return orgAnswer(
      await askRaw("data-sync", { digest, device: body.device, now: body.now, since: body.since, writes: body.writes, live: body.live === true }),
    );
  }
  return null;
}
