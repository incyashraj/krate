/* Staged rollout (G10): a pilot group first, then everyone; a group pinned
 * to one version. Every place that decides what a person gets is asked:
 * the app's link, the listing, the check-in, a download, a lease and its
 * renewal, team data, the releases list, the inventory and Insights.
 *
 * The bundles are the real signed fixtures (three versions of one app,
 * dev.krate.bounce: 1.0.0, 1.0.1, 1.0.2), judged by the real validator.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-rollout.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";
import { initKey } from "./org-keys-tools.mjs";

const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const APP = "dev.krate.bounce";
const V1 = "candidate-v1.krate";
const V2 = "candidate-v2.krate";
const V3 = "root-signed.krate";
const enc = encodeURIComponent;
const pair = (r) => [r.status, r.body && r.body.error];
const must = (r, what) => {
  assert.ok(r.status < 300, `${what}: ${r.status} ${JSON.stringify(r.body)}`);
  return r;
};

async function send(env, name) {
  const up = must(await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) }), `send ${name}`);
  return up.body.digest;
}
async function release(env, name) {
  const d = await send(env, name);
  return { d, ok: must(await call(env, "bob", "POST", `/orgs/acme/candidates/${d}/approve`), `approve ${name}`) };
}

/* acme (alice owner, bob reviewer, carol developer, dan recipient), erin an
 * admin, frank a recipient; the group pilot holds dan, finance holds frank. */
async function world() {
  const env = orgEnv({ ORG_LEASE_KEY: SEED });
  await acme(env);
  for (const [who, role] of [["erin", "admin"], ["frank", "recipient"]]) {
    const inv = must(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: who, role } }), `invite ${who}`);
    must(await call(env, who, "POST", `/orgs/acme/invites/${inv.body.code}/accept`), `accept ${who}`);
  }
  for (const [g, who] of [["pilot", "dan"], ["finance", "frank"]]) {
    must(await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: g } }), `group ${g}`);
    must(await call(env, "alice", "PUT", `/orgs/acme/groups/${g}/members/${enc(`github:${who}`)}`), `add ${who}`);
  }
  return env;
}

const current = (env, who) => call(env, who, "GET", `/orgs/acme/apps/${enc(APP)}/current`, { headers: { accept: "application/json" } });
const look = (env, who, d) => call(env, who, "GET", `/a/${d}`, { headers: { accept: "application/json" } });
const lease = (env, who, d) => call(env, who, "GET", `/orgs/acme/leases/${d}`);
const sync = (env, who, d) => call(env, who, "POST", `/orgs/acme/data/${d}/sync`, { json: { device: "aaaaaaaaaaaaaaaa", now: Date.now(), since: null, writes: [] } });
const appsOf = async (env, who) => (await call(env, who, "GET", "/orgs/acme/apps")).body.apps.find((a) => a.app_id === APP);
const pilot = (env, who, body) => call(env, who, "PUT", `/orgs/acme/apps/${enc(APP)}/pilot`, { json: body });
const pin = (env, who, group, digest) => call(env, who, "PUT", `/orgs/acme/apps/${enc(APP)}/pins/${group}`, { json: { digest } });
const toEveryone = (env, who) => call(env, who, "POST", `/orgs/acme/apps/${enc(APP)}/pilot/release`);

test("a new version goes to the pilot group first: on the link, download, check-in, lease, data, lists, inventory and Insights", async () => {
  const env = await world();
  await initKey(env);
  const { d: d1 } = await release(env, V1);

  // Only owners and admins choose rings; the group must exist.
  for (const who of ["carol", "bob", "dan"]) assert.deepStrictEqual(pair(await pilot(env, who, { group: "pilot" })), [403, "role-cannot"], who);
  assert.deepStrictEqual(pair(await pilot(env, "alice", { group: "nobody" })), [404, "no-such-group"]);
  assert.deepStrictEqual(pair(await pilot(env, "alice", { group: "Not A Group" })), [400, "group-invalid"]);
  const set = must(await pilot(env, "erin", { group: "pilot" }), "pilot set");
  assert.deepStrictEqual(set.body.rollout.pilot.group, "pilot");
  assert.strictEqual(set.body.rollout.pilot.version, null, "nothing with the pilot yet");

  // 1.0.1 is approved: it goes to the pilot group, everyone else stays on 1.0.0.
  const { d: d2, ok } = await release(env, V2);
  assert.deepStrictEqual(ok.body.pilot, { group: "pilot", people: 1, everyone: { digest: d1, version: "1.0.0", name: "Bounce" } });

  // The app's link.
  const dc = must(await current(env, "dan"), "dan current");
  assert.deepStrictEqual([dc.body.digest, dc.body.ring, dc.body.group], [d2, "pilot", "pilot"]);
  const fc = must(await current(env, "frank"), "frank current");
  assert.deepStrictEqual([fc.body.digest, fc.body.ring], [d1, "everyone"]);
  assert.strictEqual(must(await current(env, "bob"), "bob").body.digest, d1, "a reviewer in no group gets what everyone gets");

  // Download: the pilot's version is refused to anyone outside it, and the
  // refusal says which version is theirs. Reviewers still open it.
  assert.strictEqual((await call(env, "dan", "GET", `/a/${d2}`)).status, 200);
  const fd = await call(env, "frank", "GET", `/a/${d2}`);
  assert.deepStrictEqual(pair(fd), [403, "in-pilot"]);
  assert.match(fd.body.message, /Bounce 1\.0\.1 is with the pilot group in Acme first -- you get it when it is released to everyone/);
  assert.strictEqual(fd.body.for_you.version, "1.0.0");
  assert.strictEqual((await call(env, "bob", "GET", `/a/${d2}`)).status, 200);
  assert.strictEqual((await call(env, "frank", "GET", `/a/${d1}`)).status, 200);

  // The check-in: refused the same way, and it says which version to have.
  assert.deepStrictEqual(pair(await look(env, "frank", d2)), [403, "in-pilot"]);
  const dl = must(await look(env, "dan", d1), "dan checks in on 1.0.0");
  assert.deepStrictEqual([dl.body.for_you.digest, dl.body.for_you.ring], [d2, "pilot"]);
  assert.strictEqual(must(await look(env, "frank", d1), "frank").body.for_you, undefined, "on the right version, nothing to hear");

  // A lease, and its renewal (the same request again).
  assert.strictEqual(must(await lease(env, "dan", d2), "dan lease").body.lease.digest, d2);
  assert.deepStrictEqual(pair(await lease(env, "frank", d2)), [403, "in-pilot"]);
  assert.strictEqual(must(await lease(env, "frank", d1), "frank lease").body.lease.digest, d1);

  // Team data.
  assert.deepStrictEqual(pair(await sync(env, "frank", d2)), [403, "in-pilot"]);
  assert.strictEqual(must(await sync(env, "dan", d2), "dan sync").status, 200);

  // The listing (`krate org apps` reads it): each person's own version.
  const dan = await appsOf(env, "dan");
  assert.deepStrictEqual([dan.current, dan.version, dan.for_you.ring], [d2, "1.0.1", "pilot"]);
  const frank = await appsOf(env, "frank");
  assert.deepStrictEqual([frank.current, frank.version], [d1, "1.0.0"]);
  assert.ok(!frank.releases.some((r) => r.digest === d2), "frank's list has no pilot version");
  assert.strictEqual(frank.rollout, undefined, "the rings are for the people who manage releases");
  const alice = await appsOf(env, "alice");
  assert.strictEqual(alice.current, d1, "the managers' card says what everyone gets");
  assert.deepStrictEqual(
    [alice.rollout.everyone.version, alice.rollout.pilot.group, alice.rollout.pilot.people, alice.rollout.pilot.version.version],
    ["1.0.0", "pilot", 1, "1.0.1"],
  );

  // The releases list.
  assert.deepStrictEqual((await call(env, "frank", "GET", "/orgs/acme/releases")).body.map((r) => r.digest), [d1]);
  const dr = (await call(env, "dan", "GET", "/orgs/acme/releases")).body;
  const said = (d) => dr.filter((r) => r.digest === d).map((r) => [Boolean(r.pilot), r.current])[0];
  assert.deepStrictEqual([dr.length, said(d2), said(d1)], [2, [true, false], [false, true]]);

  // The inventory and Insights say what is current for everyone, and the pilot beside it.
  const inv = (await call(env, "alice", "GET", "/orgs/acme/inventory")).body.apps.find((a) => a.app_id === APP);
  assert.deepStrictEqual([inv.version, inv.rollout.pilot.version.version], ["1.0.0", "1.0.1"]);
  const ins = (await call(env, "carol", "GET", "/orgs/acme/insights")).body.developer.apps.find((a) => a.app_id === APP);
  assert.deepStrictEqual([ins.version, ins.pilot], ["1.0.0", { version: "1.0.1", group: "pilot" }]);
  const store = env.ORGS.object("acme");
  for (const a of store.insightsApps()) assert.strictEqual(a.current ? a.current.digest : null, store.currentOf(a.app_id), a.app_id);

  // Release to everyone: 1.0.1 is current, and the pilot group stays for the next one.
  assert.deepStrictEqual(pair(await toEveryone(env, "carol")), [403, "role-cannot"]);
  const out = must(await toEveryone(env, "alice"), "release to everyone");
  assert.deepStrictEqual([out.body.current.version, out.body.rollout.pilot.group, out.body.rollout.pilot.version], ["1.0.1", "pilot", null]);
  assert.strictEqual(must(await current(env, "frank"), "frank after").body.digest, d2);
  assert.strictEqual((await call(env, "frank", "GET", `/a/${d2}`)).status, 200);
  assert.strictEqual(must(await lease(env, "frank", d2), "frank lease after").body.lease.digest, d2);
  assert.deepStrictEqual(pair(await toEveryone(env, "alice")), [404, "no-pilot"]);

  // Audited.
  const a = (await call(env, "alice", "GET", "/orgs/acme/audit")).body;
  assert.strictEqual(a.chain_ok, true);
  const did = a.events.filter((e) => e.result === "allowed").map((e) => `${e.action} ${e.actor} ${e.reason}`);
  for (const want of [
    "release.pilot github:erin group:pilot",
    `release.pilot github:bob group:pilot ${d2} approved`,
    `release.pilot.release github:alice ${d2}`,
    `release.current github:alice ${d2} from the pilot, after ${d1}`,
  ]) {
    assert.ok(did.includes(want), want);
  }
  assert.ok(a.events.some((e) => e.action === "lease.issue" && e.actor === "github:frank" && e.reason === "in-pilot"));
});

test("a pin keeps a group on one version over the pilot and over current; withdrawing what a ring points to falls back and says so", async () => {
  const env = await world();
  const { d: d1 } = await release(env, V1);
  must(await pilot(env, "alice", { group: "pilot" }), "pilot");
  const { d: d2 } = await release(env, V2);
  must(await toEveryone(env, "alice"), "1.0.1 to everyone");
  const { d: d3 } = await release(env, V3);

  // finance stays on 1.0.0; dan is in the pilot and in finance: the pin wins.
  for (const who of ["carol", "bob"]) assert.deepStrictEqual(pair(await pin(env, who, "finance", d1)), [403, "role-cannot"], who);
  assert.deepStrictEqual(pair(await pin(env, "alice", "finance", "f".repeat(64))), [404, "digest-mismatch"]);
  must(await pin(env, "erin", "finance", d1), "pin finance");
  assert.deepStrictEqual(pair(await call(env, "erin", "PUT", `/orgs/acme/apps/${enc(APP)}/pins/nobody`, { json: { digest: d1 } })), [404, "no-such-group"]);
  must(await call(env, "alice", "PUT", `/orgs/acme/groups/finance/members/${enc("github:dan")}`), "dan to finance");
  const dc = must(await current(env, "dan"), "dan").body;
  assert.deepStrictEqual([dc.digest, dc.ring, dc.group], [d1, "pin", "finance"]);
  assert.deepStrictEqual([(await current(env, "frank")).body.digest, (await current(env, "bob")).body.digest], [d1, d2]);
  // A group pinned to the pilot's own version opens it, though it is not the pilot.
  must(await call(env, "alice", "DELETE", `/orgs/acme/groups/finance/members/${enc("github:dan")}`), "dan out of finance");
  must(await pin(env, "alice", "finance", d3), "finance onto the pilot's version");
  assert.strictEqual(must(await lease(env, "frank", d3), "frank pinned to the pilot").body.lease.digest, d3);
  must(await pin(env, "alice", "finance", d1), "finance back to 1.0.0");
  assert.deepStrictEqual(pair(await lease(env, "frank", d3)), [403, "in-pilot"]);

  const view = (await appsOf(env, "alice")).rollout;
  assert.deepStrictEqual(
    [view.everyone.version, view.pilot.version.version, view.pins.map((p) => [p.group, p.people, p.version.version])],
    ["1.0.1", "1.0.2", [["finance", 1, "1.0.0"]]],
  );

  // Turning the pilot off while a version is with it is refused.
  const off = await call(env, "alice", "DELETE", `/orgs/acme/apps/${enc(APP)}/pilot`);
  assert.deepStrictEqual(pair(off), [409, "pilot-out"]);
  assert.match(off.body.message, /1\.0\.2 is still with the pilot group -- release it to everyone or withdraw it first/);

  // Withdraw the pilot's version: the pilot ends, dan falls back to 1.0.1.
  const w3 = must(await call(env, "bob", "POST", `/orgs/acme/releases/${d3}/withdraw`, { json: { reason: "breaks on save" } }), "withdraw 1.0.2");
  assert.deepStrictEqual(w3.body.fallback, [{ group: "pilot", was: "pilot", now: { digest: d2, version: "1.0.1", name: "Bounce" } }]);
  assert.strictEqual(w3.body.fallback_words, "The pilot group had 1.0.2 as its pilot; it gets 1.0.1 now.");
  assert.deepStrictEqual(w3.body.current, { app_id: APP, digest: d2 }, "everyone stays where they were");
  assert.strictEqual(must(await current(env, "dan"), "dan after").body.digest, d2);

  // Withdraw the pinned version: the pin ends, finance gets what everyone gets.
  const w1 = must(await call(env, "alice", "POST", `/orgs/acme/releases/${d1}/withdraw`, { json: { reason: "old data format" } }), "withdraw 1.0.0");
  assert.strictEqual(w1.body.fallback_words, "The finance group was kept on 1.0.0; it gets 1.0.1 now.");
  assert.strictEqual(must(await current(env, "frank"), "frank after").body.digest, d2);
  assert.deepStrictEqual((await appsOf(env, "alice")).rollout.pins, []);

  // With nothing with the pilot now, it can be turned off.
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/apps/${enc(APP)}/pilot`)).status, 204);

  const did = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.filter((e) => e.result === "allowed").map((e) => `${e.action} ${e.target} ${e.reason}`);
  for (const want of [
    `release.pin app:${APP}/finance ${d1}`,
    `release.pilot app:${APP} ended: ${d3} withdrawn`,
    `release.unpin app:${APP}/finance ${d1} withdrawn`,
    `release.pilot.off app:${APP} group:pilot`,
  ]) {
    assert.ok(did.includes(want), want);
  }
});

test("withdrawing the current version while another is with the pilot falls back to an older one, never to the pilot's", async () => {
  const env = await world();
  const { d: d1 } = await release(env, V1);
  const { d: d2 } = await release(env, V2);
  must(await pilot(env, "alice", { group: "pilot" }), "pilot");
  const { d: d3 } = await release(env, V3);
  const w = must(await call(env, "alice", "POST", `/orgs/acme/releases/${d2}/withdraw`, { json: { reason: "slow" } }), "withdraw current");
  assert.deepStrictEqual(w.body.current, { app_id: APP, digest: d1 });
  assert.deepStrictEqual([(await current(env, "frank")).body.digest, (await current(env, "dan")).body.digest], [d1, d3]);
  // An approved version can be put into the pilot by hand; not the current one.
  assert.deepStrictEqual(pair(await pilot(env, "alice", { group: "pilot", digest: d1 })), [404, "digest-mismatch"]);
});

test("the first version of an app with a pilot group goes to the pilot only", async () => {
  const env = await world();
  must(await pilot(env, "alice", { group: "pilot" }), "pilot before anything is sent");
  const { d: d1 } = await release(env, V1);
  assert.strictEqual(must(await current(env, "dan"), "dan").body.digest, d1);
  const f = await current(env, "frank");
  assert.deepStrictEqual(pair(f), [403, "in-pilot"]);
  assert.match(f.body.message, /Bounce is with the pilot group in Acme first/);
  assert.strictEqual(await appsOf(env, "frank"), undefined, "nothing on frank's list");
  assert.strictEqual((await appsOf(env, "dan")).current, d1);
  must(await toEveryone(env, "alice"), "to everyone");
  assert.strictEqual(must(await current(env, "frank"), "frank").body.digest, d1);
});
