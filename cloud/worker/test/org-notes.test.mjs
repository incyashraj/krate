/*
 * Release notes (src/org-notes.js), through the hub's real routes and
 * store: the sender (or an owner) writes the note while the version waits
 * or once it is released; nobody else; at most 1000 characters, empty
 * clears; a note can come with the upload in x-krate-note; and every list
 * that shows a version carries it -- /candidates, /releases, /apps.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-notes.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";

const pair = (r) => [r.status, r.body && r.body.error];
const setNote = (env, who, digest, note) => call(env, who, "PUT", `/orgs/acme/candidates/${digest}/note`, { json: { note } });
const candidate = async (env, digest) => (await call(env, "alice", "GET", "/orgs/acme/candidates")).body.find((c) => c.digest === digest);

async function send(env, headers = {}) {
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate"), headers });
  return up;
}

test("the sender writes the note while it waits and after release, and every list carries it", async () => {
  const env = orgEnv();
  await acme(env);
  const up = await send(env);
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  const digest = up.body.digest;
  assert.strictEqual((await candidate(env, digest)).note, null, "none yet: null, not missing");

  const set = await setNote(env, "carol", digest, "  Fixes the totals on the summary page.  ");
  assert.strictEqual(set.status, 200, JSON.stringify(set.body));
  assert.deepStrictEqual(set.body, { digest, note: "Fixes the totals on the summary page." });
  assert.strictEqual((await candidate(env, digest)).note, "Fixes the totals on the summary page.");
  // The approver sees it in the list they approve from.
  assert.strictEqual((await call(env, "bob", "GET", "/orgs/acme/candidates")).body.find((c) => c.digest === digest).note, "Fixes the totals on the summary page.");

  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${digest}/approve`)).status, 200);
  const releases = (await call(env, "dan", "GET", "/orgs/acme/releases")).body;
  assert.strictEqual(releases.find((r) => r.digest === digest).note, "Fixes the totals on the summary page.", "/releases");
  const apps = (await call(env, "dan", "GET", "/orgs/acme/apps")).body.apps;
  const release = apps.flatMap((a) => a.releases).find((r) => r.digest === digest);
  assert.strictEqual(release.note, "Fixes the totals on the summary page.", "/apps release entries");

  // After release: still the sender's to change; empty clears.
  assert.strictEqual((await setNote(env, "carol", digest, "Totals fixed; faster export.")).status, 200);
  assert.strictEqual((await call(env, "dan", "GET", "/orgs/acme/releases")).body.find((r) => r.digest === digest).note, "Totals fixed; faster export.");
  const cleared = await setNote(env, "carol", digest, "");
  assert.deepStrictEqual([cleared.status, cleared.body.note], [200, null]);
  assert.strictEqual((await call(env, "dan", "GET", "/orgs/acme/apps")).body.apps.flatMap((a) => a.releases).find((r) => r.digest === digest).note, null);

  const events = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.filter((e) => e.action === "candidate.note");
  assert.deepStrictEqual(events.map((e) => [e.actor, e.target, e.result, e.reason]), [
    ["github:carol", `candidate:${digest}`, "allowed", "set"],
    ["github:carol", `candidate:${digest}`, "allowed", "set"],
    ["github:carol", `candidate:${digest}`, "allowed", "cleared"],
  ]);
});

test("only the sender or an owner writes it, at most 1000 characters, and never on a declined version", async () => {
  const env = orgEnv();
  await acme(env);
  const digest = (await send(env)).body.digest;
  assert.deepStrictEqual(pair(await setNote(env, "bob", digest, "from the reviewer")), [403, "role-cannot"], "a reviewer who did not send it");
  assert.deepStrictEqual(pair(await setNote(env, "dan", digest, "from a recipient")), [403, "role-cannot"]);
  assert.deepStrictEqual(pair(await setNote(env, "stranger", digest, "x")), [403, "not-a-member"]);
  assert.strictEqual((await setNote(env, "alice", digest, "An owner's word.")).status, 200, "an owner may");
  assert.deepStrictEqual(pair(await setNote(env, "carol", digest, "x".repeat(1001))), [400, "bad-request"], "too long");
  assert.deepStrictEqual(pair(await setNote(env, "carol", digest, 7)), [400, "bad-request"], "not text");
  assert.strictEqual((await setNote(env, "carol", digest, "x".repeat(1000))).status, 200, "1000 is allowed");
  assert.deepStrictEqual(pair(await setNote(env, "carol", "b".repeat(64), "x")), [404, "digest-mismatch"]);
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${digest}/decline`, { json: { reason: "not now" } })).status, 200);
  assert.deepStrictEqual(pair(await setNote(env, "carol", digest, "after the decline")), [409, "note-closed"]);
  assert.strictEqual((await candidate(env, digest)).note, "x".repeat(1000), "the note it had");
  const denied = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.filter((e) => e.action === "candidate.note" && e.result === "denied");
  assert.deepStrictEqual(denied.map((e) => [e.actor, e.reason]), [["github:bob", "role-cannot"], ["github:dan", "role-cannot"], ["github:stranger", "not-a-member"], ["github:carol", "digest-mismatch"], ["github:carol", "note-closed"]]);
});

test("a note comes with the upload in x-krate-note, URL-encoded UTF-8, under the same limit", async () => {
  const env = orgEnv();
  await acme(env);
  const words = "Mom’s budget: totals fixed — 番茄钟";
  assert.deepStrictEqual(pair(await send(env, { "x-krate-note": encodeURIComponent("x".repeat(1001)) })), [400, "bad-request"], "too long");
  assert.deepStrictEqual(pair(await send(env, { "x-krate-note": "%E0%A4%A" })), [400, "bad-request"], "not URL-encoded");
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/candidates")).body.length, 0, "nothing refused was kept");
  const up = await send(env, { "x-krate-note": encodeURIComponent(words) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  assert.strictEqual(up.body.note, words, "the answer says it");
  assert.strictEqual((await candidate(env, up.body.digest)).note, words);
});
