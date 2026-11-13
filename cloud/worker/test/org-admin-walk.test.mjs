/* A new company admin's walk through shipping one internal app, with the
 * friction it found fixed (scratchpad WALK.md):
 *
 * - The link to send people is the app's own (`/run/org/<slug>/<app-id>`):
 *   it opens the version that is theirs now, after an update, a rollback
 *   or a withdrawal, on a phone or a computer. A version's link stops
 *   working when that version is withdrawn.
 * - Someone who only ever opens the app in a browser tab is counted: a tab
 *   open is a download with no check-in after it.
 * - "Who has which version" by name, for the people who run the
 *   organisation; counts only, masked under 3, for a reviewer.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-admin-walk.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";
import { initKey } from "./org-keys-tools.mjs";
import { RUN_PATH } from "../src/web-player.js";

const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const APP = "dev.krate.bounce";
const SHARE = `https://hub.example/run/org/acme/${APP}`;
const enc = encodeURIComponent;
const must = (r, what) => {
  assert.ok(r.status < 300, `${what}: ${r.status} ${JSON.stringify(r.body)}`);
  return r;
};

async function release(env, name) {
  const up = must(await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) }), `send ${name}`);
  must(await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`), `approve ${name}`);
  return up.body.digest;
}
/* An open in the Player: the tab fetches the app's web pack, which is the
 * download check -- nothing else. */
const tab = (env, who, d) => call(env, who, "GET", `/a/${d}/web`);
const inventoryApp = async (env, who) => must(await call(env, who, "GET", "/orgs/acme/inventory"), `inventory ${who}`).body.apps.find((a) => a.app_id === APP);
const whoOn = (app) => Object.fromEntries(app.versions.map((v) => [v.version, (v.names || []).map((n) => n.display).join(",")]));

async function world() {
  const env = orgEnv({ ORG_LEASE_KEY: SEED });
  await acme(env);
  await initKey(env);
  const inv = must(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "recipient" } }), "invite frank");
  must(await call(env, "frank", "POST", `/orgs/acme/invites/${inv.body.code}/accept`), "accept frank");
  return env;
}

test("the link to send people is the app's own: one address through an update, a rollback and a withdrawal, which the Player opens", async () => {
  const env = await world();
  const d1 = await release(env, "candidate-v1.krate");

  // Everyone who lists the app gets the same address, and the Player owns it.
  for (const who of ["alice", "dan"]) {
    const a = must(await call(env, who, "GET", "/orgs/acme/apps"), `apps ${who}`).body.apps.find((x) => x.app_id === APP);
    assert.strictEqual(a.share_url, SHARE, who);
    assert.notStrictEqual(a.share_url, a.releases[0].url, "not a version's link");
  }
  const m = RUN_PATH.exec(new URL(SHARE).pathname);
  assert.ok(m && m[2] === "acme" && m[3] === APP, "the Player's own address pattern carries it");
  const cur = must(await call(env, "dan", "GET", `/orgs/acme/apps/${enc(APP)}/current`, { headers: { accept: "application/json" } }), "current");
  assert.deepStrictEqual([cur.body.share_url, cur.body.digest], [SHARE, d1]);

  // An update: the same address now opens 1.0.1.
  const d2 = await release(env, "candidate-v2.krate");
  const after = must(await call(env, "dan", "GET", `/orgs/acme/apps/${enc(APP)}/current`, { headers: { accept: "application/json" } }), "after update");
  assert.deepStrictEqual([after.body.share_url, after.body.digest], [SHARE, d2]);

  // A rollback answers the address to tell people, unchanged.
  const back = must(await call(env, "alice", "PUT", `/orgs/acme/apps/${enc(APP)}/current`, { json: { digest: d1 } }), "rollback");
  assert.strictEqual(back.body.share_url, SHARE);
  assert.strictEqual(must(await call(env, "dan", "GET", `/orgs/acme/apps/${enc(APP)}/current`, { headers: { accept: "application/json" } }), "rolled").body.digest, d1);

  // Withdrawing 1.0.0 (now current): 1.0.0's own link is dead; the app's
  // address opens 1.0.1, the newest still released.
  must(await call(env, "alice", "POST", `/orgs/acme/releases/${d1}/withdraw`, { json: { reason: "loses data on save" } }), "withdraw");
  assert.strictEqual((await call(env, "dan", "GET", `/a/${d1}`)).status, 410, "the version's link stops");
  const now = must(await call(env, "dan", "GET", `/orgs/acme/apps/${enc(APP)}/current`, { headers: { accept: "application/json" } }), "after withdraw");
  assert.deepStrictEqual([now.body.share_url, now.body.digest], [SHARE, d2]);
});

test("a person who only opens the app in a browser tab is counted, on the version they opened", async () => {
  const env = await world();
  const d1 = await release(env, "candidate-v1.krate");
  assert.strictEqual((await inventoryApp(env, "alice")).people_30_days, 0, "nobody yet");

  const opened = await tab(env, "frank", d1);
  assert.strictEqual(opened.status, 200, JSON.stringify(opened.body));
  const app = await inventoryApp(env, "alice");
  assert.strictEqual(app.people_30_days, 1, "frank's tab open counts");
  assert.deepStrictEqual(app.people.map((p) => p.display), ["frank"]);
  assert.deepStrictEqual(whoOn(app), { "1.0.0": "frank" });
  const usage = must(await call(env, "alice", "GET", "/orgs/acme/usage"), "usage").body.usage;
  assert.deepStrictEqual(usage.map((u) => [u.app_id, u.people]), [[APP, 1]]);
  const ins = must(await call(env, "alice", "GET", "/orgs/acme/insights"), "insights").body.org.adoption;
  assert.strictEqual(ins.active_week, 1, "Insights sees the tab open too");

  // A refused tab open counts nobody.
  await tab(env, "stranger", d1);
  assert.strictEqual((await inventoryApp(env, "alice")).people_30_days, 1);
});

test("who has which version: each person once, on the one they opened last; names for owners and admins, masked counts for a reviewer", async () => {
  const env = await world();
  for (const [g, who] of [["pilot", "dan"]]) {
    must(await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: g } }), "group");
    must(await call(env, "alice", "PUT", `/orgs/acme/groups/${g}/members/${enc(`github:${who}`)}`), "add");
  }
  const d1 = await release(env, "candidate-v1.krate");
  must(await call(env, "alice", "PUT", `/orgs/acme/apps/${enc(APP)}/pilot`, { json: { group: "pilot" } }), "pilot");
  for (const who of ["dan", "frank", "carol"]) assert.strictEqual((await tab(env, who, d1)).status, 200, who);
  const d2 = await release(env, "candidate-v2.krate");

  // The pilot's dan opens 1.0.1 (on a computer: a check-in); the rest are on 1.0.0.
  must(await call(env, "dan", "GET", `/a/${d2}`, { headers: { accept: "application/json" } }), "dan checks in on 1.0.1");
  let app = await inventoryApp(env, "alice");
  assert.deepStrictEqual(whoOn(app), { "1.0.0": "carol,frank", "1.0.1": "dan" }, "dan counted once, on the newer one");
  assert.strictEqual(app.versions[0].version, "1.0.0", "what everyone gets comes first");
  assert.deepStrictEqual(app.versions.map((v) => [v.version, v.current, v.people, v.state]), [["1.0.0", true, 2, "released"], ["1.0.1", false, 1, "released"]]);
  assert.strictEqual(app.people_30_days, 3);

  // A reviewer reads counts, 1 and 2 masked, and no names.
  const bob = await inventoryApp(env, "bob");
  assert.deepStrictEqual(bob.versions.map((v) => [v.version, v.people]), [["1.0.0", "<3"], ["1.0.1", "<3"]]);
  assert.ok(bob.versions.every((v) => !("names" in v)) && !("people" in bob && Array.isArray(bob.people)), "no names for a reviewer");

  // The day decides before the version does: carol had 1.0.1 yesterday
  // (say, before a rollback) and opened 1.0.0 today, so she is on 1.0.0.
  const store = env.ORGS.object("acme");
  store.sql.exec(
    "INSERT INTO usage_daily (day, app_id, digest, version, principal) VALUES (?, ?, ?, ?, ?)",
    store.orgDay(Math.floor(Date.now() / 1000) - 86400), APP, d2, "1.0.1", "github:carol",
  );

  // Released to everyone; frank opens it in his tab and moves over.
  must(await call(env, "alice", "POST", `/orgs/acme/apps/${enc(APP)}/pilot/release`), "to everyone");
  assert.strictEqual((await tab(env, "frank", d2)).status, 200);
  app = await inventoryApp(env, "alice");
  assert.deepStrictEqual(whoOn(app), { "1.0.1": "dan,frank", "1.0.0": "carol" }, "carol is the one still on the old version");
  assert.strictEqual(app.versions[0].current && app.versions[0].version, "1.0.1");

  // Withdrawing 1.0.0 keeps who was on it, marked withdrawn.
  must(await call(env, "alice", "POST", `/orgs/acme/releases/${d1}/withdraw`, { json: { reason: "old" } }), "withdraw");
  app = await inventoryApp(env, "alice");
  assert.deepStrictEqual(app.versions.find((v) => v.version === "1.0.0").state, "withdrawn");

  // Someone removed stops opening it at once, in a tab too.
  must(await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:frank")}`), "remove frank");
  const gone = await tab(env, "frank", d2);
  assert.deepStrictEqual([gone.status, gone.body.error], [403, "removed"]);
});

test("before removing someone, the people list says until when their offline access still opens apps; only owners and admins read it", async () => {
  const env = await world();
  const d1 = await release(env, "candidate-v1.krate");
  const lease = must(await call(env, "frank", "GET", `/orgs/acme/leases/${d1}`), "frank's offline access");
  const until = lease.body.lease.expires_at;
  const people = async (who) => must(await call(env, who, "GET", "/orgs/acme"), `show ${who}`).body.members;
  const frankOf = (list) => list.find((m) => m.principal === "github:frank");
  assert.strictEqual(frankOf(await people("alice")).offline_until, until, "the owner sees it");
  assert.ok(!("offline_until" in (await people("alice")).find((m) => m.principal === "github:dan")), "nobody else holds any");
  assert.strictEqual(frankOf(await people("bob")).offline_until, undefined, "a reviewer does not");
  // Removed: online access stops at once; the list still says what holds.
  must(await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:frank")}`), "remove");
  assert.strictEqual((await call(env, "frank", "GET", `/orgs/acme/leases/${d1}`)).status, 403);
});
