/* A channel is the hub's one moving name, and it moves only for its
 * publisher, through an audited update (IC-389, tests 470, 471, 476;
 * K-308).
 *
 * Driven against the real handler with two real committed bundles that
 * carry the same app under different bytes.
 *
 *   node cloud/worker/test/channel.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

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
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}

const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
async function publish(e, who, bytes, name, extra = {}) {
  const res = await worker.fetch(
    req("/publish", { method: "POST", headers: { authorization: `Bearer krs_${who}`, "x-krate-name": name, ...extra }, body: bytes }),
    e,
  );
  assert.strictEqual(res.status, 200, await res.clone().text());
  return JSON.parse(await res.text());
}
const channel = async (e, path) => {
  const res = await worker.fetch(req(path), e);
  return { status: res.status, body: res.status === 200 ? JSON.parse(await res.text()) : await res.text(), headers: res.headers };
};

/* ---- a publish names its channel, and the channel resolves ------------ */
const e = env();
const first = await publish(e, "alice", V1, "My Notes");
assert.strictEqual(first.channel, "https://hub.example/c/alice/my-notes", "the publish response names the channel");
let c = await channel(e, "/c/alice/my-notes");
assert.strictEqual(c.status, 200);
assert.strictEqual(c.body.current, first.id, "the channel names the release just published");
assert.strictEqual(c.body.url, `https://hub.example/a/${first.id}`);
assert.strictEqual(c.body.history.length, 1);
assert.strictEqual(c.body.history[0].previous, null, "the first move replaced nothing");

/* ---- the same bytes again move nothing ---------------------------------- */
await publish(e, "alice", V1, "My Notes");
c = await channel(e, "/c/alice/my-notes");
assert.strictEqual(c.body.history.length, 1, "republishing identical bytes is not a move");

/* ---- a new release moves it, and the move records what it replaced (470) */
const second = await publish(e, "alice", V2, "My Notes");
assert.notStrictEqual(second.id, first.id, "the fixtures must differ or this proves nothing");
c = await channel(e, "/c/alice/my-notes");
assert.strictEqual(c.body.current, second.id);
assert.deepStrictEqual(c.body.history.map((m) => [m.hash, m.previous]), [[first.id, null], [second.id, first.id]], "every move names what it replaced");

/* ---- the client never takes bytes from a channel: it is sent to the fixed address (471) */
const dl = await worker.fetch(req("/c/alice/my-notes?dl=1"), e);
assert.strictEqual(dl.status, 302);
assert.strictEqual(dl.headers.get("location"), `https://hub.example/a/${second.id}?dl=1`, "a download is a redirect to the address the channel currently names");
assert.strictEqual(dl.headers.get("cache-control"), "no-store", "a channel resolution is never cached");

/* ---- nobody else can move it ------------------------------------------- */
{
  const e2 = env();
  const a = await publish(e2, "alice", V1, "My Notes");
  const bobs = await publish(e2, "bob", V2, "My Notes");
  assert.strictEqual(bobs.channel, "https://hub.example/c/bob/my-notes", "bob's same-named app is bob's channel");
  assert.strictEqual((await channel(e2, "/c/alice/my-notes")).body.current, a.id, "bob publishing the same name did not touch alice's channel");
  // Bob re-POSTing alice's exact bytes: the listing stays alice's (IC-387),
  // and so does her channel.
  await worker.fetch(req("/publish", { method: "POST", headers: { authorization: "Bearer krs_bob", "x-krate-name": "My Notes" }, body: V1 }), e2);
  assert.strictEqual((await channel(e2, "/c/alice/my-notes")).body.current, a.id, "a re-POST of her bytes by someone else moves nothing");
}

/* ---- an unlisted publish is not the app's public face ------------------ */
{
  const e2 = env();
  const quiet = await publish(e2, "alice", V1, "Secret Thing", { "x-krate-unlisted": "1" });
  assert.strictEqual(quiet.channel, undefined, "an unlisted publish names no channel");
  assert.strictEqual((await channel(e2, "/c/alice/secret-thing")).status, 404);
}

/* ---- a name never resolves to bytes now listed under someone else ------ */
{
  // Alice's old release was retired from the gallery when she published a
  // newer one; its bytes stay. Bob then publishes those exact bytes under
  // his own name, so the hash is listed as bob's. When alice's newer
  // release goes, her channel must not fall back to a hash that is now
  // bob's listing.
  const e2 = env();
  const a1 = await publish(e2, "alice", V1, "My Notes");
  const a2 = await publish(e2, "alice", V2, "My Notes");
  assert.strictEqual(await e2.APPS.get(`app:${a1.id}`), null, "the older listing was retired");
  const b = await publish(e2, "bob", V1, "Bobs Notes");
  assert.strictEqual(b.id, a1.id, "bob now lists alice's old bytes");
  await worker.fetch(req(`/app/${a2.id}`, { method: "DELETE", headers: { authorization: "Bearer krs_alice" } }), e2);
  assert.strictEqual((await channel(e2, "/c/alice/my-notes")).status, 404, "her channel is gone rather than pointing at bob's listing");
}

/* ---- a takedown shows on the resolution -------------------------------- */
{
  const e2 = env();
  e2.KRATE_ADMINS = "alice";
  const p = await publish(e2, "alice", V1, "Blocked");
  await worker.fetch(req(`/admin/takedown/${p.id}`, { method: "POST", headers: { authorization: "Bearer krs_alice", "content-type": "application/json" }, body: JSON.stringify({ reason: "x" }) }), e2);
  const r = await channel(e2, "/c/alice/blocked");
  assert.strictEqual(r.body.blocked, true, "the resolution says the current release is blocked");
}

/* ---- unpublishing the current release retreats the channel (470) ------- */
const un = await worker.fetch(req(`/app/${second.id}`, { method: "DELETE", headers: { authorization: "Bearer krs_alice" } }), e);
assert.strictEqual(un.status, 200, await un.text());
c = await channel(e, "/c/alice/my-notes");
assert.strictEqual(c.status, 200, "the earlier release's bytes and links were kept, so the name still resolves");
assert.strictEqual(c.body.current, first.id, "the channel falls back to the newest earlier release it can still name");
assert.strictEqual(c.body.history.at(-1).reason, "unpublished", "and the retreat is on the record");

/* ---- bytes gone by another door: the name resolves to nothing ---------- */
e.KRATE_ADMINS = "alice";
const purge = await worker.fetch(req(`/blob/${first.id}`, { method: "DELETE", headers: { authorization: "Bearer krs_alice" } }), e);
assert.strictEqual(purge.status, 200, await purge.text());
assert.strictEqual((await channel(e, "/c/alice/my-notes")).status, 404, "with the bytes purged the name resolves to nothing, never to gone bytes");
assert.strictEqual((await worker.fetch(req("/c/alice/my-notes?dl=1"), e)).status, 404, "and a download is not redirected to an address that is gone");

/* ---- shapes that are not channels -------------------------------------- */
for (const path of ["/c/alice", "/c/alice/my-notes/extra", "/c/../x", "/c/alice/My_Notes"]) {
  assert.strictEqual((await worker.fetch(req(path), e)).status, 404, `${path} is not a channel`);
}

console.log("OK -- a channel is named by its publish, moves only through that publisher's listed publishes with every move recorded, sends downloads to the fixed address it names, retreats on unpublish, and resolves to nothing rather than to gone bytes");
