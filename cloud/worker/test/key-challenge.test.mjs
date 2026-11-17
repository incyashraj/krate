/* Proof of an Android app's signing key (IC-800, portable test 1717).
 *
 * Drives the REAL worker and the REAL validator (crates/apksig inside it),
 * with challenge answers signed by Android's own apksigner
 * (crates/apksig/tests/fixtures/README.md). What matters:
 *
 *   1. A challenge is issued only to a signed-in person, binds package,
 *      certificate, the APK, the import, the account, a nonce and an
 *      expiry, and lives fifteen minutes.
 *   2. An answer signed with the app's key, by the account it was issued
 *      to, gets the hub's signed statement of key control -- once. A
 *      replay, another account, another key or another statement does not.
 *
 *   node --experimental-wasm-modules cloud/worker/test/key-challenge.test.mjs
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import worker from "../src/index.js";
import { KEY_CONTROL_DOMAIN, CHALLENGE_SECONDS, PURPOSE } from "../src/key-challenge.js";
import { canonicalJson } from "../src/org.js";

const fixture = (name) =>
  new Uint8Array(readFileSync(new URL(`../../../crates/apksig/tests/fixtures/${name}`, import.meta.url)));

const KV = new Map();
const TTL = new Map();
const LEASE_SEED = "11".repeat(32);
const env = {
  ORG_LEASE_KEY: LEASE_SEED,
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v, opts) => { KV.set(k, v); if (opts?.expirationTtl) TTL.set(k, opts.expirationTtl); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix, limit = 1000 }) => ({
      keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).slice(0, limit).map((name) => ({ name })),
    }),
  },
};
// The account the fixture's challenge was issued to, and another.
KV.set("session:krs_owner", "fixture-account");
KV.set("user:fixture-account", JSON.stringify({ id: "fixture-account", login: "owner" }));
KV.set("session:krs_other", "u-other");
KV.set("user:u-other", JSON.stringify({ id: "u-other", login: "other" }));

async function call(path, token, body, type = "application/json") {
  const headers = { "content-type": type };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(
    new Request(`https://hub.test${path}`, { method: "POST", headers, body }),
    env,
    { waitUntil() {} },
  );
  const text = await res.text();
  let value = null;
  try { value = JSON.parse(text); } catch {}
  return { status: res.status, value, text };
}

/* ---- issuing --------------------------------------------------------------- */
const ask = {
  package: "com.example.owned",
  certificate_sha256: "19d4c337b50b2e946cb268adfe217d4212f5d25e111aa32e73341a17b65622fb",
  apk_sha256: "ab".repeat(32),
  request: "import-1",
};
assert.strictEqual((await call("/port/key-challenge", null, JSON.stringify(ask))).status, 401, "signed in first");
for (const bad of [
  { ...ask, package: "not a package" },
  { ...ask, certificate_sha256: "XYZ" },
  { ...ask, request: "../etc" },
]) {
  assert.strictEqual((await call("/port/key-challenge", "krs_owner", JSON.stringify(bad))).status, 400);
}
const before = Math.floor(Date.now() / 1000);
const issued = await call("/port/key-challenge", "krs_owner", JSON.stringify(ask));
assert.strictEqual(issued.status, 200, issued.text);
const s = issued.value.statement;
assert.deepStrictEqual(
  { package: s.package, certificate_sha256: s.certificate_sha256, apk_sha256: s.apk_sha256, request: s.request, account: s.account },
  { ...ask, account: "fixture-account" },
  "every bound field, the account from the session not the body",
);
assert.match(s.nonce, /^[0-9a-f]{32}$/);
assert.strictEqual(s.purpose, PURPOSE);
assert.ok(s.expires_at >= before + CHALLENGE_SECONDS && s.expires_at <= before + CHALLENGE_SECONDS + 5);
assert.deepStrictEqual(JSON.parse(KV.get(`keychal:${s.nonce}`)), s, "kept under its nonce");
assert.ok(TTL.get(`keychal:${s.nonce}`) <= CHALLENGE_SECONDS + 60, "and only for a while");
const second = await call("/port/key-challenge", "krs_owner", JSON.stringify(ask));
assert.notStrictEqual(second.value.statement.nonce, s.nonce, "a fresh nonce every time");

/* ---- answering ------------------------------------------------------------- */
// The fixture answer's statement, as if this hub had issued it.
const answer = fixture("challenge-rsa.apk");
const wrongKey = fixture("challenge-wrong-key.apk");
const seedFixture = () => {
  const statement = {
    v: 1,
    purpose: PURPOSE,
    package: "com.example.owned",
    certificate_sha256: "19d4c337b50b2e946cb268adfe217d4212f5d25e111aa32e73341a17b65622fb",
    apk_sha256: "123af10d657699cd4c7d5aca348358920f940d7dff2bbb4fcb837f7be1186e76",
    request: "fixture-import",
    account: "fixture-account",
    nonce: "00112233445566778899aabbccddeeff",
    expires_at: 4102444800,
  };
  KV.set(`keychal:${statement.nonce}`, JSON.stringify(statement));
  return statement;
};
const APK = "application/vnd.android.package-archive";

seedFixture();
assert.strictEqual((await call("/port/key-challenge/answer", "krs_other", answer, APK)).status, 403, "another account");
assert.ok(KV.has("keychal:00112233445566778899aabbccddeeff"), "a refused account does not use the nonce up");

const wrong = await call("/port/key-challenge/answer", "krs_owner", wrongKey, APK);
assert.strictEqual(wrong.status, 422, wrong.text);
assert.strictEqual(wrong.value.error, "wrong-key", wrong.text);
assert.ok(!KV.has("keychal:00112233445566778899aabbccddeeff"), "a wrong answer uses the nonce up");

const statement = seedFixture();
const proved = await call("/port/key-challenge/answer", "krs_owner", answer, APK);
assert.strictEqual(proved.status, 200, proved.text);
const a = proved.value.attestation;
assert.deepStrictEqual(
  [a.kind, a.package, a.certificate_sha256, a.signed_with, a.apk_sha256, a.request, a.account, a.nonce],
  ["android-signing-key-control", statement.package, statement.certificate_sha256, statement.certificate_sha256, statement.apk_sha256, statement.request, statement.account, statement.nonce],
);
// Signed by the hub's key, under its own domain: not readable as a lease.
const unhex = (t) => Uint8Array.from(t.match(/../g), (h) => parseInt(h, 16));
const pkcs8 = new Uint8Array([...unhex("302e020100300506032b657004220420"), ...unhex(LEASE_SEED)]);
const priv = await crypto.subtle.importKey("pkcs8", pkcs8, { name: "Ed25519" }, true, ["sign"]);
const jwk = await crypto.subtle.exportKey("jwk", priv);
const pub = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: jwk.x }, { name: "Ed25519" }, false, ["verify"]);
const signed = new TextEncoder().encode(KEY_CONTROL_DOMAIN + canonicalJson(a));
assert.ok(await crypto.subtle.verify({ name: "Ed25519" }, pub, unhex(proved.value.signature), signed));
const asLease = new TextEncoder().encode("krate-lease-v1\n" + canonicalJson(a));
assert.ok(!(await crypto.subtle.verify({ name: "Ed25519" }, pub, unhex(proved.value.signature), asLease)));

// Once: the same answer again is a replay.
const replay = await call("/port/key-challenge/answer", "krs_owner", answer, APK);
assert.strictEqual(replay.status, 410, replay.text);
assert.strictEqual(replay.value.error, "unknown-challenge");

// An answer the hub never issued, and an unsigned one.
assert.strictEqual((await call("/port/key-challenge/answer", "krs_owner", answer, APK)).status, 410);
assert.strictEqual((await call("/port/key-challenge/answer", "krs_owner", new Uint8Array([1, 2, 3]), APK)).status, 422);
assert.strictEqual((await call("/port/key-challenge/answer", null, answer, APK)).status, 401);

console.log("key challenge: issued, answered once, refused for replay, another account and another key");
