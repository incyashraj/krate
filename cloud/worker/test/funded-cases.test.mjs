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

// A READ reports the spend and writes NOTHING. Minting on a read put KV
// puts on Studio's per-session status check -- on the namespace publishes
// and sign-ins share -- and a dropped put (quota gone) meant the next read
// minted again, unbounded, exactly when the budget was exhausted. That is
// how counting took the product down on 2026-08-10.
const writesBefore = KV.size;
const bob = (await call("/case/list", { device: BOB_DEV }, "krs_bob")).body;
assert.strictEqual(bob.n, 2, "the counter's spend is reported on a read");
assert.strictEqual(bob.cases.length, 0, "a read materialises nothing");
assert.strictEqual(KV.size, writesBefore, "a read writes no KV keys at all");

// Reading again is still free, and still reports the same number.
const bobAgain = (await call("/case/list", { device: BOB_DEV }, "krs_bob")).body;
assert.strictEqual(bobAgain.n, 2, "a repeat read is stable");
assert.strictEqual(KV.size, writesBefore, "and still writes nothing");

// The wall reads the same number, so a migrated user is gated correctly
// without a single write having happened.
const kvBeforePlanGet = KV.size;
const bobPlan = (await call("/plan/get", { device: BOB_DEV }, "krs_bob")).body;
assert.strictEqual(bobPlan.n, 2, "/plan/get agrees on a read");
// The one that matters most: Studio calls /plan/get on EVERY session, so a
// write here is a write per session forever. This assertion is why the
// read/write split exists -- without it the sabotage (write: true) passed.
assert.strictEqual(
  KV.size, kvBeforePlanGet,
  "/plan/get must write NOTHING -- it runs on every Studio session, on the "
  + "namespace publishes and sign-ins share",
);

// A call that was ALREADY writing materialises them, once, with their
// history intact.
await call("/plan/count", { device: BOB_DEV }, "krs_bob");
const bobWritten = (await call("/case/list", { device: BOB_DEV }, "krs_bob")).body;
assert.strictEqual(bobWritten.n, 3, "the write counted, on top of the migrated two");
assert.strictEqual(bobWritten.cases.length, 3, "and now the records exist");
assert.strictEqual(
  bobWritten.cases.filter((c) => c.attempts[0].note === "migrated from the counter").length,
  2,
  "the two migrated slots say where they came from",
);

/* ---- offline makes mirror as cases --------------------------------------- */
const carolDev = "c".repeat(64);
// A read honours the client's number without writing for it: Studio sends
// its local count on every session sync, and minting there turned one
// status check into up to three KV puts, forever.
const kvBeforeOffline = KV.size;
const offline = (await call("/plan/get", { device: carolDev, n: 2 }, null)).body;
assert.strictEqual(offline.n, 2, "offline makes are honoured on a read");
assert.strictEqual(KV.size, kvBeforeOffline, "and cost no writes");

// The next call that writes anyway materialises them, capped at the free
// allowance so a client claiming a thousand offline makes cannot make us
// write a thousand records.
await call("/plan/count", { device: carolDev, n: 2 }, null);
const carol = (await call("/case/list", { device: carolDev }, null)).body;
assert.ok(
  carol.cases.some((c) => c.attempts[0].note === "made offline, mirrored later"),
  `the offline makes became readable records: ${JSON.stringify(carol.cases)}`,
);

const greedy = "e".repeat(64);
await call("/plan/count", { device: greedy, n: 1000 }, null);
const capped = (await call("/case/list", { device: greedy }, null)).body;
assert.ok(
  capped.cases.length <= 4,
  `a huge claimed count cannot mint unbounded records, got ${capped.cases.length}`,
);

/* ---- no identity, no ledger ---------------------------------------------- */
const nobody = await call("/case/open", { request: "an app" }, null);
assert.strictEqual(nobody.status, 400, "no account and no device is nobody");

console.log("OK -- cases fund attempts, failures are free, races lose nothing, and old counters become readable records");
