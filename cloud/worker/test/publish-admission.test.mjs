/* The publish door applies the archive contract, bounds what it reads, and
 * reads the store back before saying "published" (IC-833, tests 1831,
 * 1833, 1837, 1840; K-309).
 *
 * Every archive here is built by hand -- local headers, central directory,
 * end-of-central-directory -- because the shapes that matter (a name twice,
 * a count the directory does not carry, an encrypted flag, a path with
 * `..`) are exactly the ones a real zip library refuses to write. The real
 * handler is driven with a real session and a real R2-shaped mock.
 *
 *   node cloud/worker/test/publish-admission.test.mjs
 */
import assert from "node:assert";
import { deflateRawSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock, sha256Hex } from "./r2-mock.mjs";
import { crc32 } from "./zip-tools.mjs";

const MANIFEST = new TextEncoder().encode(
  '[app]\nid = "dev.krate.door"\nname = "Door"\nversion = "0.1.0"\nentry = "code.wasm"\nworld = "krate:app/cli@0.1.0"\n',
);
// A real component with a `run` export: the validator judges the component
// against the declared world, so eight header bytes no longer pass.
const WASM = new Uint8Array(readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "crates", "bundle", "tests", "fixtures", "minimal-run.wasm")));

function env(bundles = r2Mock()) {
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
    BUNDLES: bundles,
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}

const publish = (body, extra = {}) =>
  new Request("https://hub.example/publish", {
    method: "POST",
    headers: { authorization: "Bearer krs_alice", "content-type": "application/octet-stream" },
    body,
    ...extra,
  });

/* ---- a zip writer that writes whatever it is told ----------------------
 * entries: [{ name, data, deflate?, size?, flags? }]. `size` overrides the
 * declared uncompressed size, `flags` the general-purpose bits, and
 * `declaredCount` the EOCD's record count. */
function zip(entries, { declaredCount } = {}) {
  const enc = new TextEncoder();
  const parts = [];
  const central = [];
  let offset = 0;
  const le16 = (n) => [n & 0xff, (n >> 8) & 0xff];
  const le32 = (n) => [n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff, (n >>> 24) & 0xff];
  for (const e of entries) {
    const name = enc.encode(e.name);
    const stored = e.deflate ? new Uint8Array(deflateRawSync(e.data)) : e.data;
    const size = e.size ?? e.data.byteLength;
    const method = e.deflate ? 8 : 0;
    const flags = e.flags ?? 0;
    // A real CRC over the uncompressed data: the validator checks it, and
    // a zero would be refused as a checksum before any rule was reached.
    const crc = crc32(e.data) >>> 0;
    const local = new Uint8Array([
      ...le32(0x04034b50), ...le16(20), ...le16(flags), ...le16(method), ...le16(0), ...le16(0),
      ...le32(crc), ...le32(stored.byteLength), ...le32(size), ...le16(name.length), ...le16(0),
      ...name,
    ]);
    central.push(
      new Uint8Array([
        ...le32(0x02014b50), ...le16(20), ...le16(20), ...le16(flags), ...le16(method), ...le16(0), ...le16(0),
        ...le32(crc), ...le32(stored.byteLength), ...le32(size), ...le16(name.length), ...le16(0), ...le16(0),
        ...le16(0), ...le16(0), ...le32(0), ...le32(offset), ...name,
      ]),
    );
    parts.push(local, stored);
    offset += local.byteLength + stored.byteLength;
  }
  const cdSize = central.reduce((n, c) => n + c.byteLength, 0);
  const count = declaredCount ?? entries.length;
  const eocd = new Uint8Array([
    ...le32(0x06054b50), ...le16(0), ...le16(0), ...le16(count), ...le16(count),
    ...le32(cdSize), ...le32(offset), ...le16(0),
  ]);
  const all = [...parts, ...central, eocd];
  const out = new Uint8Array(all.reduce((n, p) => n + p.byteLength, 0));
  let at = 0;
  for (const p of all) { out.set(p, at); at += p.byteLength; }
  return out;
}

const good = () => [
  { name: "manifest.toml", data: MANIFEST, deflate: true },
  { name: "code.wasm", data: WASM },
];

/* ---- a well-formed hand-built archive is admitted, with a receipt ------ */
{
  const e = env();
  const bytes = zip(good());
  const res = await worker.fetch(publish(bytes), e);
  assert.strictEqual(res.status, 200, await res.clone().text());
  const receipt = JSON.parse(await res.text());
  assert.strictEqual(receipt.id, await sha256Hex(bytes));
  assert.deepStrictEqual(
    receipt.stored,
    { size: bytes.byteLength, sha256: receipt.id },
    "the receipt says what the store holds, read back (1840)",
  );
}

/* ---- the door: each shape refused for its own named reason (1833) ------ */
const refused = [
  ["a name twice", zip([...good(), { name: "code.wasm", data: WASM }]), /same file twice: code\.wasm/],
  ["a case alias of a name", zip([...good(), { name: "Code.WASM", data: WASM }]), /same file twice/],
  ["a count the directory does not carry", zip(good(), { declaredCount: 3 }), /declares 3 entries but only 2/],
  ["a traversal path", zip([...good(), { name: "assets/../../etc/x", data: WASM }]), /unsafe entry path/],
  ["a backslash separator", zip([...good(), { name: "assets\\x.png", data: WASM }]), /unsafe entry path/],
  ["an absolute path", zip([...good(), { name: "/etc/passwd", data: WASM }]), /unsafe entry path/],
  ["a drive letter", zip([...good(), { name: "C:/x", data: WASM }]), /unsafe entry path/],
  ["an empty segment", zip([...good(), { name: "assets//x", data: WASM }]), /unsafe entry path/],
  ["a non-ASCII name", zip([...good(), { name: "assets/caf\u00e9.png", data: WASM }]), /not plain ASCII/],
  ["an encrypted entry", zip([...good(), { name: "assets/x", data: WASM, flags: 1 }]), /encrypted entry/],
  ["a path too deep", zip([...good(), { name: `source/${"d/".repeat(17)}f`, data: WASM }]), /nests deeper/],
  ["a path too long", zip([...good(), { name: `source/${"a".repeat(200)}`, data: WASM }]), /longer than/],
  ["an entry declaring 600 MiB", zip([...good(), { name: "assets/big", data: WASM, size: 600 * 1024 * 1024 }]), /declares more than/],
  ["entries declaring 1.5 GiB together", zip([...good(), { name: "assets/a", data: WASM, size: 500 * 1024 * 1024 }, { name: "assets/b", data: WASM, size: 500 * 1024 * 1024 }, { name: "assets/c", data: WASM, size: 500 * 1024 * 1024 }]), /together/],
  ["no central directory at all", new TextEncoder().encode("PK\x03\x04 manifest.toml code.wasm words"), /no central directory/],
  ["a manifest that inflates far past its declared size", zip([{ name: "manifest.toml", data: new Uint8Array(4 * 1024 * 1024), deflate: true, size: 1 }, { name: "code.wasm", data: WASM }]), /not a valid \.krate bundle/],
];
for (const [what, bytes, why] of refused) {
  const e = env();
  const res = await worker.fetch(publish(bytes), e);
  const body = await res.text();
  assert.strictEqual(res.status, 422, `${what} must be refused at the door: ${res.status} ${body}`);
  assert.match(body, why, `${what} must be refused for its own reason: ${body}`);
  assert.strictEqual(e.BUNDLES._blobs.size, 0, `${what} must not reach the store`);
  assert.ok(![...e._kv.keys()].some((k) => k.startsWith("app:")), `${what} must not be listed`);
}

/* ---- the bound is on bytes as they stream, not after they land (1831) --- */
{
  const CHUNK = 256 * 1024;
  const TOTAL = 40 * 1024 * 1024;
  let pulls = 0;
  const stream = new ReadableStream({
    pull(c) {
      if (pulls * CHUNK >= TOTAL) { c.close(); return; }
      pulls += 1;
      c.enqueue(new Uint8Array(CHUNK));
    },
  });
  const res = await worker.fetch(publish(stream, { duplex: "half" }), env());
  assert.strictEqual(res.status, 413, await res.text());
  await new Promise((r) => setTimeout(r, 20));
  assert.ok(
    pulls < TOTAL / CHUNK,
    `an oversized upload must be refused before it is all read: ${pulls} of ${TOTAL / CHUNK} chunks were pulled`,
  );
  assert.ok(pulls * CHUNK <= 5 * 1024 * 1024 + 2 * CHUNK, `only the ceiling plus a chunk is ever read: ${pulls} chunks`);
}

/* ---- the store is never handed bytes that do not match the key (1837) --- */
{
  const bytes = zip(good());
  const hash = await sha256Hex(bytes);
  // The store already holds DIFFERENT bytes at this address.
  const blobs = new Map([[hash, new Uint8Array([1, 2, 3])]]);
  const e = env(r2Mock(blobs));
  const res = await worker.fetch(publish(bytes), e);
  assert.strictEqual(res.status, 500, await res.clone().text());
  assert.match(await res.text(), /not overwriting/);
  assert.deepStrictEqual([...blobs.get(hash)], [1, 2, 3], "the wrong bytes were not replaced -- that is a store to inspect, not to paper over");
  assert.ok(!e._kv.has(`app:${hash}`), "nothing is listed over a store that disagrees");
}
{
  // The store itself refuses bytes whose digest is not the one named. A
  // put that lies about its digest cannot get past R2, and the handler
  // reports it rather than listing an object that does not exist.
  const bytes = zip(good());
  const bundles = r2Mock();
  const honest = bundles.put;
  bundles.put = (k, v, opts) => honest(k, v, { ...opts, sha256: "0".repeat(64) });
  const e = env(bundles);
  const res = await worker.fetch(publish(bytes), e);
  assert.strictEqual(res.status, 500, await res.clone().text());
  assert.match(await res.text(), /store refused/);
}

/* ---- an object stored before digests were recorded is upgraded --------- */
{
  const bytes = zip(good());
  const hash = await sha256Hex(bytes);
  const blobs = new Map([[hash, bytes], [`${hash}\u0000meta`, { sha256: false }]]);
  const bundles = r2Mock(blobs);
  let puts = 0;
  const honest = bundles.put;
  bundles.put = (k, v, opts) => { puts += 1; assert.strictEqual(opts.sha256, hash, "the upgrade names the digest"); return honest(k, v, opts); };
  const res = await worker.fetch(publish(bytes), env(bundles));
  assert.strictEqual(res.status, 200, await res.clone().text());
  assert.strictEqual(puts, 1, "a legacy object without a recorded digest is written again under the guard");
}

/* ---- a receipt that disagrees with what was sent fails the publish (1840) */
{
  const bytes = zip(good());
  const hash = await sha256Hex(bytes);
  const bundles = r2Mock();
  const honestHead = bundles.head;
  let heads = 0;
  bundles.head = async (k) => {
    const o = await honestHead(k);
    heads += 1;
    // The read-back after the put reports one byte short.
    return o && heads >= 2 ? { ...o, size: o.size - 1 } : o;
  };
  const e = env(bundles);
  const res = await worker.fetch(publish(bytes), e);
  assert.strictEqual(res.status, 500, await res.clone().text());
  assert.match(await res.text(), /could not be verified/);
  assert.ok(!bundles._blobs.has(hash), "an object that could not be verified is removed");
  assert.ok(!e._kv.has(`app:${hash}`), "and never listed");
}

/* ---- and identical bytes twice are one object, one listing ------------- */
{
  const bytes = zip(good());
  const e = env();
  const first = JSON.parse(await (await worker.fetch(publish(bytes), e)).text());
  const second = JSON.parse(await (await worker.fetch(publish(bytes), e)).text());
  assert.strictEqual(first.id, second.id);
  assert.deepStrictEqual(first.stored, second.stored, "a byte-identical retry reads back the same receipt");
  assert.strictEqual(e.BUNDLES._blobs.size, 1);
}

console.log(
  "OK -- the publish door judges the whole central directory before reading an entry, bounds the body and " +
    "the manifest inflate on bytes as they arrive, hands the store a digest it must match, never overwrites " +
    "a disagreeing object, and reads the stored copy back before reporting a receipt",
);
