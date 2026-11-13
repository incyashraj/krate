/* Signed .krate bundles made in the test, with whatever source a test
 * needs (G15): the real component and manifest of the org fixture, the
 * caller's `source/` files and capabilities, zipped here and marked and
 * signed for the organisation by the validator's own signing code (the
 * code a browser signs with), under a root key made for the test.
 */
import { generateKeyPairSync } from "node:crypto";
import { deflateRawSync, inflateRawSync, crc32 } from "node:zlib";
import * as validator from "../src/validator.wasm";
import { fixture } from "./org-tools.mjs";
import { directory } from "./zip-tools.mjs";

const hex = (b) => Buffer.from(b).toString("hex");

/// A root key for the test: {public, seed} as hex.
export function rootKey() {
  const { privateKey } = generateKeyPairSync("ed25519");
  const jwk = privateKey.export({ format: "jwk" });
  return { public: hex(Buffer.from(jwk.x, "base64url")), seed: hex(Buffer.from(jwk.d, "base64url")) };
}

/// A zip of `entries` ({name: Uint8Array | string}), deflated.
export function zip(entries) {
  const local = [];
  const central = [];
  let at = 0;
  for (const [name, raw] of Object.entries(entries)) {
    const data = typeof raw === "string" ? new TextEncoder().encode(raw) : raw;
    const packed = deflateRawSync(data);
    const n = Buffer.from(name);
    const crc = crc32(data) >>> 0;
    const lh = Buffer.alloc(30);
    lh.writeUInt32LE(0x04034b50, 0); lh.writeUInt16LE(20, 4); lh.writeUInt16LE(0, 6); lh.writeUInt16LE(8, 8);
    lh.writeUInt32LE(0, 10); lh.writeUInt32LE(crc, 14); lh.writeUInt32LE(packed.length, 18); lh.writeUInt32LE(data.length, 22);
    lh.writeUInt16LE(n.length, 26); lh.writeUInt16LE(0, 28);
    local.push(lh, n, packed);
    const ch = Buffer.alloc(46);
    ch.writeUInt32LE(0x02014b50, 0); ch.writeUInt16LE(20, 4); ch.writeUInt16LE(20, 6); ch.writeUInt16LE(0, 8); ch.writeUInt16LE(8, 10);
    ch.writeUInt32LE(0, 12); ch.writeUInt32LE(crc, 16); ch.writeUInt32LE(packed.length, 20); ch.writeUInt32LE(data.length, 24);
    ch.writeUInt16LE(n.length, 28); ch.writeUInt16LE(0, 30); ch.writeUInt16LE(0, 32); ch.writeUInt16LE(0, 34); ch.writeUInt16LE(0, 36);
    ch.writeUInt32LE(0, 38); ch.writeUInt32LE(at, 42);
    central.push(ch, n);
    at += 30 + n.length + packed.length;
  }
  const cd = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(Object.keys(entries).length, 8); end.writeUInt16LE(Object.keys(entries).length, 10);
  end.writeUInt32LE(cd.length, 12); end.writeUInt32LE(at, 16);
  return new Uint8Array(Buffer.concat([...local, cd, end]));
}

/// One entry of a fixture bundle, inflated.
function entryOf(bytes, name) {
  const e = directory(bytes).find((x) => x.name === name);
  const lh = e.local;
  const start = lh + 30 + (bytes[lh + 26] | (bytes[lh + 27] << 8)) + (bytes[lh + 28] | (bytes[lh + 29] << 8));
  const size = bytes[e.record + 20] | (bytes[e.record + 21] << 8) | (bytes[e.record + 22] << 16) | (bytes[e.record + 23] << 24);
  const raw = bytes.subarray(start, start + size);
  return e.method === 0 ? raw : new Uint8Array(inflateRawSync(raw));
}

const BASE = fixture("candidate-v1.krate");
const MANIFEST = new TextDecoder().decode(entryOf(BASE, "manifest.toml"));
export const COMPONENT = entryOf(BASE, "code.wasm");

/// The fixture's manifest at `version`, with `caps` added ([cap, rationale]).
export function manifest(version, caps = []) {
  let m = MANIFEST.replace(/^version = ".*"$/m, `version = "${version}"`);
  for (const [cap, why] of caps) m += `\n[[capabilities]]\ncap = "${cap}"\nrationale = "${why}"\nrequired = true\n`;
  return m;
}

/// Mark and sign `bytes` for `org` with `key` (from rootKey()).
export function sign(bytes, key, org = "acme") {
  const head = new TextEncoder().encode(JSON.stringify({ org, signed_at: 1760000000, seed: key.seed, public: key.public }));
  const frame = new Uint8Array(4 + head.length + bytes.length);
  new DataView(frame.buffer).setUint32(0, head.length, true);
  frame.set(head, 4);
  frame.set(bytes, 4 + head.length);
  const input = validator.krate_alloc(frame.length);
  new Uint8Array(validator.memory.buffer, input, frame.length).set(frame);
  const out = validator.krate_sign(input, frame.length);
  const len = new DataView(validator.memory.buffer).getUint32(out, true);
  const body = new Uint8Array(validator.memory.buffer, out + 4, len).slice();
  validator.krate_free(out, 4 + len);
  validator.krate_free(input, frame.length);
  const jlen = new DataView(body.buffer).getUint32(0, true);
  const meta = JSON.parse(new TextDecoder().decode(body.subarray(4, 4 + jlen)));
  if (!meta.ok) throw new Error(`signing refused: ${meta.problem}`);
  return body.slice(4 + jlen);
}

/// A signed bundle: `version`, `source` ({path under source/: text}),
/// extra capabilities, and a component (the fixture's unless given).
export function bundle(key, { version = "1.0.0", source = {}, caps = [], component = COMPONENT, extra = {} } = {}) {
  const entries = { "manifest.toml": manifest(version, caps), "code.wasm": component };
  for (const [path, text] of Object.entries(source)) entries[`source/${path}`] = text;
  Object.assign(entries, extra);
  return sign(zip(entries), key);
}
