/* The publish door judges every upload with the Rust validator compiled to
 * WebAssembly (IC-833 tests 1832-1836; K-309): the manifest is parsed by
 * the crate's own parser, the component is checked against its declared
 * world, a signature is verified over the recomputed statement, and what
 * the listing records -- permissions, identities, release -- is what the
 * validator concluded.
 *
 * Needs cloud/worker/src/validator.wasm: scripts/build-hub-validator.sh.
 *
 *   node --experimental-wasm-modules cloud/worker/test/validator.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker, { judgeBundle } from "../src/index.js";
import { r2Mock, sha256Hex } from "./r2-mock.mjs";
import { patchStored } from "./zip-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const BOUNCE = new Uint8Array(readFileSync(join(root, "evidence", "ported", "bounce.krate")));
const SIGNED = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-signed.krate")));
const TAMPERED = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-tampered.krate")));
const MODULE = new Uint8Array(readFileSync(join(here, "fixtures", "module-not-component.krate")));

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
    BUNDLES: r2Mock(),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}
const publish = (bytes, extra = {}) =>
  new Request("https://hub.example/publish", {
    method: "POST",
    headers: { authorization: "Bearer krs_alice", "x-krate-name": "Bounce", ...extra },
    body: bytes,
  });

/* ---- the validator itself, through the glue ---------------------------- */
{
  const answer = judgeBundle(BOUNCE);
  assert.strictEqual(answer.ok, true, JSON.stringify(answer));
  const j = answer.judgement;
  assert.strictEqual(j.manifest.app.id, "dev.krate.bounce");
  assert.strictEqual(j.archive, await sha256Hex(BOUNCE), "the archive identity is the plain sha256 the hub keys on");
  assert.strictEqual(j.signature.state, "absent");
  assert.strictEqual(j.validator, 1, "the validator names its own version");
  assert.match(j.wit, /^[0-9a-f]{64}$/, "and the WIT it judged against");
  // Two calls, same answer: the module's memory is managed, not leaked
  // into a different result.
  assert.deepStrictEqual(judgeBundle(BOUNCE), answer);
}

/* ---- the listing records what the validator concluded (1835) ----------- */
{
  const e = env();
  const res = await worker.fetch(publish(BOUNCE, { "x-krate-capabilities": "[]" }), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  const { id } = JSON.parse(await res.text());
  const meta = JSON.parse(e._kv.get(`app:${id}`));
  assert.deepStrictEqual(meta.capabilities.map((c) => c.cap), ["ui.window:create", "io.stdout", "io.args"], "permissions come from the crate's own manifest parser");
  assert.strictEqual(meta.capabilities[0].rationale, "Open the animation window");
  assert.strictEqual(meta.identity.archive, id, "the listing's archive identity is the store key");
  assert.match(meta.identity.execution, /^[0-9a-f]{64}$/);
  assert.strictEqual(meta.signature.state, "absent");
  assert.strictEqual(meta.release, null, "unsigned: no release");
}

/* ---- a signed bundle: the signature is verified here, and the release recorded */
{
  const e = env();
  const res = await worker.fetch(publish(SIGNED), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  const { id } = JSON.parse(await res.text());
  const meta = JSON.parse(e._kv.get(`app:${id}`));
  assert.strictEqual(meta.signature.state, "valid");
  assert.match(meta.signature.public_key, /^[0-9a-f]{64}$/);
  assert.match(meta.release.id, /^[0-9a-f]{64}$/, "a verifying signature is a release the hub can name");
  assert.strictEqual(meta.release.version, "1.0.0");
  // The channel's move carries the release id (470).
  const channel = JSON.parse(e._kv.get("channel:alice/bounce"));
  assert.strictEqual(channel.history.at(-1).release, meta.release.id);
}

/* ---- a signed bundle changed after signing is refused ------------------ */
{
  const e = env();
  const res = await worker.fetch(publish(TAMPERED), e);
  assert.strictEqual(res.status, 422, await res.clone().text());
  assert.match(await res.text(), /signature that does not verify \(tampered\)/);
  assert.strictEqual(e.BUNDLES._blobs.size, 0, "nothing stored");
}

/* ---- the component is judged, not just found (1834) -------------------- */
{
  const e = env();
  const res = await worker.fetch(publish(MODULE), e);
  assert.strictEqual(res.status, 422, await res.clone().text());
  assert.match(await res.text(), /component/i, "a core module where a component belongs is refused for what it is");
}

/* ---- a manifest the crate's parser rejects is refused in its words ----- */
{
  const e = env();
  // Take the stored fixture used elsewhere and break its [app] table.
  const stored = patchStored(new Uint8Array(readFileSync(join(here, "fixtures", "bounce-stored.krate"))), "manifest.toml", (content) => {
    const text = new TextDecoder("latin1").decode(content);
    const at = text.indexOf("[app]");
    assert.ok(at >= 0);
    content.set(new TextEncoder().encode("[xyz]"), at);
    return content;
  });
  const res = await worker.fetch(publish(stored), e);
  assert.strictEqual(res.status, 422, await res.clone().text());
  const why = await res.text();
  assert.match(why, /manifest/, "refused as a manifest problem");
  assert.doesNotMatch(why, /checksum/i, "for the manifest, not for a checksum the patch broke");
}

console.log("OK -- the publish door runs the Rust validator: manifest, component, ceilings, identities and signature are judged by the same code krate run uses, and the listing records what it concluded");
