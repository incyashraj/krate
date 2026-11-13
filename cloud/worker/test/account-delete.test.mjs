/*
 * Deleting an account, and its export (K-1245, src/account-delete.js).
 *
 * Through the hub's real routes over this machine's stand-ins: KV and R2
 * as maps, the REAL Org and SyncAccount objects over SQLite. Two accounts
 * live side by side with everything a person can leave on the hub --
 * sign-ins, published apps with versions, a screenshot, Studio
 * conversations, a support ticket and report, requests, an API key, a
 * paid plan, account sync with a file, organisations, referrals, reports
 * on someone else's app -- and every key, object and row is scanned before
 * and after. Deleting one removes every key that names it and leaves every
 * key of the other byte for byte; the last owner of an organisation is
 * refused; a run that dies part-way is finished by running again; the
 * export carries no token or secret. GitHub and Stripe are stubs: nothing
 * here reaches either.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/account-delete.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, worker } from "./org-tools.mjs";
import { syncNamespace } from "./sync-account-tools.mjs";
import { sha256Hex } from "./r2-mock.mjs";
import { CONFIRM_PHRASE, STEPS, resumeAccountDeletions } from "../src/account-delete.js";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = (...p) => new Uint8Array(readFileSync(join(here, ...p)));
const NOTES1 = fixture("fixtures", "notes-0.1.0.krate");
const NOTES2 = fixture("fixtures", "notes-0.2.0.krate");
const BOUNCE = fixture("..", "..", "..", "evidence", "ported", "bounce.krate");
const SIGNED = fixture("fixtures", "bounce-signed.krate");
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13]);
const ZIP = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 1, 2, 3, 4, 5, 6, 7, 8]);

/* GitHub and Stripe. A GitHub web sign-in exchanges its code for the next
 * token in `issue`; the OAuth app's grant revocation ends every token of
 * that GitHub user, and /user refuses them from then on (the rest of /user
 * is test/github-stub.mjs). Stripe's subscription cancel is recorded. */
const outside = { issue: [], revokedGrants: [], cancelled: [] };
const dead = new Set();
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  const headers = new Headers((input && input.headers) || init.headers || {});
  if (url === "https://github.com/login/oauth/access_token") {
    return new Response(JSON.stringify({ access_token: outside.issue.shift() }), { headers: { "content-type": "application/json" } });
  }
  if (url === "https://api.github.com/user") {
    const token = (headers.get("authorization") || "").replace(/^Bearer\s+/i, "");
    if (dead.has(token)) return new Response("Bad credentials", { status: 401 });
  }
  if (/^https:\/\/api\.github\.com\/applications\/[^/]+\/grant$/.test(url) && init.method === "DELETE") {
    const { access_token } = JSON.parse(init.body);
    outside.revokedGrants.push(access_token);
    dead.add(access_token);
    return new Response(null, { status: 204 });
  }
  const sub = url.match(/^https:\/\/api\.stripe\.com\/v1\/subscriptions\/([^/?]+)$/);
  if (sub && init.method === "DELETE") {
    outside.cancelled.push(sub[1]);
    return new Response(JSON.stringify({ id: sub[1], status: "canceled" }), { headers: { "content-type": "application/json" } });
  }
  return before(input, init);
};

function hub(extra = {}) {
  const env = orgEnv({ GITHUB_CLIENT_SECRET: "test-only", STRIPE_SECRET_KEY: "sk_test_stand_in", ...extra });
  env.SYNC_ACCOUNTS = syncNamespace(env);
  return env;
}

async function hit(env, method, path, { token, json: payload, bytes, headers = {} } = {}) {
  const h = { ...headers };
  if (token) h.authorization = `Bearer ${token}`;
  let body;
  if (payload !== undefined) {
    h["content-type"] = "application/json";
    body = JSON.stringify(payload);
  } else if (bytes) body = bytes;
  const res = await worker.fetch(new Request(`https://hub.example${path}`, { method, headers: h, body }), env);
  const text = await res.text();
  let parsed = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = text;
  }
  return { status: res.status, body: parsed, headers: res.headers, text };
}
const ok = (r, what) => {
  assert.ok(r.status >= 200 && r.status < 300, `${what}: ${r.status} ${typeof r.body === "string" ? r.body : JSON.stringify(r.body)}`);
  return r;
};

/* The real email-link sign-in: a fresh Krate session. */
let links = 0;
async function emailSignIn(env, email) {
  const t = `link${++links}`;
  env._kv.set(`email:${t}`, JSON.stringify({ email, from: "web", nonce: "", app_nonce: "" }));
  const res = await hit(env, "GET", `/login/email/verify?t=${t}`);
  assert.equal(res.status, 302, JSON.stringify(res.body));
  return new URLSearchParams(res.headers.get("location").split("#")[1]).get("token");
}

/* The real GitHub web sign-in: GitHub's own token, indexed. */
let states = 0;
async function githubSignIn(env, login) {
  const state = `state-${++states}`;
  env._kv.set(`login:${state}`, JSON.stringify({ from: "web", nonce: "", app_nonce: "" }));
  outside.issue.push(`gho_${login}`);
  dead.delete(`gho_${login}`);
  const res = await hit(env, "GET", `/login/callback?state=${state}&code=c${states}`);
  assert.equal(res.status, 302, JSON.stringify(res.body));
  return `gho_${login}`;
}

const idOf = (env, token) => env._kv.get(`session:${token}`);

async function deviceOf(seed) {
  const public_key = seed.repeat(64).slice(0, 64);
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(public_key));
  return { public_key, id: [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("").slice(0, 16) };
}
const b64 = (s) => Buffer.from(s).toString("base64");

/* One person's whole presence on the hub. Answers what names them. */
async function livedIn(env, token, { email, seed, bundles, org }) {
  const id = idOf(env, token);
  const hashes = [];
  for (const [i, bytes] of bundles.entries()) {
    const r = ok(await hit(env, "POST", "/publish", { token, bytes, headers: { "x-krate-name": `App of ${seed}` } }), `publish ${i}`);
    hashes.push(r.body.id);
  }
  ok(await hit(env, "POST", `/shot/${hashes[hashes.length - 1]}`, { token, bytes: PNG, headers: { "content-type": "image/png" } }), "shot");
  ok(await hit(env, "POST", "/sessions", { token, json: { id: `conv-${seed}`, updated: 5, title: `A ${seed} app`, transcript: [`make me a ${seed} app`] } }), "conversation");
  const ticket = ok(await hit(env, "POST", "/support/new", { token, json: { subject: `help ${seed}`, text: `it broke for ${seed}` } }), "ticket").body;
  const report = ok(await hit(env, "POST", "/report", { token, bytes: new Uint8Array([...ZIP, seed.charCodeAt(0)]), headers: { "x-krate-note": `report from ${seed}` } }), "report").body;
  ok(await hit(env, "POST", "/makeit", { json: { email, request: `a ${seed} tracker` } }), "makeit");
  // The founding-200 list is closed (F-322); a person who joined it before
  // still has the record, and deleting must still take it.
  env._kv.set(`founding:${email}`, JSON.stringify({ at: 1 }));
  const me = ok(await hit(env, "GET", "/me", { token }), "me").body;
  // Account sync: a device, the key, one entry and one file.
  const dev = await deviceOf(Buffer.from(seed).toString("hex"));
  ok(await hit(env, "PUT", `/account/sync/devices/${dev.id}`, { token, json: { public_key: dev.public_key, name: `${seed}'s Mac` } }), "device");
  ok(await hit(env, "POST", "/account/sync/init", {
    token,
    json: {
      device_id: dev.id, version: 1, public_key: "ee".repeat(32), envelope: b64(`sealed key ${seed}`),
      recovery: { public_key: "ff".repeat(32), params: { alg: "argon2id", salt: "00".repeat(16), m_kib: 19 * 1024, t: 2, p: 1 }, blob: b64(`recovery ${seed}`) },
    },
  }), "sync init");
  const bucket = (await sha256Hex(new TextEncoder().encode(`bucket ${seed}`)));
  const file = new TextEncoder().encode(`sealed file body of ${seed}`);
  const fileId = await sha256Hex(file);
  ok(await hit(env, "PUT", `/account/sync/blobs/${bucket}/${fileId}`, { token, bytes: file, headers: { "content-type": "application/octet-stream" } }), "sync file");
  ok(await hit(env, "PUT", `/account/sync/buckets/${bucket}`, {
    token,
    json: { writes: [
      { name: "a1".repeat(32), vv: { [dev.id]: 1 }, by: dev.id, kind: "v", k: 1, c: b64(`CIPHERTEXT-${seed}`) },
      { name: "b2".repeat(32), vv: { [dev.id]: 2 }, by: dev.id, kind: "v", k: 1, c: b64(`FILE-ENTRY-${seed}`), blob: fileId },
    ] },
  }), "sync bucket");
  // What a person cannot make through a route here: an API key's sealed
  // body, a build history, a paid plan and its payment, the new-account
  // welcome waiting, an announcement's mark, the make-counter.
  env._kv.set(`keys:${id}`, JSON.stringify({ anthropic: { iv: "aXY=", body: `SEALED-KEY-${seed}`, tail: "wxyz", added: 1 } }));
  env._kv.set(`spend:${id}`, JSON.stringify([{ at: 1, usd: 0.5, paid_by: "own" }]));
  env._kv.set(`ent:${id}`, JSON.stringify({ plan: "monthly", status: "active", until: Date.now() + 1e9, sub: `sub_${seed}`, customer: `cus_${seed}`, at: 1 }));
  env._kv.set(`cust:cus_${seed}`, id);
  env._kv.set(`pay:1700000000000:${seed}`, JSON.stringify({ userId: id, email, amount: 1200, currency: "usd", invoice: `in_${seed}`, at: 1 }));
  env._kv.set(`welcome-due:${id}`, "1");
  env._kv.set(`announced:news-1:${seed}digest`, id);
  env._kv.set(`mkacct:${id}`, "1");
  if (org) {
    const inv = ok(await hit(env, "POST", `/orgs/${org}/invites`, { token: "gho_alice", json: email.includes("@") ? { email, role: "developer" } : { github: email, role: "developer" } }), "invite");
    ok(await hit(env, "POST", `/orgs/${org}/invites/${inv.body.code}/accept`, { token }), "accept");
  }
  return { id, token, hashes, ticket, report: report.id, refcode: me.referral.code, bucket, fileId, email, seed };
}

/* Everything the hub keeps, as plain values to compare. */
function snapshot(env) {
  const kv = new Map(env._kv);
  const r2 = new Map([...env.BUNDLES._blobs].filter(([k]) => !k.includes("\u0000")).map(([k, v]) => [k, Buffer.from(v).toString("base64")]));
  const sync = env.SYNC_ACCOUNTS.dump();
  const orgs = {};
  for (const [name, o] of env.ORGS.objects) orgs[name] = o.sql.exec("SELECT principal, role, state FROM members ORDER BY principal").toArray();
  return { kv, r2, sync, orgs };
}

/* Two people and a GitHub publisher, with organisations around them. */
async function world(env) {
  ok(await hit(env, "POST", "/orgs", { token: "gho_alice", json: { slug: "acme", name: "Acme" } }), "acme");
  const anaToken = await emailSignIn(env, "ana@corp.example");
  const zedToken = await emailSignIn(env, "zed@other.example");
  const zed = await livedIn(env, zedToken, { email: "zed@other.example", seed: "zed", bundles: [BOUNCE], org: "acme" });
  const ana = await livedIn(env, anaToken, { email: "ana@corp.example", seed: "ana", bundles: [NOTES1, NOTES2], org: "acme" });
  // Ana joined through zed's invite link, and reported zed's app.
  ok(await hit(env, "POST", "/referral/claim", { token: anaToken, json: { code: zed.refcode } }), "referral");
  ok(await hit(env, "POST", `/report/app/${zed.hashes[0]}`, { token: anaToken, json: { why: "other", note: "looks broken" } }), "app report");
  // Ana owns duo with gina, a second owner: she can leave it.
  const ginaToken = await githubSignIn(env, "gina");
  ok(await hit(env, "POST", "/orgs", { token: anaToken, json: { slug: "duo", name: "Duo" } }), "duo");
  const inv = ok(await hit(env, "POST", "/orgs/duo/invites", { token: anaToken, json: { github: "gina", role: "owner" } }), "invite gina");
  ok(await hit(env, "POST", `/orgs/duo/invites/${inv.body.code}/accept`, { token: ginaToken }), "gina joins");
  return { ana, zed, ginaToken };
}

const deleteAs = (env, token, body = { confirm: CONFIRM_PHRASE, apps: "remove" }) => hit(env, "POST", "/me/delete", { token, json: body });

/* The proof that matters: after deleting `gone`, nothing on the hub names
 * it, and everything that did not name it is exactly as it was. */
function assertOnlyTheirsWent(env, was, gone, { changedOnPurpose = [] } = {}) {
  const now = snapshot(env);
  const markers = [gone.id, gone.email, gone.token, ...gone.hashes, `SEALED-KEY-${gone.seed}`, gone.bucket, gone.fileId, `conv-${gone.seed}`, gone.report];
  const names = (s) => markers.some((m) => m && String(s).includes(m));
  // KV: nothing left names them but the deletion's own marker, which
  // holds no name, address or token ...
  for (const [k, v] of now.kv) {
    if (k === `acctdel:${gone.id}`) {
      const tomb = JSON.parse(v);
      assert.equal(tomb.state, "done");
      assert.ok(!v.includes(gone.email) && !v.includes(gone.token) && !v.includes("resume_key"), `the marker holds more than it should: ${v}`);
      continue;
    }
    assert.ok(!names(k) && !names(v), `still on the hub after deletion: ${k} = ${String(v).slice(0, 160)}`);
  }
  // ... and every key that did not name them is untouched.
  const onPurpose = (k) => changedOnPurpose.some((p) => (p instanceof RegExp ? p.test(k) : p === k));
  for (const [k, v] of was.kv) {
    if (names(k) || names(v)) continue;
    if (onPurpose(k)) continue;
    assert.equal(now.kv.get(k), v, `another key changed or went: ${k}`);
  }
  // Nothing new appeared but the deletion's own record.
  for (const k of now.kv.keys()) assert.ok(was.kv.has(k) || k === `acctdel:${gone.id}` || onPurpose(k), `a key appeared: ${k}`);
  // R2: their bytes, screenshot, report and synced files gone; the rest equal.
  for (const [k, v] of was.r2) {
    if (names(k)) assert.ok(!now.r2.has(k), `R2 still holds ${k}`);
    else if (!k.startsWith("acsync/") || !k.includes(gone.id)) assert.equal(now.r2.get(k), v, `R2 changed: ${k}`);
  }
  assert.ok(![...now.r2.keys()].some((k) => k.startsWith(`acsync/${gone.id}/`) || k === `report:${gone.report}`));
  // Their sync object is empty; everyone else's is row for row the same.
  for (const [name, tables] of Object.entries(now.sync)) {
    if (name === `acct:${gone.id}`) {
      for (const [t, rows] of Object.entries(tables)) assert.equal(rows.length, 0, `their sync object still holds ${t}`);
    } else assert.deepEqual(tables, was.sync[name], `another account's sync changed: ${name}`);
  }
  return now;
}

test("deleting an account removes every key that names it, and nothing of anyone else's", async () => {
  const env = hub();
  const { ana, zed } = await world(env);
  // The world really holds ana everywhere this test looks.
  const was = snapshot(env);
  const anaKeys = [...was.kv].filter(([k, v]) => k.includes(ana.id) || String(v).includes(ana.id) || String(v).includes(ana.email)).map(([k]) => k.split(":")[0]);
  for (const family of ["user", "session", "usess", "ident", "sess", "tick", "utick", "report", "makeit", "keys", "spend", "ent", "cust", "pay", "welcome-due", "announced", "mkacct", "rcode", "refby", "refs", "app", "owner", "line"]) {
    assert.ok(anaKeys.includes(family), `the fixture has no ${family}: key for ana -- it would prove nothing`);
  }
  assert.ok(was.kv.has(`founding:${ana.email}`) && [...was.r2.keys()].some((k) => k.startsWith(`acsync/${ana.id}/`)));
  assert.ok(Object.values(was.sync[`acct:${ana.id}`]).some((rows) => rows.length), "ana's sync object holds rows");

  // The confirm step: what goes, what stays, her apps and organisations.
  const view = ok(await hit(env, "GET", "/me/delete", { token: ana.token }), "preview").body;
  assert.equal(view.fresh, true);
  assert.equal(view.confirm_phrase, CONFIRM_PHRASE);
  assert.deepEqual(view.apps.map((a) => a.name), ["App of ana"], "one app, two versions: one row");
  assert.equal(view.app_builds, 2);
  assert.deepEqual(view.orgs.map((o) => [o.slug, o.roles.join(), o.blocked]), [["acme", "developer", false], ["duo", "owner", false]]);
  assert.equal(view.blocked, null);
  assert.ok(view.deletes.length && view.stays.length);
  assert.deepEqual(snapshot(env).kv, was.kv, "looking writes nothing");

  // Refused without the typed confirmation or a choice about her apps.
  assert.equal((await deleteAs(env, ana.token, { confirm: "delete", apps: "remove" })).body.error, "confirm");
  assert.equal((await deleteAs(env, ana.token, { confirm: CONFIRM_PHRASE })).body.error, "apps");

  const r = ok(await deleteAs(env, ana.token), "delete");
  assert.equal(r.body.state, "done");
  assert.equal(r.body.counts.orgs_left, 2);
  assert.equal(r.body.counts.app_builds_removed, 2);
  assert.equal(r.body.counts.conversations, 1);
  assert.deepEqual(outside.cancelled.filter((s) => s === "sub_ana"), ["sub_ana"], "her paid plan was cancelled at Stripe");

  const now = assertOnlyTheirsWent(env, was, ana, {
    changedOnPurpose: [`refs:${zed.id}`, /^pay:/, `app-report:${zed.hashes[0]}`],
  });
  // Kept on purpose, without her.
  assert.deepEqual(JSON.parse(now.kv.get(`refs:${zed.id}`)), ["deleted"], "zed keeps the credit, not her id");
  const pay = JSON.parse(now.kv.get("pay:1700000000000:ana"));
  assert.deepEqual([pay.userId, pay.email, pay.invoice, pay.amount], ["", "", "in_ana", 1200]);
  assert.deepEqual(JSON.parse(now.kv.get("pay:1700000000000:zed")).userId, zed.id);
  const reported = JSON.parse(now.kv.get(`app-report:${zed.hashes[0]}`));
  assert.equal(reported.count, 1);
  assert.equal(reported.reports[0].login, undefined, "the report stays, without her login");
  // The organisations keep her as removed, and nothing else changed there.
  const memberOf = (slug, p) => now.orgs[`org:${slug}`].find((m) => m.principal === p);
  assert.equal(memberOf("acme", "email:ana@corp.example").state, "removed");
  assert.equal(memberOf("duo", "email:ana@corp.example").state, "removed");
  assert.deepEqual(memberOf("duo", "github:gina"), { principal: "github:gina", role: "owner", state: "active" });
  for (const slug of ["acme", "duo"]) {
    const others = (rows) => rows.filter((m) => m.principal !== "email:ana@corp.example");
    assert.deepEqual(others(now.orgs[`org:${slug}`]), others(was.orgs[`org:${slug}`]));
  }
  // Her app's links are gone; zed's still open.
  assert.equal((await hit(env, "GET", `/a/${ana.hashes[0]}`)).status, 404);
  assert.equal((await hit(env, "GET", `/a/${ana.hashes[1]}`)).status, 404);
  assert.equal((await hit(env, "GET", `/a/${zed.hashes[0]}`)).status, 200);
  // Her sign-in ends; his works.
  assert.equal((await hit(env, "GET", "/me", { token: ana.token })).status, 401);
  assert.equal((await hit(env, "GET", "/me", { token: zed.token })).status, 200);
  // What stays of the deletion itself: when, by whom, counts -- no name.
  const tomb = JSON.parse(now.kv.get(`acctdel:${ana.id}`));
  assert.deepEqual(Object.keys(tomb).sort(), ["apps", "at", "by", "counts", "id", "state"]);
  assert.equal(tomb.state, "done");
});

test("the last owner of an organisation is refused, by name, and nothing is touched", async () => {
  const env = hub();
  const ana = await emailSignIn(env, "ana@corp.example");
  ok(await hit(env, "POST", "/orgs", { token: ana, json: { slug: "solo", name: "Solo Studio" } }), "solo");
  ok(await hit(env, "POST", "/orgs", { token: ana, json: { slug: "pair", name: "Pair" } }), "pair");
  const inv = ok(await hit(env, "POST", "/orgs/pair/invites", { token: ana, json: { github: "bob", role: "owner" } }), "invite");
  ok(await hit(env, "POST", `/orgs/pair/invites/${inv.body.code}/accept`, { token: "gho_bob" }), "bob");
  const view = ok(await hit(env, "GET", "/me/delete", { token: ana }), "preview").body;
  assert.equal(view.blocked.error, "last-owner");
  assert.deepEqual(view.blocked.orgs.map((o) => o.slug), ["solo"], "pair has another owner");
  const was = snapshot(env);
  const r = await deleteAs(env, ana);
  assert.equal(r.status, 409);
  assert.equal(r.body.error, "last-owner");
  assert.match(r.body.message, /only owner of Solo Studio \(solo\)/);
  assert.match(r.body.message, /Make someone else an owner, or close the organisation/);
  const now = snapshot(env);
  assert.deepEqual(now.kv, was.kv, "not one key changed");
  assert.deepEqual(now.orgs, was.orgs);
  assert.equal((await hit(env, "GET", "/me", { token: ana })).status, 200);

  // A closing organisation is its owner's choice already: not blocked.
  ok(await hit(env, "POST", "/orgs/solo/close", { token: ana, json: { confirm_slug: "solo" } }), "close solo");
  const done = ok(await deleteAs(env, ana), "delete once solo is closing");
  assert.equal(done.body.state, "done");
  assert.equal(snapshot(env).orgs["org:solo"].find((m) => m.principal === "email:ana@corp.example").state, "removed");
});

test("a deletion that dies part-way is finished by running it again", async () => {
  const env = hub();
  const { ana, zed } = await world(env);
  const was = snapshot(env);
  const cancels = outside.cancelled.length;
  // The isolate dies while removing her Studio conversations: after her
  // organisations, plan and apps are done.
  const del = env.APPS.delete;
  let armed = true;
  env.APPS.delete = async (k) => {
    if (armed && k.startsWith(`sess:${ana.id}:`)) throw new Error("the isolate was torn down");
    return del(k);
  };
  const crashed = await deleteAs(env, ana.token);
  assert.equal(crashed.status, 500);
  assert.equal(crashed.body.error, "unfinished");
  const job = JSON.parse(env._kv.get(`acctdel:${ana.id}`));
  assert.equal(job.state, "running");
  assert.deepEqual(job.done, ["orgs", "billing", "apps", "sync"], "it got as far as the conversations");
  assert.ok(!env._kv.has(`app:${ana.hashes[1]}`), "what it did stays done");
  assert.ok(env._kv.has(`user:${ana.id}`), "the account is still there to finish");
  armed = false;
  // Pressing Delete again carries on (no second confirmation needed).
  const again = ok(await hit(env, "POST", "/me/delete", { token: ana.token, json: {} }), "again");
  assert.equal(again.body.state, "done");
  assertOnlyTheirsWent(env, was, ana, { changedOnPurpose: [`refs:${zed.id}`, /^pay:/, `app-report:${zed.hashes[0]}`] });
  // Stripe was asked once: the plan step was done before the crash.
  assert.deepEqual(outside.cancelled.slice(cancels), ["sub_ana"]);
});

test("a deletion that dies after the sign-ins went is finished by the nightly run, and by support", async () => {
  for (const finisher of ["nightly", "desk"]) {
    const env = hub({ KRATE_ADMINS: "alice" });
    const { ana, zed } = await world(env);
    const was = snapshot(env);
    const del = env.APPS.delete;
    let armed = true;
    env.APPS.delete = async (k) => {
      if (armed && k.startsWith("ident:")) throw new Error("the isolate was torn down");
      return del(k);
    };
    assert.equal((await deleteAs(env, ana.token)).status, 500);
    armed = false;
    assert.equal((await hit(env, "GET", "/me", { token: ana.token })).status, 401, "her sign-ins are gone, so she cannot press it again");
    if (finisher === "nightly") {
      assert.deepEqual(await resumeAccountDeletions(env, null, Date.now()), { finished: 0 }, "not while it may still be running");
      // The worker's own nightly entry point, ten minutes on.
      const job = JSON.parse(env._kv.get(`acctdel:${ana.id}`));
      job.touched -= 11 * 60 * 1000;
      env._kv.set(`acctdel:${ana.id}`, JSON.stringify(job));
      await worker.scheduled({ cron: "0 3 * * *" }, env);
    } else {
      const r = ok(await hit(env, "POST", "/admin/api/user/delete", { token: "gho_alice", json: { id: ana.id } }), "desk");
      assert.equal(r.body.state, "done");
    }
    assert.equal(JSON.parse(env._kv.get(`acctdel:${ana.id}`)).state, "done", finisher);
    assertOnlyTheirsWent(env, was, ana, { changedOnPurpose: [`refs:${zed.id}`, /^pay:/, `app-report:${zed.hashes[0]}`, /^scrub:/] });
  }
});

/* KV's own listing: in key order, `limit` at a time, the cursor naming
 * where the last page ended -- so a key deleted meanwhile moves nothing. */
function paging(env) {
  env.APPS.list = async ({ prefix = "", limit = 1000, cursor } = {}) => {
    const all = [...env._kv.keys()].filter((k) => k.startsWith(prefix)).sort();
    const from = cursor ? all.findIndex((k) => k > cursor) : 0;
    const page = from < 0 ? [] : all.slice(from, from + limit);
    const done = from < 0 || from + limit >= all.length;
    return { keys: page.map((name) => ({ name })), list_complete: done, cursor: done ? undefined : page[page.length - 1] };
  };
}

test("a large account is deleted over several requests, each within its share", async () => {
  const env = hub({ ACCOUNT_DELETE_OPS: "40" });
  paging(env);
  const { ana, zed } = await world(env);
  const was = snapshot(env);
  let r = await deleteAs(env, ana.token);
  let requests = 1;
  while (r.status === 202) {
    assert.equal(r.body.state, "running");
    assert.match(r.body.resume, new RegExp(`^${ana.id}\\.[0-9a-f]{32}$`));
    // The page carries on with the key, signed in or not.
    r = await hit(env, "POST", "/me/delete", { json: { resume: r.body.resume } });
    requests += 1;
    assert.ok(requests < 100, "it finishes");
  }
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.state, "done");
  assert.ok(requests > 3, `forty operations a request is several requests (${requests})`);
  assertOnlyTheirsWent(env, was, ana, { changedOnPurpose: [`refs:${zed.id}`, /^pay:/, `app-report:${zed.hashes[0]}`] });
  // A wrong key carries nothing on.
  assert.equal((await hit(env, "POST", "/me/delete", { json: { resume: `${zed.id}.00` } })).status, 404);
});

test("a GitHub account: its grant is revoked at GitHub and its moving names go with its apps", async () => {
  const env = hub();
  const gina = await githubSignIn(env, "gina");
  const id = env._kv.get("ghlogin:gina");
  const pub = ok(await hit(env, "POST", "/publish", { token: gina, bytes: SIGNED, headers: { "x-krate-name": "Bounce" } }), "publish").body;
  assert.ok(env._kv.has("channel:gina/bounce"));
  const zed = await emailSignIn(env, "zed@other.example");
  ok(await hit(env, "POST", "/publish", { token: zed, bytes: BOUNCE, headers: { "x-krate-name": "Bounce" } }), "zed publishes");
  const view = ok(await hit(env, "GET", "/me/delete", { token: gina }), "preview").body;
  assert.match(view.keep_apps_means, /same GitHub account/);
  const r = ok(await deleteAs(env, gina), "delete");
  assert.equal(r.body.state, "done");
  assert.equal(r.body.github_note, null);
  assert.ok(outside.revokedGrants.includes(gina), "Krate's whole grant revoked at GitHub");
  for (const k of env._kv.keys()) assert.ok(k === `acctdel:${id}` || (!k.includes(id) && !k.startsWith("channel:gina/") && !k.includes(pub.id) && k !== "ghlogin:gina"), `left: ${k}`);
  assert.ok(!env.BUNDLES._blobs.has(pub.id));
  assert.equal((await hit(env, "GET", "/me", { token: gina })).status, 401);
  assert.equal((await hit(env, "GET", "/me", { token: zed })).status, 200);
});

test("keeping published apps leaves them up and deletes the rest", async () => {
  const env = hub();
  const ana = await emailSignIn(env, "ana@corp.example");
  const pub = ok(await hit(env, "POST", "/publish", { token: ana, bytes: NOTES1, headers: { "x-krate-name": "Notes" } }), "publish").body;
  const r = ok(await deleteAs(env, ana, { confirm: CONFIRM_PHRASE, apps: "keep" }), "delete");
  assert.equal(r.body.state, "done");
  assert.ok(env._kv.has(`app:${pub.id}`) && env.BUNDLES._blobs.has(pub.id));
  assert.equal((await hit(env, "GET", `/a/${pub.id}`)).status, 200);
  assert.ok(!env._kv.has(`session:${ana}`) && ![...env._kv.keys()].some((k) => k.startsWith("user:") && env._kv.get(k).includes("ana@corp")));
});

test("only a fresh sign-in may delete; signed out may not", async () => {
  const env = hub();
  const ana = await emailSignIn(env, "ana@corp.example");
  const id = idOf(env, ana);
  assert.equal((await deleteAs(env, null)).status, 401);
  const key = `usess:${id}:${ana}`;
  const meta = JSON.parse(env._kv.get(key));
  meta.created_at -= 31 * 60;
  env._kv.set(key, JSON.stringify(meta));
  assert.equal((await hit(env, "GET", "/me/delete", { token: ana })).body.fresh, false);
  const r = await deleteAs(env, ana);
  assert.equal(r.status, 403);
  assert.equal(r.body.error, "sign-in-again");
  assert.ok(env._kv.has(`user:${id}`) && !env._kv.has(`acctdel:${id}`));
});

test("the export holds everything about the person and no token, key or secret", async () => {
  const env = hub();
  const { ana, zed } = await world(env);
  const zedToken = zed.token;
  const r = ok(await hit(env, "GET", "/me/export", { token: ana.token }), "export");
  assert.match(r.headers.get("content-disposition"), /^attachment; filename="krate-account-\d{4}-\d{2}-\d{2}\.json"$/);
  const x = r.body;
  assert.equal(x.format, "krate-account-export/1");
  assert.equal(x.account.email, ana.email);
  assert.equal(x.account.id, ana.id);
  assert.deepEqual(x.account.proven_emails, [ana.email]);
  assert.equal(x.sign_ins.length, 1);
  assert.deepEqual(Object.keys(x.sign_ins[0]).sort(), ["created_at", "id", "kind", "label"]);
  assert.deepEqual(x.published_apps.map((a) => a.hash).sort(), [...ana.hashes].sort());
  assert.equal(x.app_links.length, 1);
  assert.deepEqual(x.organisations.map((o) => [o.slug, o.role]), [["acme", "developer"], ["duo", "owner"]]);
  assert.equal(x.studio_conversations[0].id, "conv-ana");
  assert.equal(x.support_tickets[0].subject, "help ana");
  assert.equal(x.support_reports[0].id, ana.report);
  assert.equal(x.requests_sent[0].request, "a ana tracker");
  assert.deepEqual(x.api_keys, [{ vendor: "anthropic", ends_with: "wxyz", added: 1 }]);
  assert.equal(x.plan.plan, "monthly");
  assert.equal(x.payments[0].invoice, "in_ana");
  assert.equal(x.account_sync.devices[0].name, "ana's Mac");
  assert.deepEqual(x.account_sync.apps.map((a) => [a.entries, a.files]), [[2, 1]]);
  // Never a secret, a token, or what the hub cannot read.
  const text = r.text;
  const ticketKey = JSON.parse(env._kv.get(`tick:${ana.ticket.id}`)).key;
  for (const secret of [ana.token, ana.token.slice(4), ticketKey, "SEALED-KEY-ana", "aXY=", b64("CIPHERTEXT-ana"), b64("FILE-ENTRY-ana"), b64("sealed key ana"), b64("recovery ana"), "usess:", "krs_"]) {
    assert.ok(!text.includes(secret), `the export carries ${secret}`);
  }
  // Another person's export holds nothing of hers.
  const theirs = ok(await hit(env, "GET", "/me/export", { token: zedToken }), "zed export").text;
  assert.ok(theirs.includes("zed@other.example") && !theirs.includes(ana.email) && !theirs.includes(ana.id));
  assert.equal((await hit(env, "GET", "/me/export")).status, 401);
});
