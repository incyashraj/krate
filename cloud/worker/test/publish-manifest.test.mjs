/* The permissions a listing shows come from the bundle, not from a header
 * (IC-669, test 1310).
 *
 * The CLI sends `x-krate-capabilities`, derived from the manifest, and the
 * hub stored whatever arrived. A dishonest client could list a microphone
 * app as asking for nothing, and the same author could re-POST identical
 * bytes with a different header and rewrite the permissions on the page
 * without changing a byte of the app -- a metadata-only update that changes
 * what the person is told.
 *
 * The hub now reads manifest.toml out of the archive it already holds. This
 * drives the real handler with a real committed bundle, so the zip reader
 * is exercised on bytes the CLI actually wrote, not on a fixture shaped to
 * pass. A stored (uncompressed) variant covers the other method.
 *
 *   node cloud/worker/test/publish-manifest.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";
import { patchStored } from "./zip-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const BOUNCE = new Uint8Array(readFileSync(join(root, "evidence", "ported", "bounce.krate")));
const STORED = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-stored.krate")));

function env() {
  const kv = new Map([
    ["session:krs_alice", "u1"],
    ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
  ]);
  const blobs = new Map();
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async () => ({ keys: [], list_complete: true }),
    },
    BUNDLES: r2Mock(blobs),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}

function publish(bytes, extra = {}) {
  return new Request("https://hub.example/publish", {
    method: "POST",
    headers: { authorization: "Bearer krs_alice", "content-type": "application/octet-stream", ...extra },
    body: bytes,
  });
}

async function listing(e, res) {
  const { id } = JSON.parse(await res.text());
  return JSON.parse(e._kv.get(`app:${id}`));
}

const MANIFEST_CAPS = ["ui.window:create", "io.stdout", "io.args"];

/* ---- a lying header does not change what the listing shows ------------- */
{
  const e = env();
  const res = await worker.fetch(publish(BOUNCE, { "x-krate-capabilities": "[]" }), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  const meta = await listing(e, res);
  assert.deepStrictEqual(
    meta.capabilities.map((c) => c.cap),
    MANIFEST_CAPS,
    "the listing must show what the manifest declares, not what the header said",
  );
  assert.strictEqual(meta.capabilities[0].rationale, "Open the animation window");
  assert.strictEqual(meta.capabilities[0].required, true);
}

/* ---- and a header claiming MORE does not add anything either ----------- */
{
  const e = env();
  const lie = JSON.stringify([{ cap: "audio.capture", rationale: "x", required: true }]);
  const res = await worker.fetch(publish(BOUNCE, { "x-krate-capabilities": lie }), e);
  const meta = await listing(e, res);
  assert.ok(!meta.capabilities.some((c) => c.cap === "audio.capture"), "a header cannot invent a permission");
}

/* ---- a same-author re-POST cannot rewrite the permissions -------------- */
{
  const e = env();
  await worker.fetch(publish(BOUNCE), e);
  const res = await worker.fetch(publish(BOUNCE, { "x-krate-capabilities": "[]" }), e);
  const meta = await listing(e, res);
  assert.deepStrictEqual(meta.capabilities.map((c) => c.cap), MANIFEST_CAPS, "a metadata-only re-POST changed the permissions shown (1310)");
}

/* ---- the stored (uncompressed) method reads too ------------------------- */
{
  const e = env();
  const res = await worker.fetch(publish(STORED), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  const meta = await listing(e, res);
  assert.deepStrictEqual(meta.capabilities.map((c) => c.cap), MANIFEST_CAPS, "a stored manifest entry must read the same");
}

/* ---- the words alone no longer get past the door (K-309) ---------------- */
{
  const e = env();
  const words = new TextEncoder().encode("PK\x03\x04 manifest.toml code.wasm and nothing that is an archive");
  const res = await worker.fetch(publish(words), e);
  assert.strictEqual(res.status, 422, await res.clone().text());
  assert.match(await res.text(), /manifest\.toml/);
}

/* ---- a real archive whose manifest has no [app] table is refused ------- */
{
  // Take the stored fixture and overwrite the manifest bytes in place with
  // same-length text that has no [app] table. Stored means no CRC-checked
  // inflate stands in the way, so the reader sees exactly this text.
  const e = env();
  const bytes = patchStored(STORED, "manifest.toml", (content) => {
    const text = new TextDecoder("latin1").decode(content);
    const at = text.indexOf("[app]");
    assert.ok(at >= 0, "the fixture carries an [app] table to break");
    content.set(new TextEncoder().encode("[xyz]"), at);
    return content;
  });
  const res = await worker.fetch(publish(bytes), e);
  assert.strictEqual(res.status, 422, await res.clone().text());
  assert.match(await res.text(), /manifest/, "refused as a manifest problem, in the parser's words");
}

/* ---- an archive with a manifest but no code.wasm entry is refused ------ */
{
  const e = env();
  const bytes = new Uint8Array(STORED);
  const text = new TextDecoder("latin1").decode(bytes);
  // Rename the entry NAMES (local header and central directory) to
  // "code.wash", but leave the manifest's own `entry = "code.wasm"` line
  // alone. The first version renamed that too, so the old substring gate
  // refused the body before the directory check could, and a sabotage
  // that removed the directory check survived on the older message.
  let idx = text.indexOf("code.wasm");
  assert.ok(idx > 0);
  let renamed = 0;
  while (idx >= 0) {
    if (text.slice(idx - 9, idx) !== 'entry = "') {
      bytes.set(new TextEncoder().encode("code.wash"), idx);
      renamed += 1;
    }
    idx = text.indexOf("code.wasm", idx + 1);
  }
  assert.strictEqual(renamed, 2, "the local header and the central directory name were renamed");
  assert.ok(new TextDecoder("latin1").decode(bytes).includes('entry = "code.wasm"'), "the manifest line still says code.wasm");
  const res = await worker.fetch(publish(bytes), e);
  assert.strictEqual(res.status, 422, await res.clone().text());
  assert.match(await res.text(), /code\.wasm/);
}

console.log(
  "OK -- the listing's permissions are read from the bundle's own manifest: a lying header changes nothing, " +
    "a same-author re-POST cannot rewrite them, both zip methods read, and a body that is not a real archive is refused",
);
