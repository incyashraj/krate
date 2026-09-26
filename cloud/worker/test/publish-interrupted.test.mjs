/* A publish interrupted after its bytes are public never leaves an app
 * nobody owns (IC-833, test 1838).
 *
 * The bundle is served at /a/<hash> the moment the store holds it. The
 * listing, written after, was the only record of who published it -- so a
 * KV write that failed, a client that went away or an isolate that died in
 * between left an app anyone could download and nobody could remove, its
 * author included: unpublish answered "not listed". Each case below stops
 * the publish at a different point and asks the one question that matters
 * afterwards: can the author still remove what is public?
 *
 * Driven against the real handler with a committed bundle.
 *
 *   node cloud/worker/test/publish-interrupted.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock, sha256Hex } from "./r2-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const APP = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));
const HASH = await sha256Hex(APP);

/* A KV whose writes can be made to fail by key prefix, and an R2 whose put
 * can be made to store and then throw -- the shape of an isolate that died
 * after the bytes landed. */
function env({ failPut = () => false, storeThenThrow = false } = {}) {
  const kv = new Map([
    ["session:krs_alice", "u1"], ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
    ["session:krs_bob", "u2"], ["user:u2", JSON.stringify({ login: "bob", name: "Bob", avatar_url: "" })],
  ]);
  const bundles = r2Mock();
  if (storeThenThrow) {
    const put = bundles.put;
    bundles.put = async (k, v, opts) => {
      await put(k, v, opts);
      throw new Error("the isolate went away after the write landed");
    };
  }
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => {
        if (failPut(k)) throw new Error(`KV: write refused for ${k}`);
        kv.set(k, v);
      },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    BUNDLES: bundles,
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}

const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
const publish = (e, who = "alice") =>
  worker.fetch(req("/publish", { method: "POST", headers: { authorization: `Bearer krs_${who}`, "x-krate-name": "Bounce" }, body: APP }), e);
const remove = (e, who) =>
  worker.fetch(req(`/app/${HASH}`, { method: "DELETE", headers: { authorization: `Bearer krs_${who}` } }), e);
const isPublic = async (e) => (await worker.fetch(req(`/a/${HASH}`), e)).status === 200;

/* ---- the listing write fails after the bytes are public ---------------- */
{
  const e = env({ failPut: (k) => k.startsWith("app:") });
  const res = await publish(e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  assert.match(await res.text(), /listing is delayed/, "the person is told what degraded");
  assert.ok(await isPublic(e), "the fixture must really leave the app public with no listing");
  assert.strictEqual(e._kv.get(`app:${HASH}`), undefined, "and really with no listing");

  assert.strictEqual((await remove(e, "bob")).status, 403, "somebody else still cannot remove it");
  const own = await remove(e, "alice");
  assert.strictEqual(own.status, 200, `its author can remove it: ${await own.clone().text()}`);
  assert.ok(!(await isPublic(e)), "and then it is gone");
  assert.strictEqual(e._kv.get(`owner:${HASH}`), undefined, "with its ownership record");
}

/* ---- someone re-posts the bytes while the listing is missing ----------- */
{
  const e = env({ failPut: (k) => k.startsWith("app:") });
  assert.strictEqual((await publish(e, "alice")).status, 200);
  const theirs = await publish(e, "bob");
  assert.match(await theirs.text(), /already published/, "the author does not transfer");
  assert.strictEqual(JSON.parse(e._kv.get(`owner:${HASH}`)).login, "alice");
  assert.strictEqual((await remove(e, "bob")).status, 403, "so bob still cannot remove it");
}

/* ---- the ownership write itself fails: nothing becomes public ---------- */
{
  const e = env({ failPut: (k) => k.startsWith("owner:") });
  const res = await publish(e);
  assert.strictEqual(res.status, 503, await res.clone().text());
  assert.match(await res.text(), /nothing was stored/);
  assert.ok(!(await isPublic(e)), "no bytes without a recorded owner");
  assert.strictEqual(e.BUNDLES._blobs.has(HASH), false);
}

/* ---- the store writes and then the isolate dies ------------------------ */
{
  const e = env({ storeThenThrow: true });
  const res = await publish(e);
  assert.strictEqual(res.status, 500, "the publish did not report success");
  assert.ok(await isPublic(e), "the fixture must really leave the bytes behind");
  assert.strictEqual(JSON.parse(e._kv.get(`owner:${HASH}`)).login, "alice", "and they are still owned");
  assert.strictEqual((await remove(e, "alice")).status, 200, "so their author can remove them");
  assert.ok(!(await isPublic(e)));
}

console.log("OK -- an interrupted publish always leaves an owner who can remove what is public");
