/* A registry takedown is a record with a notice, a reason, a scope and an
 * appeal (IC-669, tests 1313 and 1314).
 *
 * Before this the only removals were the author's own unpublish and an
 * admin purge that deleted everything and left a 404: no reason anywhere,
 * nothing for the author to answer, and no way to tell "never existed"
 * from "removed". Driven against the real handler with a real committed
 * bundle, through the whole life of a block: publish, block, the hash
 * answers 451 with the notice, the gallery no longer lists it, the author
 * appeals, the admin restores, and the app is back as it was. The bytes
 * scope removes the bytes and says so on restore.
 *
 *   node cloud/worker/test/takedown.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const BOUNCE = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));

function env() {
  const kv = new Map([
    ["session:krs_alice", "u1"], ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
    ["session:krs_bob", "u2"], ["user:u2", JSON.stringify({ login: "bob", name: "Bob", avatar_url: "" })],
    ["session:krs_root", "u3"], ["user:u3", JSON.stringify({ login: "root", name: "Root", avatar_url: "" })],
  ]);
  const blobs = new Map();
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    BUNDLES: {
      head: async (k) => (blobs.has(k) ? {} : null),
      get: async (k) => (blobs.has(k) ? { body: blobs.get(k), arrayBuffer: async () => blobs.get(k) } : null),
      put: async (k, v) => { blobs.set(k, v); },
      delete: async (k) => { blobs.delete(k); },
    },
    KRATE_ADMINS: "root",
    PUBLIC_BASE: "https://hub.example",
    _kv: kv, _blobs: blobs,
  };
}

const as = (who) => ({ authorization: `Bearer krs_${who}`, "content-type": "application/json" });
const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
const post = (path, who, body) => req(path, { method: "POST", headers: as(who), body: JSON.stringify(body) });

async function publish(e) {
  const res = await worker.fetch(req("/publish", { method: "POST", headers: { authorization: "Bearer krs_alice" }, body: BOUNCE }), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  return JSON.parse(await res.text()).id;
}

/* ---- only an admin may take down, and a reason is required ------------- */
{
  const e = env(); const id = await publish(e);
  assert.strictEqual((await worker.fetch(post(`/admin/takedown/${id}`, "bob", { reason: "x" }), e)).status, 404, "a non-admin sees nothing");
  assert.strictEqual((await worker.fetch(post(`/admin/takedown/${id}`, "root", {}), e)).status, 400, "no reason, no takedown");
  assert.strictEqual((await worker.fetch(post(`/admin/takedown/${id}`, "root", { reason: "x", scope: "everything" }), e)).status, 400, "an unknown scope is refused");
  assert.strictEqual((await worker.fetch(post(`/admin/takedown/${"0".repeat(64)}`, "root", { reason: "x" }), e)).status, 404, "nothing published under the hash");
}

/* ---- the whole life of a listing-scope block --------------------------- */
{
  const e = env(); const id = await publish(e);

  // Before: served and listed.
  assert.strictEqual((await worker.fetch(req(`/a/${id}?dl=1`), e)).status, 200);
  assert.ok((await (await worker.fetch(req("/apps"), e)).text()).includes(id), "listed before the block");

  const td = await worker.fetch(post(`/admin/takedown/${id}`, "root", { reason: "impersonates a bank", notice: "reported 2026-09-13" }), e);
  assert.strictEqual(td.status, 200, await td.clone().text());
  const record = JSON.parse(await td.text()).takedown;
  assert.strictEqual(record.scope, "listing");
  assert.strictEqual(record.by, "root");
  assert.strictEqual(record.appeal.state, "none");

  // The hash answers with the notice, not a 404 (1313: notice + reason).
  const blocked = await worker.fetch(req(`/a/${id}?dl=1`), e);
  assert.strictEqual(blocked.status, 451, "a removed app is a 451 with a notice, never a 404");
  const notice = JSON.parse(await blocked.text());
  assert.strictEqual(notice.reason, "impersonates a bank");
  assert.strictEqual(notice.appeal.state, "none");
  assert.match(notice.appeal.how, /appeal/);
  assert.match(blocked.headers.get("link") || "", /blocked-by/);

  // The public notice, the meta, and the gallery.
  assert.strictEqual((await worker.fetch(req(`/takedown/${id}`), e)).status, 451);
  assert.strictEqual((await worker.fetch(req(`/meta/${id}`), e)).status, 451);
  assert.ok(!(await (await worker.fetch(req("/apps"), e)).text()).includes(id), "the gallery no longer lists it");

  // The bytes are still there: a listing-scope block keeps them (1314).
  assert.ok(e._blobs.has(id), "listing scope keeps the bytes for a restore");

  // Appeal: only the author.
  assert.strictEqual((await worker.fetch(post(`/takedown/${id}/appeal`, "bob", { text: "not mine to say" }), e)).status, 403);
  assert.strictEqual((await worker.fetch(post(`/takedown/${id}/appeal`, "alice", {}), e)).status, 400, "an appeal needs words");
  const ap = await worker.fetch(post(`/takedown/${id}/appeal`, "alice", { text: "it is a demo, the bank name is fictional" }), e);
  assert.strictEqual(ap.status, 200, await ap.clone().text());
  assert.strictEqual(JSON.parse(await (await worker.fetch(req(`/takedown/${id}`), e)).text()).appeal.state, "appealed");

  // Restore: the app is back exactly as it was, and the record is kept closed.
  assert.strictEqual((await worker.fetch(post(`/admin/takedown/${id}/restore`, "bob", {}), e)).status, 404);
  const rs = await worker.fetch(post(`/admin/takedown/${id}/restore`, "root", { note: "appeal upheld" }), e);
  assert.strictEqual(rs.status, 200, await rs.clone().text());
  assert.strictEqual(JSON.parse(await rs.text()).restored, true);
  assert.strictEqual((await worker.fetch(req(`/a/${id}?dl=1`), e)).status, 200, "served again");
  assert.ok((await (await worker.fetch(req("/apps"), e)).text()).includes(id), "listed again");
  const closedRes = await worker.fetch(req(`/takedown/${id}`), e);
  assert.strictEqual(closedRes.status, 200, "a closed takedown must still answer at its notice URL, not vanish into a 404");
  const after = JSON.parse(await closedRes.text());
  assert.strictEqual(after.was_removed, true, "the closed record still answers: it WAS removed, and why");
  assert.strictEqual(after.closed.decision, "restored");
}

/* ---- a denied appeal stays blocked and says so -------------------------- */
{
  const e = env(); const id = await publish(e);
  await worker.fetch(post(`/admin/takedown/${id}`, "root", { reason: "malware", emergency: true }), e);
  await worker.fetch(post(`/takedown/${id}/appeal`, "alice", { text: "false positive" }), e);
  const dn = await worker.fetch(post(`/admin/takedown/${id}/deny`, "root", { note: "confirmed by two scanners" }), e);
  assert.strictEqual(dn.status, 200, await dn.clone().text());
  const notice = JSON.parse(await (await worker.fetch(req(`/a/${id}?dl=1`), e)).text());
  assert.strictEqual(notice.appeal.state, "denied");
  assert.strictEqual(notice.emergency, true, "the emergency flag is on the notice");
}

/* ---- the bytes scope removes the bytes, keeps the notice, and restore says so */
{
  const e = env(); const id = await publish(e);
  await worker.fetch(post(`/admin/takedown/${id}`, "root", { reason: "malware", scope: "listing-and-bytes", emergency: true }), e);
  assert.ok(!e._blobs.has(id), "bytes scope removes the bytes");
  assert.strictEqual((await worker.fetch(req(`/a/${id}?dl=1`), e)).status, 451, "the notice outlives the bytes");
  const rs = JSON.parse(await (await worker.fetch(post(`/admin/takedown/${id}/restore`, "root", {}), e)).text());
  assert.strictEqual(rs.restored, false);
  assert.match(rs.note, /publish again/);
}

console.log("OK -- a takedown is a record with a reason, a scope and an appeal: the hash answers 451 with the notice, the gallery drops it, only the author can appeal, only an admin can restore or deny, a listing-scope block keeps the bytes and a bytes-scope block says so on restore");
