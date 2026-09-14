/* A person's own API key, and what their builds cost.
 *
 * Krate pays for the first app made in a browser. After that somebody can
 * bring their own key and keep building in the tab -- so the hub has to
 * hold that key, and holding a customer's API key is the most sensitive
 * thing this worker does. Two properties matter more than the feature:
 *
 *   1. The key is encrypted at rest. KV is not a secret store; a dump of
 *      the namespace must not be a list of customers' API keys.
 *   2. The key never comes back to a browser. What a page may learn is
 *      that a key is set and its last four characters.
 *
 * Drives the REAL worker -- `import worker from "../src/index.js"` and
 * Request objects -- so the routing, the auth and the crypto are the
 * shipped ones, not a mirror that can drift.
 *
 *   node --experimental-wasm-modules cloud/worker/test/api-keys.test.mjs
 */
import assert from "node:assert/strict";
import worker from "../src/index.js";

const KV = new Map();
const env = {
  // The wrapping secret. In production this is a Cloudflare secret; absent,
  // every key route refuses rather than storing something it cannot protect.
  KEY_WRAP_SECRET: "test-wrapping-secret",
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix, limit = 1000 }) => ({
      keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).slice(0, limit).map((name) => ({ name })),
    }),
  },
};

KV.set("session:krs_alice", "u-alice");
KV.set("user:u-alice", JSON.stringify({ id: "u-alice", login: "alice" }));
KV.set("session:krs_bob", "u-bob");
KV.set("user:u-bob", JSON.stringify({ id: "u-bob", login: "bob" }));

async function call(method, path, body, token, envOverride) {
  const headers = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(
    new Request(`https://hub.test${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
    envOverride || env,
    { waitUntil() {} },
  );
  const text = await res.text();
  let parsed = null;
  try { parsed = JSON.parse(text); } catch (e) { /* plain text */ }
  return { status: res.status, body: parsed, text };
}

const REAL_KEY = "sk-ant-api03-thisisapretendkeylongenoughtopass";

/* ---- signing in is the whole authorisation ------------------------------ */
assert.strictEqual((await call("GET", "/keys")).status, 401, "no session, no keys");
assert.strictEqual((await call("POST", "/keys", { vendor: "anthropic", key: REAL_KEY })).status, 401);
assert.strictEqual((await call("GET", "/spend")).status, 401);

/* ---- nothing is set to begin with --------------------------------------- */
const empty = await call("GET", "/keys", undefined, "krs_alice");
assert.strictEqual(empty.status, 200, empty.text);
assert.deepStrictEqual(empty.body.keys.map((k) => [k.vendor, k.set]), [
  ["anthropic", false],
  ["openai", false],
], "both vendors offered, neither set");

/* ---- a key that does not look like one is refused before it is stored --- */
assert.strictEqual((await call("POST", "/keys", { vendor: "anthropic", key: "nope" }, "krs_alice")).status, 422);
assert.strictEqual((await call("POST", "/keys", { vendor: "anthropic", key: "sk-wrongvendorbutlongenough" }, "krs_alice")).status, 422,
  "an Anthropic key starts with sk-ant-");
assert.strictEqual((await call("POST", "/keys", { vendor: "pretend", key: REAL_KEY }, "krs_alice")).status, 400);
assert.strictEqual(
  (await call("POST", "/keys", { vendor: "anthropic", key: `sk-ant-has space ${"x".repeat(30)}` }, "krs_alice")).status,
  422,
  "whitespace is not part of any key we take",
);

/* ---- stored, and stored ENCRYPTED --------------------------------------- */
const saved = await call("POST", "/keys", { vendor: "anthropic", key: REAL_KEY }, "krs_alice");
assert.strictEqual(saved.status, 200, saved.text);
assert.strictEqual(saved.body.tail, REAL_KEY.slice(-4));

const atRest = KV.get("keys:u-alice");
assert.ok(atRest, "something was written");
assert.ok(!atRest.includes(REAL_KEY), "the key is NOT in KV in the clear");
assert.ok(!atRest.includes("sk-ant-api03"), "nor any recognisable part of it");
assert.ok(JSON.parse(atRest).anthropic.iv, "it is sealed with an iv");

/* ---- what a browser may learn is: set, and the last four ---------------- */
const listed = await call("GET", "/keys", undefined, "krs_alice");
assert.strictEqual(listed.body.keys[0].set, true);
assert.strictEqual(listed.body.keys[0].tail, REAL_KEY.slice(-4));
assert.ok(!listed.text.includes(REAL_KEY), "the key never comes back to the page");
assert.ok(!listed.text.includes("sk-ant-api03"), "not even most of it");

/* ---- one person's key is not another's ---------------------------------- */
const bobSees = await call("GET", "/keys", undefined, "krs_bob");
assert.strictEqual(bobSees.body.keys[0].set, false, "Bob has no key of his own");
assert.strictEqual((await call("POST", "/keys/use", { vendor: "anthropic" }, "krs_bob")).status, 404,
  "and cannot fetch Alice's");

/* ---- the build service reads it back, with the owner's session ---------- */
const used = await call("POST", "/keys/use", { vendor: "anthropic" }, "krs_alice");
assert.strictEqual(used.status, 200, used.text);
assert.strictEqual(used.body.key, REAL_KEY, "it decrypts to exactly what was stored");

// Without the wrapping secret the stored bytes are useless -- which is the
// point of encrypting them at all.
const noSecret = await call("POST", "/keys/use", { vendor: "anthropic" }, "krs_alice", { ...env, KEY_WRAP_SECRET: "" });
assert.notStrictEqual(noSecret.status, 200, "a KV dump alone does not yield the key");

/* ---- forgetting means gone ---------------------------------------------- */
assert.strictEqual((await call("POST", "/keys/forget", { vendor: "anthropic" }, "krs_alice")).status, 200);
assert.strictEqual((await call("GET", "/keys", undefined, "krs_alice")).body.keys[0].set, false);
assert.strictEqual((await call("POST", "/keys/use", { vendor: "anthropic" }, "krs_alice")).status, 404);

/* ---- the ledger: two pockets, kept apart -------------------------------- */
await call("POST", "/spend", { usd: 0.7612, model: "claude-opus-5", rounds: 7, app: "Tip calculator", paid_by: "krate" }, "krs_alice");
await call("POST", "/spend", { usd: 0.4231, model: "claude-opus-5", rounds: 5, app: "Pasta timer", paid_by: "own" }, "krs_alice");

const report = await call("GET", "/spend", undefined, "krs_alice");
assert.strictEqual(report.status, 200, report.text);
assert.strictEqual(report.body.builds, 2);
assert.strictEqual(report.body.total.own, 0.4231, "their money, on its own");
assert.strictEqual(report.body.total.krate, 0.7612, "and ours, on its own");
assert.strictEqual(report.body.recent[0].app, "Pasta timer", "newest first");

// A number that is not a number does not enter the ledger.
assert.strictEqual((await call("POST", "/spend", { usd: "lots" }, "krs_alice")).status, 400);
assert.strictEqual((await call("POST", "/spend", { usd: -5 }, "krs_alice")).status, 400);
assert.strictEqual((await call("POST", "/spend", { usd: 99999 }, "krs_alice")).status, 400);

// And one person's spending is their own.
assert.strictEqual((await call("GET", "/spend", undefined, "krs_bob")).body.builds, 0);

console.log("PASS  a key is sealed at rest and never returned to a browser");
console.log("PASS  only its owner can use it, and forgetting it removes it");
console.log("PASS  spending is recorded per build, their money apart from ours");
