/* The hub can tell what it holds, reconcile it, export it and put it back
 * (IC-852, tests 1869, 1870, 1886, 1890, 1896).
 *
 * Driven against the real handler. Drift is manufactured the way it
 * happens: bytes deleted behind a listing, an alias left behind, a channel
 * whose release is gone, an object that hashes to something other than
 * its key, an object nothing names.
 *
 *   node --experimental-wasm-modules cloud/worker/test/durability.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker, { canonicalRecords } from "../src/index.js";
import { r2Mock, sha256Hex } from "./r2-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const V1 = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));
const V2 = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-stored.krate")));

function env() {
  const kv = new Map([
    ["session:krs_alice", "u1"], ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
    ["session:krs_bob", "u2"], ["user:u2", JSON.stringify({ login: "bob", name: "Bob", avatar_url: "" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    BUNDLES: r2Mock(),
    KRATE_ADMINS: "alice",
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}
const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
const admin = (path, init = {}) => req(path, { ...init, headers: { authorization: "Bearer krs_alice", ...(init.headers || {}) } });
async function publish(e, bytes, name) {
  const res = await worker.fetch(req("/publish", { method: "POST", headers: { authorization: "Bearer krs_alice", "x-krate-name": name }, body: bytes }), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  return JSON.parse(await res.text());
}
const scrub = async (e, repair = false) => {
  const res = await worker.fetch(admin("/admin/scrub", { method: repair ? "POST" : "GET" }), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  return JSON.parse(await res.text());
};

/* ---- a healthy hub scrubs clean ---------------------------------------- */
const e = env();
const a = await publish(e, V1, "Notes");
const b = await publish(e, V2, "Sketch");
let report = await scrub(e);
assert.strictEqual(report.problems.length, 0, JSON.stringify(report.problems));
assert.strictEqual(report.bundles, 2);
assert.strictEqual(report.listings, 2);
assert.strictEqual(await (await worker.fetch(req("/health"), e)).text(), "ok");

/* ---- drift, of every kind (1869, 1896) --------------------------------- */
// bytes gone behind a listing, its alias, and its channel
e.BUNDLES._blobs.delete(a.id);
// an alias to nothing
await e.APPS.put("alias:deadbeefdead", "0".repeat(64));
// an object whose bytes do not hash to its key
const bogus = "f".repeat(64);
e.BUNDLES._blobs.set(bogus, new Uint8Array([1, 2, 3]));
// an object nothing names
const orphan = await sha256Hex(new Uint8Array([9, 9, 9]));
e.BUNDLES._blobs.set(orphan, new Uint8Array([9, 9, 9]));

report = await scrub(e);
const kinds = (r) => r.problems.map((p) => p.kind).sort();
assert.deepStrictEqual(
  kinds(report),
  ["dangling-alias", "dangling-alias", "dangling-channel", "dangling-listing", "digest-mismatch", "orphan-object", "orphan-object"].sort(),
  JSON.stringify(report.problems, null, 1),
);
assert.ok(report.problems.every((p) => p.repaired === false), "a GET scrub repairs nothing");
assert.ok(e._kv.has(`app:${a.id}`), "and the dangling listing is still there");
const healthText = await (await worker.fetch(req("/health"), e)).text();
assert.match(healthText, /found 7 problem/, `the alert is on /health: ${healthText}`);
assert.match(healthText, /dangling-listing/);

/* ---- repairs remove only what cannot be lost (1869, 1870) -------------- */
report = await scrub(e, true);
const repaired = report.problems.filter((p) => p.repaired).map((p) => p.kind).sort();
assert.deepStrictEqual(repaired, ["dangling-alias", "dangling-alias", "dangling-channel", "dangling-listing"]);
assert.ok(!e._kv.has(`app:${a.id}`), "the listing to gone bytes is removed");
assert.ok(!e._kv.has("alias:deadbeefdead"), "the alias to nothing is removed");
assert.strictEqual((await worker.fetch(req("/c/alice/notes"), e)).status, 404, "the channel is settled to nothing");
assert.ok(e.BUNDLES._blobs.has(bogus), "a mismatching object is reported, never deleted by a scrub");
assert.ok(e.BUNDLES._blobs.has(orphan), "an orphan object is reported, never deleted by a scrub");
assert.ok(e.BUNDLES._blobs.has(b.id), "the healthy app is untouched");
// A second scrub still names what repairs cannot fix.
report = await scrub(e);
assert.deepStrictEqual(kinds(report), ["digest-mismatch", "orphan-object", "orphan-object"].sort());

/* ---- the scheduled run stores its report (1886) ------------------------- */
{
  const e2 = env();
  await publish(e2, V1, "Notes");
  await worker.scheduled({ cron: "17 3 * * *" }, e2, {});
  const last = await worker.fetch(admin("/admin/scrub/last"), e2);
  assert.strictEqual(last.status, 200);
  const stored = JSON.parse(await last.text());
  assert.strictEqual(stored.schema, "krate.hub-scrub.v1");
  assert.strictEqual(stored.repair, false, "the scheduled run never repairs");
}

/* ---- export, wipe, restore: exact control records (1890) --------------- */
{
  const e2 = env();
  const p1 = await publish(e2, V1, "Notes");
  const p2 = await publish(e2, V2, "Sketch");
  const td = await worker.fetch(admin(`/admin/takedown/${p1.id}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ reason: "x" }) }), e2);
  assert.strictEqual(td.status, 200, await td.clone().text());
  const before = new Map([...e2._kv].filter(([k]) => ["app:", "alias:", "channel:", "takedown:", "takedown-closed:"].some((f) => k.startsWith(f))));
  assert.ok(before.size >= 5, `a real set of control records: ${before.size}`);

  const res = await worker.fetch(admin("/admin/export"), e2);
  assert.strictEqual(res.status, 200);
  assert.match(res.headers.get("content-disposition") || "", /krate-hub-export-/);
  const dump = JSON.parse(await res.text());
  assert.strictEqual(dump.schema, "krate.hub-export.v1");
  assert.strictEqual(dump.count, before.size);
  assert.ok(!JSON.stringify(dump).includes("krs_alice"), "an export carries no sessions");
  assert.ok(!Object.keys(dump.records).some((f) => f.startsWith("session")), "nor session families");

  // Wipe every control record, keep the bytes, restore.
  for (const k of before.keys()) e2._kv.delete(k);
  const restored = await worker.fetch(admin("/admin/restore", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(dump) }), e2);
  assert.strictEqual(restored.status, 200, await restored.clone().text());
  const outcome = JSON.parse(await restored.text());
  assert.strictEqual(outcome.restored, before.size);
  assert.strictEqual(outcome.skipped, 0);
  for (const [k, v] of before) {
    assert.strictEqual(e2._kv.get(k), v, `${k} restored exactly`);
  }
  assert.strictEqual((await scrub(e2)).problems.length, 0, "a restored hub scrubs clean");

  // Restoring again over existing records skips them unless asked.
  const again = JSON.parse(await (await worker.fetch(admin("/admin/restore", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(dump) }), e2)).text());
  assert.strictEqual(again.restored, 0);
  assert.strictEqual(again.skipped, before.size);
  await e2.APPS.put(`app:${p2.id}`, "{\"edited\":true}");
  const over = JSON.parse(await (await worker.fetch(admin("/admin/restore?overwrite", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(dump) }), e2)).text());
  assert.strictEqual(over.restored, before.size);
  assert.strictEqual(e2._kv.get(`app:${p2.id}`), before.get(`app:${p2.id}`), "overwrite puts the exported record back");

  // An export edited in transit restores nothing.
  const forged = JSON.parse(JSON.stringify(dump));
  forged.records["app:"][`app:${p2.id}`] = "{\"author_login\":\"mallory\"}";
  for (const k of before.keys()) e2._kv.delete(k);
  const refused = await worker.fetch(admin("/admin/restore", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(forged) }), e2);
  assert.strictEqual(refused.status, 400, await refused.clone().text());
  assert.match(await refused.text(), /digest does not match/);
  assert.strictEqual([...e2._kv.keys()].filter((k) => k.startsWith("app:")).length, 0, "nothing was restored from a forged export");
  // A record smuggled into the wrong family is refused too.
  const smuggled = JSON.parse(JSON.stringify(dump));
  smuggled.records["app:"]["session:krs_mallory"] = "u9";
  smuggled.digest = await sha256Hex(canonicalRecords(smuggled.records));
  const r2 = await worker.fetch(admin("/admin/restore", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(smuggled) }), e2);
  assert.strictEqual(r2.status, 400, await r2.clone().text());
  assert.match(await r2.text(), /not a app: record/, "refused for the family, with the digest verified first");
}

/* ---- all of it is admin-only, and invisible to others ------------------ */
for (const [path, method] of [["/admin/scrub", "GET"], ["/admin/scrub", "POST"], ["/admin/scrub/last", "GET"], ["/admin/export", "GET"], ["/admin/restore", "POST"]]) {
  const res = await worker.fetch(req(path, { method, headers: { authorization: "Bearer krs_bob" } }), e);
  assert.strictEqual(res.status, 404, `${method} ${path} is not a door for a non-admin`);
}

console.log("OK -- the hub inventories its bytes and control records, names every kind of drift, repairs only what cannot be lost, alerts on /health, stores the scheduled report, and exports and restores its control records exactly, refusing a forged export");
