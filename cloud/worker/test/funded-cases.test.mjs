/* The funded-case ledger (IC-414): the allowance is cases, not counter bumps.
 *
 * Drives the REAL worker -- `import worker from "../src/index.js"` and
 * worker.fetch() against a stubbed KV -- not a mirror of its logic. The old
 * mechanism was a pair of bare integers that could not tell a retry from a
 * new app, could not refund a failure that was our fault, and lost one of
 * every two simultaneous increments to a read-modify-write race.
 *
 *   node cloud/worker/test/funded-cases.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";

/* ---- a KV that remembers ------------------------------------------------- */
const KV = new Map();
const env = {
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix, limit = 1000 }) => ({
      keys: [...KV.keys()]
        .filter((k) => k.startsWith(prefix))
        .slice(0, limit)
        .map((name) => ({ name })),
    }),
  },
};

// Two signed-in people.
KV.set("session:krs_alice", "u-alice");
KV.set("user:u-alice", JSON.stringify({ id: "u-alice", login: "alice" }));
KV.set("session:krs_bob", "u-bob");
KV.set("user:u-bob", JSON.stringify({ id: "u-bob", login: "bob" }));

const DEV_A = "a".repeat(64);
const DEV_B = "b".repeat(64);

async function call(path, body, token) {
  const headers = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(
    new Request(`https://hub.test${path}`, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
    }),
    env,
    { waitUntil() {} },
  );
  const text = await res.text();
  let parsed = null;
  try { parsed = JSON.parse(text); } catch (e) { /* plain text */ }
  return { status: res.status, body: parsed, text };
}

/* ---- first case ---------------------------------------------------------- */
const first = await call("/case/open", { device: DEV_A, request: "a notes app" }, "krs_alice");
assert.strictEqual(first.status, 200, first.text);
const caseId = first.body.id;
assert.match(caseId, /^[a-f0-9]{16}$/, "an opaque case id");
assert.strictEqual(first.body.n, 0, "opening consumes nothing");

/* ---- failed attempts are free ------------------------------------------- */
for (const outcome of ["provider-failed", "infra-failed", "krate-failed"]) {
  const failed = await call("/case/attempt", { device: DEV_A, id: caseId, outcome }, "krs_alice");
  assert.strictEqual(failed.status, 200, failed.text);
  assert.strictEqual(failed.body.made, false, `${outcome} must not consume the case`);
}
let list = (await call("/case/list", { device: DEV_A }, "krs_alice")).body;
assert.strictEqual(list.n, 0, "three failures that were our fault cost nothing");

/* ---- retry, then the file ------------------------------------------------ */
const made = await call("/case/attempt", { device: DEV_A, id: caseId, outcome: "made" }, "krs_alice");
assert.strictEqual(made.body.made, true);
list = (await call("/case/list", { device: DEV_A }, "krs_alice")).body;
assert.strictEqual(list.n, 1, "a produced file consumes exactly one case");

/* ---- in-scope revision and the wrong file -------------------------------- */
for (const outcome of ["revision", "not-as-asked", "revision"]) {
  await call("/case/attempt", { device: DEV_A, id: caseId, outcome }, "krs_alice");
}
list = (await call("/case/list", { device: DEV_A }, "krs_alice")).body;
assert.strictEqual(list.n, 1, "revising the same app is inside the same case");

/* ---- ownership ----------------------------------------------------------- */
const foreign = await call("/case/attempt", { device: DEV_B, id: caseId, outcome: "made" }, "krs_bob");
assert.strictEqual(foreign.status, 404, "someone else's case does not exist for you");

/* ---- acceptance closes the case ------------------------------------------ */
const accepted = await call("/case/close", { device: DEV_A, id: caseId, verdict: "accepted" }, "krs_alice");
assert.strictEqual(accepted.body.state, "accepted");
const afterClose = await call("/case/attempt", { device: DEV_A, id: caseId, outcome: "revision" }, "krs_alice");
assert.strictEqual(afterClose.status, 409, "new work after acceptance is a new case");

// Closing again with the same verdict changes nothing and does not error.
const again = await call("/case/close", { device: DEV_A, id: caseId, verdict: "abandoned" }, "krs_alice");
assert.strictEqual(again.body.state, "accepted", "a verdict is history, not a setting");

/* ---- abandonment refunds only what was never made ------------------------ */
const doomed = await call("/case/open", { device: DEV_A, request: "a game" }, "krs_alice");
await call("/case/attempt", { device: DEV_A, id: doomed.body.id, outcome: "provider-failed" }, "krs_alice");
await call("/case/close", { device: DEV_A, id: doomed.body.id, verdict: "abandoned" }, "krs_alice");
list = (await call("/case/list", { device: DEV_A }, "krs_alice")).body;
assert.strictEqual(list.n, 1, "an abandoned case that never made a file never cost anything");

/* ---- multi-device race --------------------------------------------------- */
// Two devices, one account, opening and making at the same time. The old
// counters lost one of these to read-modify-write; the ledger keeps both.
const [raceA, raceB] = await Promise.all([
  call("/case/open", { device: DEV_A, request: "app one" }, "krs_alice"),
  call("/case/open", { device: DEV_B, request: "app two" }, "krs_alice"),
]);
await Promise.all([
  call("/case/attempt", { device: DEV_A, id: raceA.body.id, outcome: "made" }, "krs_alice"),
  call("/case/attempt", { device: DEV_B, id: raceB.body.id, outcome: "made" }, "krs_alice"),
]);
list = (await call("/case/list", { device: DEV_A }, "krs_alice")).body;
assert.strictEqual(list.n, 3, "simultaneous makes are BOTH recorded -- no lost update");

/* ---- the wall ------------------------------------------------------------ */
const fourth = await call("/case/open", { device: DEV_A, request: "one more" }, "krs_alice");
assert.strictEqual(fourth.status, 402, "the fourth free app is refused");
assert.strictEqual(fourth.body.wall, true);

/* ---- the legacy route is the same ledger --------------------------------- */
// Studio and the builder still speak /plan/count; it must count cases now.
const legacyN = (await call("/plan/get", { device: DEV_A }, "krs_alice")).body.n;
assert.strictEqual(legacyN, 3, "/plan/get reads the ledger");

/* ---- migration ----------------------------------------------------------- */
// Bob made two apps before the ledger existed: his counter says 2, his
// ledger is empty. The first read mints the difference as readable records.
// His own machine: DEV_B belongs to the race test above, and a device
// shared between accounts pools its allowance BY DESIGN ("a new account on
// the same machine gets none") -- reusing it here would show him alice's
// race case, which is the two-key rule working, not migration failing.
const BOB_DEV = "d".repeat(64);
KV.set("mkacct:u-bob", "2");
const bob = (await call("/case/list", { device: BOB_DEV }, "krs_bob")).body;
assert.strictEqual(bob.n, 2, "the counter's spend survives as cases");
assert.strictEqual(bob.cases.length, 2);
assert.ok(
  bob.cases.every((c) => c.attempts[0].note === "migrated from the counter"),
  "every migrated slot says where it came from",
);
// And reading again does not mint again.
const bobAgain = (await call("/case/list", { device: BOB_DEV }, "krs_bob")).body;
assert.strictEqual(bobAgain.cases.length, 2, "migration is once, not per read");

/* ---- offline makes mirror as cases --------------------------------------- */
const carolDev = "c".repeat(64);
const offline = (await call("/plan/get", { device: carolDev, n: 2 }, null)).body;
assert.strictEqual(offline.n, 2, "offline makes are honoured");
const carol = (await call("/case/list", { device: carolDev }, null)).body;
assert.strictEqual(carol.cases.length, 2, "and each one is a readable record");
assert.ok(carol.cases.every((c) => c.attempts[0].note === "made offline, mirrored later"));

/* ---- no identity, no ledger ---------------------------------------------- */
const nobody = await call("/case/open", { request: "an app" }, null);
assert.strictEqual(nobody.status, 400, "no account and no device is nobody");

console.log("OK -- cases fund attempts, failures are free, races lose nothing, and old counters become readable records");
