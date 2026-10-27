/*
 * Declining a version with a reason (src/org-decline.js), through the
 * hub's real routes and store: whoever may approve declines, never the
 * version's own sender; only a waiting version; the reason is required
 * and bounded; the row says who, when and why; the audit says it; a
 * declined version is never approved afterwards; and the inventory does
 * not count it as waiting.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-decline.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";

const pair = (r) => [r.status, r.body && r.body.error];

async function waiting(env, name = "candidate-v1.krate") {
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  assert.strictEqual(up.body.state, "waiting-for-approval");
  return up.body.digest;
}
const decline = (env, who, digest, reason) => call(env, who, "POST", `/orgs/acme/candidates/${digest}/decline`, { json: { reason } });
const row = async (env, digest) => (await call(env, "alice", "GET", "/orgs/acme/candidates")).body.find((c) => c.digest === digest);

test("a reviewer declines a waiting version with a reason, and the answer, the row and the audit say who, when and why", async () => {
  const env = orgEnv();
  await acme(env);
  const digest = await waiting(env);
  const before = await row(env, digest);
  assert.deepStrictEqual([before.state, before.declined_by, before.declined_at, before.decline_reason], ["waiting-for-approval", null, null, null], "nothing declined yet, the fields there and empty");

  const r = await decline(env, "bob", digest, "  it asks for the camera  ");
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  assert.deepStrictEqual(Object.keys(r.body).sort(), ["decline_reason", "declined_at", "declined_by", "digest", "state"]);
  assert.strictEqual(r.body.digest, digest);
  assert.strictEqual(r.body.state, "declined");
  assert.strictEqual(r.body.declined_by, "github:bob");
  assert.strictEqual(typeof r.body.declined_at, "number");
  assert.ok(Math.abs(r.body.declined_at - Date.now() / 1000) < 60);
  assert.strictEqual(r.body.decline_reason, "it asks for the camera", "trimmed");

  const after = await row(env, digest);
  assert.deepStrictEqual(
    [after.state, after.declined_by, after.declined_at, after.decline_reason],
    ["declined", "github:bob", r.body.declined_at, "it asks for the camera"],
  );
  // The sender sees it in the list too.
  const carols = (await call(env, "carol", "GET", "/orgs/acme/candidates")).body.find((c) => c.digest === digest);
  assert.strictEqual(carols.decline_reason, "it asks for the camera");

  const events = (await call(env, "alice", "GET", "/orgs/acme/audit")).body;
  assert.strictEqual(events.chain_ok, true);
  const ev = events.events.filter((e) => e.action === "candidate.decline");
  assert.deepStrictEqual(ev.map((e) => [e.actor, e.target, e.result, e.reason]), [["github:bob", `candidate:${digest}`, "allowed", "it asks for the camera"]]);
});

test("the sender never declines their own version, whatever their role, and roles that cannot approve cannot decline", async () => {
  const env = orgEnv();
  await acme(env);
  // carol (developer) sent it.
  const digest = await waiting(env);
  assert.deepStrictEqual(pair(await decline(env, "carol", digest, "changed my mind")), [403, "self-approval"], "its sender");
  assert.deepStrictEqual(pair(await decline(env, "dan", digest, "no")), [403, "role-cannot"], "a recipient");
  assert.deepStrictEqual(pair(await decline(env, "stranger", digest, "no")), [403, "not-a-member"], "a stranger");
  assert.deepStrictEqual(pair(await call(env, null, "POST", `/orgs/acme/candidates/${digest}/decline`, { json: { reason: "x" } })), [401, "not-signed-in"]);
  // An owner sending their own version is refused the same way.
  const own = await call(env, "alice", "POST", "/orgs/acme/candidates", { bytes: fixture("root-signed.krate") });
  assert.strictEqual(own.status, 201, JSON.stringify(own.body));
  assert.deepStrictEqual(pair(await decline(env, "alice", own.body.digest, "mine")), [403, "self-approval"], "an owner, their own upload");
  // An owner may decline someone else's.
  assert.strictEqual((await decline(env, "alice", digest, "not this quarter")).status, 200);
  const denied = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.filter((e) => e.action === "candidate.decline" && e.result === "denied");
  assert.deepStrictEqual(denied.map((e) => [e.actor, e.reason]), [["github:carol", "self-approval"], ["github:dan", "role-cannot"], ["github:stranger", "not-a-member"], ["github:alice", "self-approval"]]);
});

test("the reason is required and at most 500 characters, and only a waiting version is declined", async () => {
  const env = orgEnv();
  await acme(env);
  const digest = await waiting(env);
  for (const [reason, why] of [[undefined, "missing"], ["", "empty"], ["   ", "blank"], ["x".repeat(501), "too long"], [42, "not text"]]) {
    assert.deepStrictEqual(pair(await decline(env, "bob", digest, reason)), [400, "bad-request"], why);
  }
  assert.strictEqual((await row(env, digest)).state, "waiting-for-approval", "nothing refused changed it");
  assert.strictEqual((await decline(env, "bob", digest, "x".repeat(500))).status, 200, "500 is allowed");
  assert.deepStrictEqual(pair(await decline(env, "bob", digest, "again")), [409, "declined"], "declined twice");

  const other = await waiting(env, "candidate-v2.krate");
  // v2 replaces nothing released, so it is approved as it is.
  const ok = await call(env, "bob", "POST", `/orgs/acme/candidates/${other}/approve`, { json: { accept: [] } });
  assert.strictEqual(ok.status, 200, JSON.stringify(ok.body));
  assert.deepStrictEqual(pair(await decline(env, "bob", other, "too late")), [409, "not-waiting"], "a release is not declined");
  assert.deepStrictEqual(pair(await decline(env, "bob", "a".repeat(64), "nothing")), [404, "digest-mismatch"]);
  assert.deepStrictEqual(pair(await call(env, "bob", "POST", "/orgs/acme/candidates/nothex/decline", { json: { reason: "x" } })), [404, "digest-mismatch"]);
});

test("a declined version is never approved, the same bytes again stay declined, and the inventory does not call it waiting", async () => {
  const env = orgEnv();
  await acme(env);
  const digest = await waiting(env);
  assert.strictEqual((await decline(env, "bob", digest, "it asks for the camera")).status, 200);
  for (const who of ["bob", "alice"]) {
    const r = await call(env, who, "POST", `/orgs/acme/candidates/${digest}/approve`, { json: { accept: [] } });
    assert.deepStrictEqual(pair(r), [409, "declined"], `${who} approving it`);
  }
  assert.deepStrictEqual(pair(await call(env, "carol", "POST", `/orgs/acme/candidates/${digest}/approve`)), [409, "declined"], "its sender too");
  assert.strictEqual((await row(env, digest)).state, "declined");
  // Nobody can open it.
  assert.deepStrictEqual(pair(await call(env, "dan", "GET", `/a/${digest}`, { headers: { accept: "application/json" } })), [403, "digest-mismatch"]);
  // Sending the same bytes again is the version they already are.
  const again = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.deepStrictEqual([again.status, again.body.state], [200, "declined"]);

  const inv = await call(env, "alice", "GET", "/orgs/acme/inventory");
  assert.strictEqual(inv.status, 200, JSON.stringify(inv.body));
  const app = inv.body.apps.find((a) => a.digest === digest);
  assert.ok(app, "the app is in the inventory");
  assert.notStrictEqual(app.state, "waiting", "a declined version is not waiting");
  assert.notStrictEqual(app.state, "released");
  const releases = (await call(env, "dan", "GET", "/orgs/acme/releases")).body;
  assert.ok(!releases.some((r) => r.digest === digest), "not a release");
});
