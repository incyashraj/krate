/* A failed build must never eat somebody's one free app (IC-001, K-XXX).
 *
 * This is the single most damaging thing that can go wrong when strangers
 * arrive with the API key switched on: a person types one sentence, OUR
 * provider 500s or OUR machine times out, and the one app they were
 * promised is gone. They cannot tell the difference between "Krate broke"
 * and "Krate lied", and there is no second chance to find out.
 *
 * `funded-cases.test.mjs` proves each outcome's `made` flag in isolation.
 * That is not the same claim. What matters is the whole sequence a real
 * failing build walks -- open, attempt, and then the NEXT open, which is
 * the door the person actually meets -- and that the wall is still open
 * at the end of it. Nothing joined those two halves, so a change that
 * made the wall count attempts rather than makes would pass every
 * existing test.
 *
 * Drives the REAL worker, replaying the exact call sequence
 * cloud/builder/src/server.js makes: `/case/open` at startBuild, then
 * `/case/attempt` with the outcome its `proc.on("close")` chose.
 *
 *   node --experimental-wasm-modules cloud/worker/test/failure-keeps-the-free-app.test.mjs
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

KV.set("session:krs_dana", "u-dana");
KV.set("user:u-dana", JSON.stringify({ id: "u-dana", login: "dana" }));
KV.set("session:krs_erin", "u-erin");
KV.set("user:u-erin", JSON.stringify({ id: "u-erin", login: "erin" }));

const DEV_D = "d".repeat(64);
const DEV_E = "e".repeat(64);

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

/* One build, exactly as the builder runs it: open a case, then record the
 * outcome its close handler picked. Returns the case id so a test can ask
 * the ledger about that build by name rather than by position. */
async function build(device, token, request, outcome) {
  const opened = await call("/case/open", { device, request }, token);
  if (opened.status !== 200) return { opened, id: null };
  await call("/case/attempt", { device, id: opened.body.id, outcome }, token);
  return { opened, id: opened.body.id };
}

/* ---- every way OUR side can fail, and the app is still owed -------------- */
//
// These are the three outcomes cloud/builder/src/server.js actually sends
// when a build dies: "krate-failed" for an engine or provider death (its
// honest default when the exit code cannot tell them apart),
// "infra-failed" for a KRATE_BUILD_TIMEOUT_MS kill, and "stopped" when the
// person pressed stop. Not one of them produced a file, so not one of them
// may cost the free app.
//
// Dana fails four times in a row -- a bad afternoon on the provider is
// exactly this -- and must still be able to make her app afterwards.
for (const outcome of ["krate-failed", "infra-failed", "krate-failed", "stopped"]) {
  const { opened, id } = await build(DEV_D, "krs_dana", "a tiny notes app", outcome);
  assert.strictEqual(
    opened.status, 200,
    `a retry after a failure must still open a case, got ${opened.status}: ${opened.text}`,
  );
  assert.ok(id, `the failed build got a case: ${opened.text}`);
  assert.strictEqual(
    opened.body.n, 0,
    `after a ${outcome} the ledger must still read zero apps made, got ${opened.body.n}`,
  );
}

// The ledger agrees, read the way the person's own page reads it.
const afterFailures = (await call("/case/list", { device: DEV_D }, "krs_dana")).body;
assert.strictEqual(
  afterFailures.n, 0,
  `four failed builds consumed nothing: ${JSON.stringify(afterFailures)}`,
);
assert.strictEqual(afterFailures.cases.length, 4, "but all four are on the record");

/* ---- and the free app is genuinely still there --------------------------- */
// The point of the whole exercise. Not "the counter says zero" -- that the
// door still opens and the app she asked for gets made.
const real = await build(DEV_D, "krs_dana", "a tiny notes app", "made");
assert.strictEqual(
  real.opened.status, 200,
  `the free app survives our failures: ${real.opened.text}`,
);
const spent = (await call("/case/list", { device: DEV_D }, "krs_dana")).body;
assert.strictEqual(spent.n, 1, "and making it is what finally spends it");

// Now, and only now, the wall closes.
const walled = await call("/case/open", { device: DEV_D, request: "another one" }, "krs_dana");
assert.strictEqual(walled.status, 402, `the SECOND app is refused: ${walled.text}`);
assert.strictEqual(walled.body.wall, true, "and it is a wall, not an error");

/* ---- an off-request app is a file, and does cost ------------------------- */
//
// The other half of the same honesty. `off-request` is the engine's exit 6:
// the app built, runs, and is not what was asked for -- the person HAS a
// file, so pretending it was free would be the opposite lie.
//
// The word the hub knows for it is "not-as-asked". Recorded, and free:
// the case stays open so the change that fixes it is not a second app.
const named = await build(DEV_E, "krs_erin", "a timer", "not-as-asked");
const erinLedger = (await call("/case/list", { device: DEV_E }, "krs_erin")).body;
assert.strictEqual(
  erinLedger.cases[0].attempts.length, 1,
  `an outcome the hub knows is recorded: ${JSON.stringify(erinLedger.cases[0])}`,
);
assert.strictEqual(erinLedger.n, 0, "and exit 6 spends nothing on its own");

/* ---- the builder's word for it is not the hub's ------------------------- */
//
// LIVE DEFECT, pinned here rather than described. cloud/builder/src/server.js
// sends the literal "off-request" (its close handler, at the `offRequest`
// branch); the hub's CASE_OUTCOMES above knows the same state as
// "not-as-asked" and answers 400 to anything else. `caseAttempt` is
// fire-and-forget -- `.catch(() => {})` -- so the rejection is SILENT: the
// case is left open forever with no attempt on it, and the person's own
// ledger page shows a build that apparently never happened.
//
// It is not a money bug today (an unrecorded attempt cannot consume an
// allowance, so it fails safe), which is exactly why nothing caught it.
// When this is fixed -- in either file -- this assertion fails and should
// be replaced by the one above it, applied to whichever word both sides
// then agree on.
const mismatch = await call(
  "/case/attempt",
  { device: DEV_E, id: named.id, outcome: "off-request" },
  "krs_erin",
);
assert.strictEqual(
  mismatch.status, 400,
  "the builder's 'off-request' is still a word the hub refuses -- if this " +
    "now passes, the two sides were reconciled and this guard should become " +
    "a positive assertion on the agreed word",
);

console.log(
  "OK -- our failures cost nobody their free app, and only a file that was made spends it",
);
