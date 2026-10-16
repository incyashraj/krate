/* Insights, hub side (IC-959).
 *
 * One known history -- eighty days of an organisation, played through the
 * real routes at the times it happened -- and every number the owner,
 * the developer, the reviewer and the recipient read, against a hand count
 * written out below. Then each role sees exactly its sections and no name
 * it may not see; then a store of 500 people, 50 apps and 90 days of
 * opens answers in well under a second.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-insights.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join as joinPath } from "node:path";
import { createHash } from "node:crypto";
import { orgEnv, call } from "./org-tools.mjs";
import { device, register, initKey, b64 } from "./org-keys-tools.mjs";
import { FEWER, sectionsFor } from "../src/org-insights.js";

const here = dirname(fileURLToPath(import.meta.url));
const FIX = joinPath(here, "fixtures", "org-inventory");
const fixture = (name) => new Uint8Array(readFileSync(joinPath(FIX, name)));
const KEYS = JSON.parse(readFileSync(joinPath(FIX, "keys.json"), "utf8"));
const LEASE_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const DAY = 86400;
const enc = encodeURIComponent;

// Noon UTC, so "k days ago" is always the day k days before.
const T0 = Math.floor(Date.UTC(2026, 9, 15, 12, 0, 0) / 1000);
const ago = (k, plus = 0) => T0 - k * DAY + plus;
const dayAgo = (k) => new Date(ago(k) * 1000).toISOString().slice(0, 10);

async function at(seconds, fn) {
  const real = Date.now;
  Date.now = () => seconds * 1000;
  try {
    return await fn();
  } finally {
    Date.now = real;
  }
}
const must = (r, what, status) => {
  if (status ? r.status !== status : r.status >= 300) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body)}`);
  return r;
};
const look = (env, who, digest) => call(env, who, "GET", `/a/${digest}`, { headers: { accept: "application/json" } });
const insights = async (env, who, when = T0) => {
  const r = await at(when, () => call(env, who, "GET", "/orgs/acme/insights"));
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  return r.body;
};

async function join(env, who, role) {
  const by = who.includes("@") ? { email: who } : { github: who };
  const inv = must(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { ...by, role } }), `invite ${who}`);
  must(await call(env, who, "POST", `/orgs/acme/invites/${inv.body.code}/accept`), `accept ${who}`);
}
async function send(env, who, name) {
  return must(await call(env, who, "POST", "/orgs/acme/candidates", { bytes: fixture(name) }), `send ${name}`, 201).body.digest;
}
const approve = async (env, digest) => must(await call(env, "bob", "POST", `/orgs/acme/candidates/${digest}/approve`), "approve");

/* ------------------------------------------------------------------ the history
 *
 * day 80  Acme made: alice owner; erin admin, bob reviewer, carol and frank
 *         developers, dan, gina, ivy, hal and joe recipients (gina..joe by
 *         email). Nobody signs in with a company account.
 * day 75  alice's MacBook makes the key; on day 74 erin's laptop is added and
 *         given it, and is never seen again.
 * day 40  carol sends Expenses 1.0.0; bob approves it a day later (day 39).
 * day 38  dan, erin and hal open 1.0.0;   day 35  carol does;  day 30  hal.
 * day 20  an expired invitation (old@), and a stranger is refused.
 * day 10  carol sends Expenses 1.0.1; bob approves it two hours later.
 * day 9   dan, erin, alice open 1.0.1;    day 8  dan;  day 7  carol.
 * day 6   dan and gina open 1.0.1.
 * day 5   alice rolls Expenses back to 1.0.0 (a rollback); ivy opens 1.0.0.
 * day 4   alice makes 1.0.1 current again (not a rollback).
 * day 3   frank sends Rota; dan tries to approve it (refused); bob approves
 *         it an hour after it was sent. dan takes offline access to 1.0.0.
 * day 2   bob withdraws 1.0.0; alice invites kim@ (open); alice removes ivy.
 * day 1   dan and erin open 1.0.1, dan twice; dan's look at 1.0.0 is
 *         refused (withdrawn); a stranger is refused twice.
 * today   dan opens 1.0.1 an hour before noon; alice's MacBook checks in.
 */
async function history() {
  const env = orgEnv({ ORG_SECRET_KEY: "org-secret-key-for-tests", ORG_LEASE_KEY: LEASE_SEED });
  let alicesMac;
  await at(ago(80), async () => {
    must(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
    must(await call(env, "alice", "PUT", "/orgs/acme/root-key", { json: { public_key: KEYS.root } }), "root");
    for (const [who, role] of [["erin", "admin"], ["bob", "reviewer"], ["carol", "developer"], ["frank", "developer"], ["dan", "recipient"]]) await join(env, who, role);
    for (const who of ["gina@acme.test", "ivy@acme.test", "hal@acme.test", "joe@acme.test"]) await join(env, who, "recipient");
  });
  await at(ago(75), async () => {
    alicesMac = await initKey(env);
  });
  await at(ago(74), async () => {
    const erins = device();
    must(await register(env, "erin", erins, "Erin's laptop"), "erin's device");
    must(await call(env, "alice", "POST", "/orgs/acme/keys/grants", { json: { device_id: erins.id, version: 1, blob: b64(300), by_device: alicesMac.id } }), "grant");
  });
  const d1 = await at(ago(40), () => send(env, "carol", "expenses-1.krate"));
  await at(ago(39), () => approve(env, d1));
  await at(ago(38), async () => {
    for (const who of ["dan", "erin", "hal@acme.test"]) must(await look(env, who, d1), `look ${who}`);
  });
  await at(ago(35), async () => must(await look(env, "carol", d1), "look carol"));
  // The edges of the windows: hal on day 30 (just outside 30 days), carol on day 7 (just outside 7).
  await at(ago(30), async () => must(await look(env, "hal@acme.test", d1), "look hal"));
  await at(ago(20), async () => {
    must(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "old@acme.test", role: "developer" } }), "old invite");
    assert.strictEqual((await look(env, "stranger", d1)).status, 403);
  });
  const d2 = await at(ago(10), () => send(env, "carol", "expenses-2.krate"));
  await at(ago(10, 7200), () => approve(env, d2));
  await at(ago(9), async () => {
    for (const who of ["dan", "erin", "alice"]) must(await look(env, who, d2), `look ${who}`);
  });
  await at(ago(8), async () => must(await look(env, "dan", d2), "look dan"));
  await at(ago(7), async () => must(await look(env, "carol", d2), "look carol"));
  await at(ago(6), async () => {
    for (const who of ["dan", "gina@acme.test"]) must(await look(env, who, d2), `look ${who}`);
  });
  await at(ago(5), async () => {
    must(await call(env, "alice", "PUT", "/orgs/acme/apps/com.acme.expenses/current", { json: { digest: d1 } }), "rollback");
    must(await look(env, "ivy@acme.test", d1), "look ivy");
  });
  await at(ago(4), async () => must(await call(env, "alice", "PUT", "/orgs/acme/apps/com.acme.expenses/current", { json: { digest: d2 } }), "forward"));
  const rota = await at(ago(3), () => send(env, "frank", "rota.krate"));
  await at(ago(3, 600), async () => {
    assert.strictEqual((await call(env, "dan", "POST", `/orgs/acme/candidates/${rota}/approve`)).status, 403);
  });
  await at(ago(3, 3600), () => approve(env, rota));
  await at(ago(3, 4000), async () => must(await call(env, "dan", "GET", `/orgs/acme/leases/${d1}`), "lease"));
  await at(ago(2), async () => {
    must(await call(env, "bob", "POST", `/orgs/acme/releases/${d1}/withdraw`, { json: { reason: "replaced by 1.0.1" } }), "withdraw");
    must(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "kim@acme.test", role: "developer" } }), "kim invite");
    must(await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("email:ivy@acme.test")}`), "remove ivy");
  });
  await at(ago(1), async () => {
    for (const who of ["dan", "erin", "dan"]) must(await look(env, who, d2), `look ${who}`);
    assert.strictEqual((await look(env, "dan", d1)).status, 410);
    for (let i = 0; i < 2; i++) assert.strictEqual((await look(env, "stranger", d2)).status, 403);
  });
  await at(T0 - 3600, async () => {
    must(await look(env, "dan", d2), "look dan today");
    must(await call(env, "alice", "GET", `/orgs/acme/keys?device=${alicesMac.id}`), "mac checks in");
  });
  return { env, d1, d2, rota, alicesMac };
}

const zeros = (n) => Array.from({ length: n }, () => 0);
/// A 30-day series from {days ago: n}, oldest first.
const series = (known, fill = 0) => zeros(30).map((_, i) => (known[29 - i] !== undefined ? known[29 - i] : fill));

test("every number matches a hand count of a known history", async () => {
  const { env, d1, d2, alicesMac } = await history();

  /* ---- the owner: Is Acme getting value, and is it safe? ---- */
  const o = await insights(env, "alice");
  assert.deepStrictEqual(o.sections, ["org", "developer", "reviewer", "yours"]);
  assert.strictEqual(o.names, true);
  const a = o.org.adoption;
  // Members now: alice erin bob carol frank dan gina hal joe (ivy removed).
  assert.strictEqual(a.members, 9);
  // Days 0-6, members only: dan, gina, erin (ivy left; carol was day 7).  Days 0-29: + alice, carol (hal was day 30).
  assert.deepStrictEqual([a.active_week, a.active_month], [3, 5]);
  // Per day: 9 -> dan erin alice; 8 -> dan; 7 -> carol; 6 -> dan gina; 5 -> ivy, gone; 1 -> dan erin; 0 -> dan.
  assert.deepStrictEqual(a.daily.map((d) => d.people), series({ 9: 3, 8: 1, 7: 1, 6: 2, 1: 2, 0: 1 }));
  assert.deepStrictEqual([a.daily[0].day, a.daily[29].day], [dayAgo(29), dayAgo(0)]);
  // Weeks back from today: 0 -> dan gina erin; 1 -> dan erin alice carol; 4 (days 28-34) -> hal; 5 (35-41) -> dan erin hal carol.
  assert.deepStrictEqual(a.trend.map((w) => w.people), [0, 0, 0, 0, 0, 0, 4, 1, 0, 0, 4, 3]);
  assert.deepStrictEqual([a.trend[11].from, a.trend[11].to, a.trend[0].from], [dayAgo(6), dayAgo(0), dayAgo(83)]);
  // Two apps with a release; Expenses opened by 5 members in 30 days; Rota by nobody.
  assert.deepStrictEqual([a.apps, a.apps_used], [2, 1]);
  assert.deepStrictEqual(a.top, [{ app_id: "com.acme.expenses", name: "Expenses", people: 5 }]);
  // Rota was released three days ago: it has not had its 30 days, so it is
  // not "unused" yet. Four weeks on, still unopened, it is.
  assert.deepStrictEqual(a.unused, []);
  const later = (await insights(env, "alice", ago(-28))).org.adoption;
  assert.deepStrictEqual(later.unused.map((u) => [u.app_id, u.version, u.owner.principal, u.released_at]), [["com.acme.rota", "1.0.0", "github:frank", ago(3, 3600)]]);
  // Everyone joined on day 80: nobody is new enough to be excused.
  assert.strictEqual(a.joined_not_opened, 0);

  const p = o.org.people;
  assert.deepStrictEqual(p.by_role, { owner: 1, admin: 1, developer: 2, reviewer: 1, recipient: 4 });
  assert.deepStrictEqual(p.by_sign_in, { github: 6, email: 3, company: 0 });
  // hal last opened on day 30, one day past the 30; joe never did anything after joining.
  assert.deepStrictEqual(
    p.inactive.map((m) => [m.principal, m.last_active]),
    [["email:hal@acme.test", dayAgo(30)], ["email:joe@acme.test", null]],
  );
  assert.deepStrictEqual([p.company_sign_in, p.without_company_sign_in], [null, []]);
  // old@ ran out on day 13; kim@ is open.
  assert.deepStrictEqual(p.invites.map((i) => [i.principal, i.role, i.invited_by.principal]), [["email:kim@acme.test", "developer", "github:alice"]]);

  const s = o.org.security;
  // ivy's removal left the key to be replaced.
  assert.deepStrictEqual([s.key.version, s.key.state], [1, "stale"]);
  assert.deepStrictEqual(
    s.devices.map((d) => [d.name, d.principal, d.holds_current, d.last_seen, d.identified, d.silent]),
    // Erin's laptop never said which computer it is (a Krate from before
    // ?device=, K-1115): not identified, and so never called silent.
    [["Alice's MacBook", "github:alice", true, T0 - 3600, true, false], ["Erin's laptop", "github:erin", true, null, false, false]],
  );
  assert.strictEqual(s.devices[0].device_id, alicesMac.id);
  assert.deepStrictEqual(s.publishers.map((x) => [x.principal, x.last_sent_at]), [["github:alice", null], ["github:carol", ago(10)], ["github:frank", ago(3)]]);
  assert.deepStrictEqual([s.approvers, s.two_person], [2, true]);
  // This week: two strangers, dan's approval, dan's withdrawn look. Before: one stranger in four weeks.
  assert.deepStrictEqual(s.refusals.week, [{ reason: "not-a-member", n: 2 }, { reason: "release-withdrawn", n: 1 }, { reason: "role-cannot", n: 1 }]);
  assert.deepStrictEqual([s.refusals.week_total, s.refusals.weekly_average_before, s.refusals.spike], [4, 0.25, false]);
  assert.deepStrictEqual(
    s.withdrawn_offline.map((w) => [w.digest, w.version, w.people, w.until]),
    [[d1, "1.0.0", 1, ago(3, 4000) + 7 * DAY]],
  );
  assert.deepStrictEqual([s.sign_in.company, s.directory, s.audit_export], [null, null, null]);

  // Approved in 30 days: 1.0.1 and Rota; waits 1 day, 2 hours, 1 hour -> median 2 hours.
  assert.deepStrictEqual(o.org.releases, {
    approved_month: 2,
    median_wait_seconds: 7200,
    approvals_counted: 3,
    waiting: 0,
    oldest_waiting_since: null,
    withdrawn_month: 1,
    rollbacks_month: 1,
  });
  // Seats: owner, admin, two developers, reviewer.
  assert.deepStrictEqual(o.org.cost, { plan: "pilot", state: "pilot", seats_used: 5, seats_paid: null, free_seats: 3, active_month: 5 });
  // alice sent nothing; her own: her MacBook, and Expenses on day 9.
  assert.deepStrictEqual(o.developer.apps, []);
  assert.deepStrictEqual(o.yours.apps, [{ app_id: "com.acme.expenses", name: "Expenses", days: 1, last: dayAgo(9) }]);
  assert.deepStrictEqual(o.yours.devices.map((d) => [d.name, d.last_seen]), [["Alice's MacBook", T0 - 3600]]);

  /* ---- the developer: How is my app doing? ---- */
  const c = await insights(env, "carol");
  assert.deepStrictEqual([c.sections, c.names], [["developer", "yours"], false]);
  assert.deepStrictEqual(c.developer.apps.map((x) => x.app_id), ["com.acme.expenses"]);
  const e = c.developer.apps[0];
  assert.deepStrictEqual([e.name, e.version, e.released], ["Expenses", "1.0.1", true]);
  // Days 0-6, everyone who opened it: dan gina ivy erin.
  assert.strictEqual(e.people_week, 4);
  // Per day: 9 -> 3; 8, 7, 6, 5, 1, 0 -> 1 or 2 people, which a developer reads as fewer than 3.
  assert.deepStrictEqual(e.daily.map((d) => d.people), series({ 9: 3, 8: FEWER, 7: FEWER, 6: FEWER, 5: FEWER, 1: FEWER, 0: FEWER }));
  // Weeks: 0 -> dan gina ivy erin; 1 -> dan erin alice carol; 4 -> hal alone (masked); 5 -> dan erin hal carol.
  assert.deepStrictEqual(e.trend.map((w) => w.people), [0, 0, 0, 0, 0, 0, 4, FEWER, 0, 0, 4, 4]);
  // This week on 1.0.1: dan gina erin; still on 1.0.0: ivy.
  assert.deepStrictEqual(e.versions.map((v) => [v.version, v.current, v.people]), [["1.0.1", true, 3], ["1.0.0", false, FEWER]]);
  // In 30 days: three strangers (days 20, 1, 1) and dan's withdrawn look.
  assert.deepStrictEqual(e.refusals, [{ reason: "not-a-member", n: 3 }, { reason: "release-withdrawn", n: 1 }]);
  assert.strictEqual(e.data, null);
  assert.deepStrictEqual(e.connections, [{ name: "finance-api", uses_week: 0 }]);
  assert.deepStrictEqual(e.waits.map((w) => [w.version, w.state, w.waited_seconds]), [["1.0.1", "released", 7200], ["1.0.0", "withdrawn", DAY]]);
  const f = await insights(env, "frank");
  // dan's refused approval of Rota is the organisation's refusal, not one Rota met.
  assert.deepStrictEqual(f.developer.apps.map((x) => [x.app_id, x.people_week, x.waits.map((w) => w.waited_seconds), x.refusals]), [["com.acme.rota", 0, [3600], []]]);

  /* ---- the reviewer: What needs me? ---- */
  const b = await insights(env, "bob");
  assert.deepStrictEqual(b.sections, ["reviewer", "yours"]);
  // bob approved 1.0.1 (2 hours) and Rota (1 hour) in 30 days.
  assert.deepStrictEqual(b.reviewer, { queue: [], approved_by_you_month: 2, approved_month: 2, median_wait_seconds_month: 5400 });

  /* ---- the recipient: their own ---- */
  const d = await insights(env, "dan");
  assert.deepStrictEqual(d.sections, ["yours"]);
  // dan opened Expenses on days 9, 8, 6, 1 and today.
  assert.deepStrictEqual(d.yours, { devices: [], apps: [{ app_id: "com.acme.expenses", name: "Expenses", days: 5, last: dayAgo(0) }] });

  // The daily counts the module keeps agree with the rows they summarise.
  const store = env.ORGS.object("acme");
  for (const x of store.insightsApps()) assert.strictEqual(x.current ? x.current.digest : null, store.currentOf(x.app_id), x.app_id);
  const db = store.ctx.storage.db;
  const kept = db.prepare("SELECT day, app_id, people FROM app_daily ORDER BY day, app_id").all().map((r) => ({ ...r }));
  const recount = db.prepare("SELECT day, app_id, COUNT(DISTINCT principal) AS people FROM usage_daily GROUP BY day, app_id ORDER BY day, app_id").all().map((r) => ({ ...r }));
  assert.deepStrictEqual(kept, recount);
  // And nothing in what the hub keeps for insights names an app's content.
  const cols = db.prepare("SELECT name FROM pragma_table_info('insights_daily')").all().map((r) => r.name);
  assert.deepStrictEqual(cols, ["day", "metric", "key", "n"]);
  void d2;
});

/* ------------------------------------------------------------ sections and names */

/// A smaller organisation with something waiting: Expenses 1.0.0 released and
/// opened by dan, erin and gina; Rota waiting from frank.
async function waitingWorld() {
  const env = orgEnv({ ORG_SECRET_KEY: "org-secret-key-for-tests" });
  await at(ago(30), async () => {
    must(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
    must(await call(env, "alice", "PUT", "/orgs/acme/root-key", { json: { public_key: KEYS.root } }), "root");
    for (const [who, role] of [["erin", "admin"], ["bob", "reviewer"], ["carol", "developer"], ["frank", "developer"], ["dan", "recipient"]]) await join(env, who, role);
    await join(env, "gina@acme.test", "recipient");
  });
  const d1 = await at(ago(5), () => send(env, "carol", "expenses-1.krate"));
  await at(ago(5, 60), () => approve(env, d1));
  await at(ago(1), async () => {
    for (const who of ["dan", "erin", "gina@acme.test"]) must(await look(env, who, d1), "look");
    must(await look(env, "dan", d1), "look");
  });
  await at(ago(0, -60), async () => must(await look(env, "dan", d1), "look"));
  const rota = await at(ago(2), () => send(env, "frank", "rota.krate"));
  return { env, d1, rota };
}

const OTHERS = ["github:alice", "github:erin", "github:bob", "github:carol", "github:frank", "github:dan", "email:gina@acme.test"];
/// Every principal and display name in an answer, wherever it sits.
function namesIn(body) {
  const text = JSON.stringify(body);
  return OTHERS.filter((p) => text.includes(p) || text.includes(`"${p.split(":")[1]}"`));
}

test("an owner releasing alone is told so, and a reviewer joining turns the second approval on", async () => {
  const env = orgEnv({ ORG_SECRET_KEY: "org-secret-key-for-tests", ORG_LEASE_KEY: LEASE_SEED });
  await at(ago(3), async () => {
    must(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
    must(await call(env, "alice", "PUT", "/orgs/acme/root-key", { json: { public_key: KEYS.root } }), "root");
  });
  const d = await at(ago(2), () => send(env, "alice", "expenses-1.krate"));
  const out = await at(ago(2, 60), () => call(env, "alice", "POST", `/orgs/acme/candidates/${d}/approve`));
  must(out, "alice releases her own");
  assert.strictEqual(out.body.release.solo, true, JSON.stringify(out.body));
  const first = await insights(env, "alice");
  let s = first.org.security;
  assert.deepStrictEqual([s.two_person, s.solo_releases, s.approvers], [false, 1, 1]);
  // A day-old organisation: its owner joined inside the month and nothing
  // released this week is called unused.
  assert.deepStrictEqual([first.org.adoption.joined_not_opened, first.org.adoption.unused.length], [1, 0]);
  await at(ago(1), () => join(env, "bob", "reviewer"));
  s = (await insights(env, "alice")).org.security;
  assert.deepStrictEqual([s.two_person, s.solo_releases, s.approvers], [true, 1, 2]);
});

test("each role sees exactly its sections, and only owners and admins see names", async () => {
  const { env, rota } = await waitingWorld();
  const expect = {
    alice: ["org", "developer", "reviewer", "yours"],
    erin: ["org", "yours"],
    carol: ["developer", "yours"],
    frank: ["developer", "yours"],
    bob: ["reviewer", "yours"],
    dan: ["yours"],
    "gina@acme.test": ["yours"],
  };
  for (const [who, sections] of Object.entries(expect)) {
    const body = await insights(env, who);
    assert.deepStrictEqual(body.sections, sections, who);
    // Exactly those sections in the answer, and no other.
    assert.deepStrictEqual(["org", "developer", "reviewer", "yours"].filter((k) => k in body), sections, who);
    assert.deepStrictEqual(sectionsFor(body.role), sections, who);
    const high = who === "alice" || who === "erin";
    assert.strictEqual(body.names, high, who);
    if (!high) assert.deepStrictEqual(namesIn(body), [], `${who} reads no one's name`);
  }

  // The owner's org section names people; the admin's too.
  const o = await insights(env, "alice");
  assert.ok(namesIn(o.org).includes("github:frank"));
  assert.deepStrictEqual(o.reviewer.queue.map((q) => [q.digest, q.sent_by.principal, q.first, q.yours]), [[rota, "github:frank", true, false]]);
  assert.strictEqual((await insights(env, "erin")).org.people.by_role.recipient, 2);

  // The reviewer's queue: what waits, how long, what it asks for -- not who sent it.
  const b = await insights(env, "bob");
  const q = b.reviewer.queue[0];
  assert.deepStrictEqual([q.name, q.version, q.waited_seconds, q.first, "sent_by" in q], ["Rota", "1.0.0", 2 * DAY, true, false]);
  // The sender's own queue entry says it is theirs (someone else approves it).
  const fr = await insights(env, "frank");
  assert.deepStrictEqual(fr.developer.apps.map((a) => [a.app_id, a.released, a.waits[0].state]), [["com.acme.rota", false, "waiting"]]);

  // Counts of 1 or 2 people read "<3" to a developer and exactly to an owner.
  const c = await insights(env, "carol");
  const e = c.developer.apps[0];
  // Yesterday: dan erin gina (3, exact); today: dan alone.
  assert.deepStrictEqual([e.daily[28].people, e.daily[29].people], [3, FEWER]);
  const mine = (await at(T0, () => call(env, "alice", "GET", "/orgs/acme/insights"))).body;
  assert.deepStrictEqual(mine.developer.apps, [], "alice sent nothing here");
  // An owner who sent the app reads the exact count.
  const asOwner = await at(T0, async () => {
    must(await call(env, "alice", "PUT", "/orgs/acme/apps/com.acme.expenses/owner", { json: { principal: "github:alice" } }), "owner");
    return (await call(env, "alice", "GET", "/orgs/acme/insights")).body;
  });
  assert.deepStrictEqual(asOwner.developer.apps.map((a) => [a.app_id, a.daily[29].people]), [["com.acme.expenses", 1]]);

  // Not a member: refused like everything else.
  assert.deepStrictEqual(
    [(await call(env, "stranger", "GET", "/orgs/acme/insights")).status, (await call(env, null, "GET", "/orgs/acme/insights")).status],
    [403, 401],
  );
});

/* ------------------------------------------------------------ the other counts */

test("the directory's requests, a device's check-in and the export are counted", async () => {
  const { env } = await waitingWorld();
  const token = "scimtok_" + "a".repeat(40);
  const hash = createHash("sha256").update(token).digest("hex");
  // Two requests that work, one that fails, one with a revoked token.
  const store = env.ORGS.object("acme");
  store.sql.exec("INSERT INTO scim_tokens (id, hash, hint, created_at, created_by) VALUES ('st_aaaaaaaa', ?, 'aaaa', ?, 'github:alice')", hash, T0);
  store.sql.exec("INSERT INTO scim_tokens (id, hash, hint, created_at, created_by, revoked_at) VALUES ('st_bbbbbbbb', ?, 'bbbb', ?, 'github:alice', ?)", "b".repeat(64), T0, T0);
  const ask = (op, args) =>
    at(T0, () => store.fetch(new Request("https://org/", { method: "POST", body: JSON.stringify({ op, slug: "acme", principals: [], args }) })));
  await ask("scim-users-list", { token_hash: hash });
  await ask("scim-user-get", { token_hash: hash, id: "nobody" });
  await ask("scim-groups-list", { token_hash: hash });
  await ask("scim-users-list", { token_hash: "b".repeat(64) });
  const s = (await insights(env, "alice")).org.security;
  assert.deepStrictEqual(s.directory, { tokens: 1, last_used_at: T0, week: { ok: 2, failed: 1, refused: 1 } });

  // A device's look at the keyring is its check-in, at most once an hour.
  const mac = await at(T0 - 7200, () => initKey(env));
  await at(T0 - 3000, () => call(env, "alice", "GET", `/orgs/acme/keys?device=${mac.id}`));
  await at(T0 - 2000, () => call(env, "alice", "GET", `/orgs/acme/keys?device=${mac.id}`));
  const dev = (await insights(env, "alice")).org.security.devices[0];
  assert.deepStrictEqual([dev.last_seen, dev.silent], [T0 - 3000, false]);

  // The organisation's export carries the owner's numbers.
  const ex = await at(T0, () => store.fetch(new Request("https://org/", { method: "POST", body: JSON.stringify({ op: "export-begin", slug: "acme", principals: ["github:alice"], args: {} }) })));
  const data = (await ex.json()).body.data;
  assert.deepStrictEqual([data.insights.adoption.members, data.insights.adoption.active_week], [7, 3]);
});

test("an older store starts its counts from the opens it already kept", async () => {
  const { env } = await waitingWorld();
  const db = env.ORGS.object("acme").ctx.storage.db;
  const before = db.prepare("SELECT day, app_id, people FROM app_daily ORDER BY day").all().map((r) => ({ ...r }));
  for (const t of ["people_daily", "app_daily", "member_seen"]) db.exec(`DELETE FROM ${t}`);
  db.exec("DELETE FROM inventory_meta WHERE key = 'insights_since'");
  const o = await insights(env, "alice");
  assert.deepStrictEqual(db.prepare("SELECT day, app_id, people FROM app_daily ORDER BY day").all().map((r) => ({ ...r })), before);
  assert.deepStrictEqual([o.org.adoption.active_week, o.since], [3, T0]);
});

/* ------------------------------------------------------------ 500 people */

test("500 people, 50 apps and 90 days of opens answer in well under a second", async () => {
  const env = orgEnv({ ORG_SECRET_KEY: "org-secret-key-for-tests" });
  await at(ago(100), async () => {
    must(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
    await join(env, "carol", "developer");
  });
  const store = env.ORGS.object("acme");
  const db = store.ctx.storage.db;
  const PEOPLE = 500;
  const APPS = 50;
  const people = Array.from({ length: PEOPLE }, (_, i) => `email:p${String(i).padStart(3, "0")}@acme.test`);
  db.exec("BEGIN");
  const addMember = db.prepare("INSERT INTO members (principal, role, state, added_at, added_by) VALUES (?, ?, 'active', ?, 'github:alice')");
  people.forEach((p, i) => addMember.run(p, i % 10 === 0 ? "developer" : i % 25 === 1 ? "reviewer" : "recipient", ago(100)));
  const addCand = db.prepare(
    "INSERT INTO candidates (digest, name, version, namespace, size, signer, big, uploaded_by, uploaded_at, state, approved_by, approved_at, app_id) VALUES (?, ?, ?, 'acme/x', 1000, '', 0, ?, ?, ?, 'github:alice', ?, ?)",
  );
  const digests = [];
  for (let a = 0; a < APPS; a++) {
    for (let v = 0; v < 4; v++) {
      const digest = createHash("sha256").update(`app${a}v${v}`).digest("hex");
      const sender = a === 0 ? "github:carol" : people[(a * 10) % PEOPLE];
      const waiting = v === 3 && a % 7 === 0;
      addCand.run(digest, `App ${a}`, `1.${v}.0`, sender, ago(90 - v * 20), waiting ? "waiting-for-approval" : "released", waiting ? null : ago(89 - v * 20), `com.acme.app${a}`);
      if (v === 2 || (v === 3 && a % 7 !== 0)) digests[a] = { digest, version: `1.${v}.0` };
    }
  }
  // Each person opens four apps on five days of seven: one row each.
  const use = db.prepare("INSERT OR IGNORE INTO usage_daily (day, app_id, digest, version, principal) VALUES (?, ?, ?, ?, ?)");
  let rows = 0;
  for (let k = 0; k < 90; k++) {
    if (k % 7 === 5 || k % 7 === 6) continue;
    const day = dayAgo(k);
    people.forEach((p, i) => {
      for (const j of [0, 1, 2, 3]) {
        const a = (i * 3 + j * 7 + k) % APPS;
        use.run(day, `com.acme.app${a}`, digests[a].digest, digests[a].version, p);
        rows++;
      }
    });
  }
  // Ninety days of refusals and seen days, as a busy organisation has.
  const count = db.prepare("INSERT INTO insights_daily (day, metric, key, n) VALUES (?, 'refusal', ?, ?)");
  for (let k = 0; k < 90; k++) count.run(dayAgo(k), `not-in-audience|com.acme.app${k % APPS}`, 1 + (k % 4));
  db.exec("DELETE FROM inventory_meta WHERE key = 'insights_since'");
  db.exec("COMMIT");
  assert.ok(rows > 120000, `${rows} opens`);

  // The first read after the counts arrive builds the daily aggregates once.
  await insights(env, "alice", T0 - 60);
  // The one-pass current release is the one org-releases.js decides.
  for (const a of store.insightsApps()) assert.strictEqual(a.current ? a.current.digest : null, store.currentOf(a.app_id), a.app_id);
  const timings = {};
  for (const who of ["alice", "carol", "email:p001@acme.test"]) {
    const login = who.startsWith("email:") ? who.slice(6) : who;
    const started = performance.now();
    const body = await insights(env, login);
    timings[login] = Math.round(performance.now() - started);
    if (who === "alice") {
      assert.strictEqual(body.org.adoption.members, PEOPLE + 2);
      assert.strictEqual(body.org.adoption.apps, APPS);
      assert.ok(body.org.adoption.active_month > 400, String(body.org.adoption.active_month));
      assert.strictEqual(body.org.adoption.top.length, 5);
    }
  }
  console.log(`insights for 500 people, 50 apps, ${rows} opens over 90 days: ${JSON.stringify(timings)} ms`);
  for (const [who, ms] of Object.entries(timings)) assert.ok(ms < 400, `${who}: ${ms} ms`);
});

test("K-1115: a computer that never said which it is reads 'not identified', never silent; one that stopped checking in is silent", async () => {
  const env = orgEnv({ ORG_LEASE_KEY: LEASE_SEED });
  await at(ago(100), async () => {
    must(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
  });
  // Alice's Mac runs IC-959's Krate: it checks in with ?device=.
  const mac = await at(ago(100), () => initKey(env));
  await at(ago(90), () => call(env, "alice", "GET", `/orgs/acme/keys?device=${mac.id}`));
  // Her old PC runs an older Krate: it reads the keyring without saying which it is.
  const pc = device();
  await at(ago(100), () => register(env, "alice", pc, "Old PC"));
  for (const k of [80, 40, 2]) await at(ago(k), () => call(env, "alice", "GET", "/orgs/acme/keys"));
  const devices = (await insights(env, "alice")).org.security.devices;
  const by = Object.fromEntries(devices.map((d) => [d.name, [d.identified, d.silent]]));
  assert.deepStrictEqual(by, { "Alice's MacBook": [true, true], "Old PC": [false, false] });
  // Its own page says the same.
  const yours = (await insights(env, "alice")).yours.devices;
  assert.deepStrictEqual(yours.map((d) => [d.name, d.identified]), [["Alice's MacBook", true], ["Old PC", false]]);
});
