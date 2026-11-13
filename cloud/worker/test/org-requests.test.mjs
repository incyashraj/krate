/*
 * App requests (src/org-requests.js), through the hub's real routes and
 * store: any member asks; owners, admins and developers see every request
 * and answer them; reviewers and recipients see only their own; the limits
 * on the text, the reply and open requests per person; the shapes field by
 * field; and the audit.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-requests.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme } from "./org-tools.mjs";
import { MATRIX } from "../src/org.js";

const pair = (r) => [r.status, r.body && r.body.error];
const ask = (env, who, text) => call(env, who, "POST", "/orgs/acme/requests", { json: { text } });
const list = (env, who) => call(env, who, "GET", "/orgs/acme/requests");
const answer = (env, who, id, body) => call(env, who, "PUT", `/orgs/acme/requests/${id}`, { json: body });

/* acme, plus erin as an admin. */
async function world() {
  const env = orgEnv();
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "admin" } });
  assert.strictEqual((await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`)).status, 200);
  return env;
}

test("a member asks for an app, and the request has every field the page reads", async () => {
  const env = await world();
  const r = await ask(env, "dan", "  A visitor log for the front desk  ");
  assert.strictEqual(r.status, 201, JSON.stringify(r.body));
  const q = r.body.request;
  assert.deepStrictEqual(Object.keys(q).sort(), ["at", "by", "id", "replied_at", "replied_by", "reply", "state", "text"]);
  assert.match(q.id, /^rq_[a-z2-9]{12}$/);
  assert.deepStrictEqual([q.text, q.by, q.state, q.reply, q.replied_by, q.replied_at], ["A visitor log for the front desk", "github:dan", "open", null, null, null]);
  assert.ok(Math.abs(q.at - Date.now() / 1000) < 60);
  // Every role may ask.
  for (const who of ["alice", "erin", "bob", "carol"]) assert.strictEqual((await ask(env, who, `${who} needs a timesheet app`)).status, 201, who);
  const events = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.filter((e) => e.action === "request.create");
  assert.deepStrictEqual(events.map((e) => e.actor), ["github:dan", "github:alice", "github:erin", "github:bob", "github:carol"]);
  assert.strictEqual(events[0].target, `request:${q.id}`);
});

test("owners, admins and developers see every request, newest first; reviewers and recipients only their own", async () => {
  const env = await world();
  const at = (s) => (env.ORGS.object("acme").now = () => s);
  at(1000);
  const d = (await ask(env, "dan", "A visitor log, please")).body.request;
  at(2000);
  const b = (await ask(env, "bob", "A review checklist app")).body.request;
  at(3000);
  const c = (await ask(env, "carol", "A deploy calendar")).body.request;
  for (const who of ["alice", "erin", "carol"]) {
    const r = await list(env, who);
    assert.strictEqual(r.status, 200, who);
    assert.deepStrictEqual(r.body.requests.map((x) => x.id), [c.id, b.id, d.id], `${who} sees all, newest first`);
  }
  assert.deepStrictEqual((await list(env, "dan")).body.requests.map((x) => x.id), [d.id], "a recipient sees their own");
  assert.deepStrictEqual((await list(env, "bob")).body.requests.map((x) => x.id), [b.id], "a reviewer sees their own");
  assert.deepStrictEqual(pair(await list(env, "stranger")), [403, "not-a-member"]);
  assert.deepStrictEqual(pair(await list(env, null)), [401, "not-signed-in"]);
  assert.deepStrictEqual(MATRIX["request.view"], { owner: "yes", admin: "yes", developer: "yes", reviewer: "own", recipient: "own" });
});

test("owners, admins and developers answer; nobody else does; states and replies are checked", async () => {
  const env = await world();
  const q = (await ask(env, "dan", "A visitor log, please")).body.request;
  assert.deepStrictEqual(pair(await answer(env, "dan", q.id, { state: "done" })), [403, "role-cannot"], "the asker, a recipient");
  assert.deepStrictEqual(pair(await answer(env, "bob", q.id, { state: "planned" })), [403, "role-cannot"], "a reviewer");
  assert.deepStrictEqual(pair(await answer(env, "carol", q.id, { state: "maybe" })), [400, "bad-request"], "not a state");
  assert.deepStrictEqual(pair(await answer(env, "carol", q.id, { state: "planned", reply: "x".repeat(1001) })), [400, "bad-request"], "reply too long");
  assert.deepStrictEqual(pair(await answer(env, "carol", "rq_nope", { state: "planned" })), [404, "no-such-request"]);

  const p = await answer(env, "carol", q.id, { state: "planned", reply: "On it this week." });
  assert.strictEqual(p.status, 200, JSON.stringify(p.body));
  const r = p.body.request;
  assert.deepStrictEqual([r.id, r.text, r.by, r.at, r.state, r.reply, r.replied_by], [q.id, q.text, "github:dan", q.at, "planned", "On it this week.", "github:carol"]);
  assert.ok(Math.abs(r.replied_at - Date.now() / 1000) < 60);
  // The asker reads the answer.
  assert.deepStrictEqual((await list(env, "dan")).body.requests.map((x) => [x.state, x.reply]), [["planned", "On it this week."]]);
  for (const [who, state] of [["erin", "done"], ["alice", "declined"], ["alice", "open"]]) {
    const s = await answer(env, who, q.id, { state });
    assert.deepStrictEqual([s.status, s.body.request.state, s.body.request.replied_by, s.body.request.reply], [200, state, `github:${who}`, "On it this week."], `${who} -> ${state}`);
  }
  const cleared = await answer(env, "carol", q.id, { state: "done", reply: "" });
  assert.deepStrictEqual([cleared.body.request.state, cleared.body.request.reply], ["done", null], "an empty reply clears it");

  const events = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.filter((e) => e.action === "request.update" || e.action === "request.manage");
  assert.deepStrictEqual(events.map((e) => [e.actor, e.action, e.result, e.reason]), [
    ["github:dan", "request.manage", "denied", "role-cannot"],
    ["github:bob", "request.manage", "denied", "role-cannot"],
    ["github:carol", "request.manage", "denied", "no-such-request"],
    ["github:carol", "request.update", "allowed", "planned, replied"],
    ["github:erin", "request.update", "allowed", "done"],
    ["github:alice", "request.update", "allowed", "declined"],
    ["github:alice", "request.update", "allowed", "open"],
    ["github:carol", "request.update", "allowed", "done, replied"],
  ]);
});

test("a request is 5 to 1000 characters, and a person has at most 20 open", async () => {
  const env = await world();
  for (const [text, why] of [["", "empty"], ["four", "four characters"], ["    hi    ", "short once trimmed"], ["x".repeat(1001), "too long"], [12345, "not text"], [undefined, "missing"]]) {
    assert.deepStrictEqual(pair(await ask(env, "dan", text)), [400, "bad-request"], why);
  }
  assert.strictEqual((await ask(env, "dan", "x".repeat(1000))).status, 201, "1000 is allowed");
  assert.strictEqual((await ask(env, "dan", "five!")).status, 201, "five is allowed");
  for (let i = 2; i < 20; i++) assert.strictEqual((await ask(env, "dan", `request number ${i}`)).status, 201, `request ${i + 1}`);
  assert.deepStrictEqual(pair(await ask(env, "dan", "one too many")), [409, "requests-full"], "the 21st open request");
  assert.strictEqual((await ask(env, "carol", "someone else may still ask")).status, 201);
  // Answering one makes room.
  const first = (await list(env, "dan")).body.requests.at(-1);
  assert.strictEqual((await answer(env, "carol", first.id, { state: "done" })).status, 200);
  assert.strictEqual((await ask(env, "dan", "now there is room")).status, 201);
  assert.strictEqual((await list(env, "dan")).body.requests.length, 21);
});
