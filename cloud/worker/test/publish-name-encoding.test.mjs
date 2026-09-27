/* A name outside ISO-8859-1 can be published (K-891).
 *
 * A header carries ISO-8859-1 at most. A browser refused to send "Mom’s"
 * or "番茄钟" at all, and a CLI's raw UTF-8 arrived garbled. Senders now
 * percent-encode the value and say so with `x-krate-encoding: uri`; the
 * hub decodes only then, so an older client's plain header means what it
 * always meant -- "100% Tips" stays "100% Tips".
 *
 *   node --experimental-wasm-modules cloud/worker/test/publish-name-encoding.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const BOUNCE = new Uint8Array(readFileSync(join(root, "evidence", "ported", "bounce.krate")));

function env() {
  const kv = new Map([
    ["session:krs_alice", "u1"],
    ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async () => ({ keys: [], list_complete: true }),
    },
    BUNDLES: r2Mock(new Map()),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}

async function publishedName(headers) {
  const e = env();
  const res = await worker.fetch(new Request("https://hub.example/publish", {
    method: "POST",
    headers: { authorization: "Bearer krs_alice", "content-type": "application/octet-stream", ...headers },
    body: BOUNCE,
  }), e, {});
  assert.strictEqual(res.status, 200, await res.clone().text());
  const { id } = JSON.parse(await res.text());
  const listing = JSON.parse(e._kv.get(`app:${id}`));
  return { name: listing.name, description: listing.description };
}

const typed = "Mom’s 番茄钟 🍅";
const got = await publishedName({
  "x-krate-encoding": "uri",
  "x-krate-name": encodeURIComponent(typed),
  "x-krate-description": encodeURIComponent("كل يوم"),
});
assert.strictEqual(got.name, typed, "the name arrives as typed");
assert.strictEqual(got.description, "كل يوم", "and so does the description");
console.log("ok  a name outside ISO-8859-1 is published as typed");

const old = await publishedName({ "x-krate-name": "100% Tips" });
assert.strictEqual(old.name, "100% Tips", "a plain header from an older client is taken as sent");
console.log("ok  an older client's plain header is unchanged");

const broken = await publishedName({ "x-krate-encoding": "uri", "x-krate-name": "50%zz off" });
assert.strictEqual(broken.name, "50%zz off", "a malformed encoding falls back to the raw value, not an error");
console.log("ok  a malformed encoding is not an error");
