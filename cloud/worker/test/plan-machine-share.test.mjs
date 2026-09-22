/* A wall the person did not walk into has to say why (K-807).
 *
 * The free app is bound to the MACHINE as well as the account: the device
 * hash is the second key, because an account is free to create and an
 * account-only limit is an invitation to make another email. That is
 * deliberate and it stays.
 *
 * What was missing is the explanation. Somebody on a second-hand or shared
 * Mac signs in, has made nothing, and their one free app is already gone.
 * The hub answered with a bare count, so no screen could tell them why --
 * the refusal read as a bug, on the screen where somebody decides whether
 * to trust us with money.
 *
 * `machine` is that explanation: how much of the count this DEVICE already
 * carried, regardless of who is signed in. Reported, never enforced -- the
 * wall is still `n`, and a client ignoring the field behaves as before.
 *
 * Driven against the REAL handler rather than a mirrored copy, because the
 * thing under test is what the worker actually returns. The neighbouring
 * plan-count.test.mjs mirrors the logic on purpose; this one cannot, since
 * a mirror would assert my own arithmetic back at me.
 *
 *   node --experimental-wasm-modules cloud/worker/test/plan-machine-share.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";

const DEV = "a".repeat(64);
const OTHER_DEV = "b".repeat(64);

function env() {
  const kv = new Map([
    ["session:krs_alice", "u-alice"],
    ["user:u-alice", JSON.stringify({ id: "u-alice", login: "alice" })],
    ["session:krs_bob", "u-bob"],
    ["user:u-bob", JSON.stringify({ id: "u-bob", login: "bob" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix, limit }) => ({
        keys: [...kv.keys()]
          .filter((k) => k.startsWith(prefix))
          .slice(0, limit || 1000)
          .map((name) => ({ name })),
        list_complete: true,
      }),
    },
    PUBLIC_BASE: "https://hub.krate.tech",
  };
}

async function plan(e, { token, device, count = false }) {
  const headers = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(
    new Request(`https://hub.krate.tech/plan/${count ? "count" : "get"}`, {
      method: "POST",
      headers,
      body: JSON.stringify({ device, n: 0 }),
    }),
    e,
  );
  assert.strictEqual(res.status, 200, await res.clone().text());
  return JSON.parse(await res.text());
}

// ---- nobody has used this machine ---------------------------------------
{
  const e = env();
  const fresh = await plan(e, { token: "krs_alice", device: DEV });
  assert.strictEqual(fresh.n, 0, "a new account on a new machine owes nothing");
  assert.strictEqual(
    fresh.machine,
    0,
    "and there is nothing to explain, so the screen says what it always said",
  );
  console.log("ok  a genuinely fresh start reports nothing to explain");
}

// ---- the machine spent it, not the person -------------------------------
{
  const e = env();
  // Alice makes her free app on this machine.
  const made = await plan(e, { token: "krs_alice", device: DEV, count: true });
  assert.strictEqual(made.n, 1, "alice used her one free app");

  // Bob buys the machine second-hand, or borrows it, and signs in. He has
  // made nothing, anywhere.
  const bob = await plan(e, { token: "krs_bob", device: DEV });
  assert.strictEqual(bob.n, 1, "the wall still holds -- this is not a way in");
  assert.strictEqual(
    bob.machine,
    1,
    "and it says the MACHINE spent it, which is the only fact that explains \
a free app gone before he made anything",
  );

  // The same Bob on his own machine is untouched.
  const elsewhere = await plan(e, { token: "krs_bob", device: OTHER_DEV });
  assert.strictEqual(elsewhere.n, 0, "bob's own machine is his own");
  assert.strictEqual(elsewhere.machine, 0, "with nothing to explain");
  console.log("ok  a second-hand machine says so, and does not leak an app");
}

// ---- the person spent it themselves -------------------------------------
{
  const e = env();
  await plan(e, { token: "krs_alice", device: DEV, count: true });
  // Alice clears her browser storage, so the device id changes, but she is
  // still Alice and still owes her make.
  const same = await plan(e, { token: "krs_alice", device: OTHER_DEV });
  assert.strictEqual(same.n, 1, "the account carries the count between machines");
  assert.strictEqual(
    same.machine,
    0,
    "but this machine spent nothing, so telling her it did would be a lie",
  );
  console.log("ok  a person who used their own app is not told a machine did");
}

// ---- an app made through the funded-case ledger --------------------------
//
// The live path a real make takes. `caseAttempt` mirrors every consumption
// into the legacy counters, so the counter alone would in fact answer this
// -- checked, not assumed, and the check below states it rather than
// leaving a reader to wonder why this case is here.
//
// It earns its place anyway: it is the route people actually use, and the
// earlier cases all reach the wall through /plan/count. If the mirroring
// is ever dropped, the counter goes quiet and the cases are what is left,
// which is why deviceOnlyCount reads both.
{
  const e = env();
  const open = await worker.fetch(
    new Request("https://hub.krate.tech/case/open", {
      method: "POST",
      headers: { "content-type": "application/json", authorization: "Bearer krs_alice" },
      body: JSON.stringify({ device: DEV, request: "a habit tracker" }),
    }),
    e,
  );
  assert.strictEqual(open.status, 200, await open.clone().text());
  const { id } = JSON.parse(await open.text());

  const attempt = await worker.fetch(
    new Request("https://hub.krate.tech/case/attempt", {
      method: "POST",
      headers: { "content-type": "application/json", authorization: "Bearer krs_alice" },
      body: JSON.stringify({ device: DEV, id, outcome: "made" }),
    }),
    e,
  );
  assert.strictEqual(attempt.status, 200, await attempt.clone().text());

  // caseAttempt mirrors a consumption into the legacy counters, so both
  // halves of the answer agree here. Stated because the obvious guess is
  // that the ledger path leaves the counter alone, and it does not.
  assert.strictEqual(
    await e.APPS.get(`mkdev:${DEV}`),
    "1",
    "a made case mirrors into the device counter",
  );
  const cases = await e.APPS.list({ prefix: `case:dev:${DEV}:` });
  assert.strictEqual(
    cases.keys.length,
    1,
    "and the case itself is stored under this device, which is the other \
half deviceOnlyCount reads",
  );

  const bob = await plan(e, { token: "krs_bob", device: DEV });
  assert.strictEqual(bob.n, 1, "the case holds the wall");
  assert.strictEqual(
    bob.machine,
    1,
    "and the machine's share is read from the cases it carries, not only \
from the counter",
  );
  console.log("ok  an app made through the ledger still explains the wall");
}

// ---- an anonymous caller ------------------------------------------------
{
  const e = env();
  await plan(e, { token: "krs_alice", device: DEV, count: true });
  const anon = await plan(e, { device: DEV });
  assert.strictEqual(anon.n, 1, "the device alone still holds the wall");
  assert.strictEqual(anon.machine, 1, "and can still explain it without an account");
  console.log("ok  the explanation does not require being signed in");
}

console.log("ok  the count says how much of it belongs to the machine");
