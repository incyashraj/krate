/* A person's web builds are signed as their account (IC-983, option B).
 *
 * The Fly builder runs an AI agent, so it never holds a key. It hands each
 * finished bundle to /sign/web-build and the hub signs it with a key kept
 * for that account alone. What matters:
 *
 *   1. Only the build service, for the person whose build it is, may ask:
 *      a session alone (a script on a krate.tech page) signs nothing.
 *   2. One account, one key: every build of theirs is the same publisher,
 *      and another account's builds are another publisher.
 *   3. The key is sealed at rest and never in an answer.
 *
 * Drives the REAL worker and the REAL validator.
 *
 *   node --experimental-wasm-modules cloud/worker/test/web-build-signing.test.mjs
 */
import assert from "node:assert/strict";
import worker from "../src/index.js";
import * as validator from "../src/validator.wasm";
import { zip, manifest, COMPONENT } from "./org-bundle-tools.mjs";

const KV = new Map();
const env = {
  KEY_WRAP_SECRET: "test-wrapping-secret",
  KRATE_BUILDER_SECRET: "test-builder-secret",
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

const AS_BUILDER = { "x-krate-builder": "test-builder-secret" };
// As the engine leaves it on the builder: unsigned.
const BUNDLE = zip({ "manifest.toml": manifest("1.0.0"), "code.wasm": COMPONENT });

async function sign(token, headers = AS_BUILDER, body = BUNDLE, on = env) {
  const all = { "content-type": "application/vnd.krate", ...headers };
  if (token) all.authorization = `Bearer ${token}`;
  const res = await worker.fetch(
    new Request("https://hub.test/sign/web-build", { method: "POST", headers: all, body }),
    on,
    { waitUntil() {} },
  );
  return {
    status: res.status,
    bytes: new Uint8Array(await res.arrayBuffer()),
    publisher: res.headers.get("x-krate-publisher"),
    namespace: res.headers.get("x-krate-namespace"),
    cors: res.headers.get("access-control-allow-origin"),
  };
}

function judge(bytes) {
  const input = validator.krate_alloc(bytes.length);
  new Uint8Array(validator.memory.buffer, input, bytes.length).set(bytes);
  const out = validator.krate_judge(input, bytes.length);
  const len = new DataView(validator.memory.buffer).getUint32(out, true);
  const answer = JSON.parse(new TextDecoder().decode(new Uint8Array(validator.memory.buffer, out + 4, len)));
  validator.krate_free(out, 4 + len);
  validator.krate_free(input, bytes.length);
  return answer;
}

/* ---- only the build service, for that person ---------------------------- */
assert.strictEqual((await sign("krs_alice", {})).status, 403, "a session alone signs nothing");
assert.strictEqual((await sign(undefined)).status, 401, "the builder's secret alone names no account");
assert.strictEqual((await sign("krs_alice", { "x-krate-builder": "nope" })).status, 403);
assert.ok(![...KV.keys()].some((k) => k.startsWith("signkey:")), "no key made for a refused ask");

/* ---- signed as the account ---------------------------------------------- */
assert.strictEqual(judge(BUNDLE).judgement.signature.state, "absent", "the fixture starts unsigned");
const first = await sign("krs_alice");
assert.strictEqual(first.status, 200);
assert.strictEqual(first.cors, null, "server to server: no CORS");
const judged = judge(first.bytes);
assert.strictEqual(judged.ok, true, JSON.stringify(judged));
assert.strictEqual(judged.judgement.signature.state, "valid");
assert.match(first.namespace, /^krate\.tech\/u-alice\//, first.namespace);
assert.match(first.publisher, /^[0-9a-f]{64}$/);

/* ---- one account, one publisher ----------------------------------------- */
const again = await sign("krs_alice");
assert.strictEqual(again.publisher, first.publisher, "a rebuild is the same publisher");
const bob = await sign("krs_bob");
assert.notStrictEqual(bob.publisher, first.publisher, "another account is another publisher");
assert.match(bob.namespace, /^krate\.tech\/u-bob\//);

/* ---- the key is sealed, and never in an answer -------------------------- */
const stored = KV.get("signkey:u-alice");
assert.ok(stored, "kept for the account");
const sealed = JSON.parse(stored);
assert.ok(sealed.iv && sealed.body, "sealed, not plain");
assert.ok(!/[0-9a-f]{64}/.test(stored), "no hex seed at rest");

/* ---- a key that does not open is never replaced ------------------------- */
{
  const other = { ...env, KEY_WRAP_SECRET: "a-different-secret" };
  const refused = await sign("krs_alice", AS_BUILDER, BUNDLE, other);
  assert.strictEqual(refused.status, 503, "a second key would be a second publisher");
  assert.strictEqual(KV.get("signkey:u-alice"), stored, "the held key is left as it was");
  const unset = await sign("krs_alice", AS_BUILDER, BUNDLE, { ...env, KEY_WRAP_SECRET: "" });
  assert.strictEqual(unset.status, 503, "no wrapping secret: nothing made, nothing signed");
}

/* ---- not a bundle -------------------------------------------------------- */
assert.strictEqual((await sign("krs_alice", AS_BUILDER, new Uint8Array(0))).status, 400);
assert.strictEqual((await sign("krs_alice", AS_BUILDER, new TextEncoder().encode("not a zip"))).status, 422);

console.log("web-build-signing: every check passed");
