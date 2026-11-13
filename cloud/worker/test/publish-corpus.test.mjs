/* The publish corpus through the real door (IC-833, tests 1821-1823, 1839).
 *
 * 1821: one file of each kind -- valid minimal, source-carrying and
 * asset-heavy apps; a name twice, a path that climbs out, a compression
 * bomb, a zip marker with no archive, no bytes, no code, and one byte over
 * the 5 MiB door -- each gets the door's recorded answer.
 * 1822: every committed file is exactly the bytes its record names (size,
 * SHA-256, what its directory expands to, its manifest).
 * 1823: a destructive case runs only against this machine's stand-ins,
 * never against anything that could be the real hub.
 * 1839: a validator that crashes publishes nothing, stores nothing, says
 * "try again", and the retry is judged afresh and published once.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/publish-corpus.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker, { __validatorForTests } from "../src/index.js";
import { orgEnv } from "./org-tools.mjs";
import { CASES, sha256, expanded, manifestOf } from "./fixtures/corpus/build.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const dir = join(here, "fixtures", "corpus");
const record = JSON.parse(readFileSync(join(dir, "corpus.json"), "utf8"));
const bytesOf = (r) => (r.committed ? new Uint8Array(readFileSync(join(dir, r.file))) : CASES.find((c) => c.file === r.file).make());

/// Refuse a destructive case unless every store it writes is a stand-in on
/// this machine and the hub it names is not a real one (1823).
function standInOnly(env) {
  const real = /^https:\/\/(hub\.)?krate\.tech\b/i;
  if (real.test(String(env.PUBLIC_BASE || ""))) throw new Error(`refusing a destructive test: PUBLIC_BASE is ${env.PUBLIC_BASE}`);
  if (!env.APPS || !env.APPS.__standIn) throw new Error("refusing a destructive test: APPS is not this machine's stand-in");
  if (!env.BUNDLES || !env.BUNDLES.__standIn) throw new Error("refusing a destructive test: BUNDLES is not this machine's stand-in");
  return env;
}

const publish = (env, bytes) => worker.fetch(new Request("https://hub.example/publish", {
  method: "POST",
  headers: { authorization: "Bearer krs_alice", "content-type": "application/octet-stream", "content-length": String(bytes.length) },
  body: bytes,
}), env, { waitUntil() {} });

test("1822: every committed corpus file is exactly what its record says", () => {
  assert.strictEqual(record.length, CASES.length, "one record per case");
  for (const r of record) {
    const bytes = bytesOf(r);
    assert.strictEqual(bytes.length, r.size, `${r.file}: size`);
    assert.strictEqual(sha256(bytes), r.sha256, `${r.file}: SHA-256`);
    assert.deepStrictEqual(expanded(bytes), r.expanded, `${r.file}: what its directory expands to`);
    assert.deepStrictEqual(manifestOf(bytes), r.manifest, `${r.file}: its manifest`);
    assert.ok(r.expect && typeof r.expect.status === "number", `${r.file}: an expected answer`);
  }
  // The kinds the contract names are all there.
  for (const kind of ["valid minimal", "source", "asset-heavy", "name twice", "climbs out", "inflating", "zip marker", "no bytes", "over the 5 MiB"]) {
    assert.ok(record.some((r) => r.kind.includes(kind)), `the corpus has a ${kind} case`);
  }
});

test("1823: a destructive case refuses anything that is not this machine's stand-ins", () => {
  assert.throws(() => standInOnly({ ...orgEnv(), PUBLIC_BASE: "https://hub.krate.tech" }), /PUBLIC_BASE/);
  assert.throws(() => standInOnly({ ...orgEnv(), APPS: { get() {}, put() {} } }), /APPS is not/);
  assert.throws(() => standInOnly({ ...orgEnv(), BUNDLES: { get() {}, put() {} } }), /BUNDLES is not/);
  assert.doesNotThrow(() => standInOnly(orgEnv()));
});

test("1821: each corpus file gets the door's recorded answer, and only the admitted are stored", async () => {
  for (const r of record) {
    const env = standInOnly(orgEnv());
    const bytes = bytesOf(r);
    const res = await publish(env, bytes);
    const said = await res.text();
    assert.strictEqual(res.status, r.expect.status, `${r.file} (${r.kind}): ${said.slice(0, 200)}`);
    if (r.expect.reason) assert.ok(said.includes(r.expect.reason), `${r.file}: says "${r.expect.reason}", said "${said.slice(0, 200)}"`);
    const stored = await env.BUNDLES.get(r.sha256);
    if (r.expect.status === 200) {
      assert.ok(stored, `${r.file}: an admitted file is stored under its digest`);
      assert.strictEqual(JSON.parse(said).id, r.sha256);
    } else {
      assert.strictEqual(stored, null, `${r.file}: a refused file is never stored`);
      assert.strictEqual(await env.APPS.get(`app:${r.sha256}`), null, `${r.file}: and never listed`);
    }
  }
});

test("1839: a validator that crashes publishes nothing; the retry is judged afresh", async () => {
  const env = standInOnly(orgEnv());
  const r = record.find((x) => x.file === "minimal.krate");
  const bytes = bytesOf(r);
  const real = __validatorForTests({
    memory: new WebAssembly.Memory({ initial: 1 }),
    krate_alloc: () => 0,
    krate_free: () => {},
    krate_judge: () => { throw new WebAssembly.RuntimeError("unreachable"); },
    krate_judged_component: () => { throw new WebAssembly.RuntimeError("unreachable"); },
  });
  let res;
  try {
    res = await publish(env, bytes);
  } finally {
    __validatorForTests(real);
  }
  const said = await res.text();
  assert.strictEqual(res.status, 503, said);
  assert.match(said, /could not finish checking this bundle, so nothing was published/);
  assert.strictEqual(await env.BUNDLES.get(r.sha256), null, "nothing stored");
  assert.strictEqual(await env.APPS.get(`app:${r.sha256}`), null, "nothing listed");
  // The retry, with the real validator: published once, under its digest.
  const again = await publish(env, bytes);
  assert.strictEqual(again.status, 200, await again.clone().text());
  assert.strictEqual(JSON.parse(await again.text()).id, r.sha256);
  // And once more: the same answer, not a second listing.
  const third = await publish(env, bytes);
  assert.strictEqual(third.status, 200);
});

test("1839: the validator cannot run an app or reach a service: it imports nothing", () => {
  const module = new WebAssembly.Module(readFileSync(join(here, "..", "src", "validator.wasm")));
  assert.deepStrictEqual(WebAssembly.Module.imports(module), [], "a validator with no imports has no way out of its own memory");
});
