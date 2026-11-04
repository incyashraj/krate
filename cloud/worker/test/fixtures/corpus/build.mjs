/* The publish corpus (IC-833, tests 1821-1822): one file of each kind the
 * door must admit or refuse, written once and committed, with a record of
 * what each file is -- its exact bytes' SHA-256 and size, what its central
 * directory says it expands to, the manifest of the ones that are apps, and
 * the answer the door must give. The test (publish-corpus.test.mjs) checks
 * every committed file against that record and drives each through the real
 * door.
 *
 * Rebuild (only when a case is added; the bytes are the point):
 *   node cloud/worker/test/fixtures/corpus/build.mjs
 *
 * Entries are stored, never deflated, except the bomb, so a rebuild on
 * another zlib writes the same bytes; the bomb's bytes are the committed
 * file. The 5 MiB "too large" case is not committed: it is made at test
 * time from zeros, and its record says so. The format has no native
 * slices, so the corpus has none.
 */
import { writeFileSync, readFileSync } from "node:fs";
import { deflateRawSync } from "node:zlib";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { crc32, directory } from "../../zip-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const WASM = new Uint8Array(readFileSync(join(here, "..", "..", "..", "..", "..", "crates", "bundle", "tests", "fixtures", "minimal-run.wasm")));
const enc = new TextEncoder();
const manifest = (id, name, extra = "") => enc.encode(`[app]\nid = "${id}"\nname = "${name}"\nversion = "0.1.0"\nentry = "code.wasm"\nworld = "krate:app/cli@0.1.0"\n${extra}`);

export function zip(entries, { declaredCount } = {}) {
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
    const crc = crc32(e.data) >>> 0;
    const local = new Uint8Array([...le32(0x04034b50), ...le16(20), ...le16(0), ...le16(method), ...le16(0), ...le16(0), ...le32(crc), ...le32(stored.byteLength), ...le32(size), ...le16(name.length), ...le16(0), ...name]);
    central.push(new Uint8Array([...le32(0x02014b50), ...le16(20), ...le16(20), ...le16(0), ...le16(method), ...le16(0), ...le16(0), ...le32(crc), ...le32(stored.byteLength), ...le32(size), ...le16(name.length), ...le16(0), ...le16(0), ...le16(0), ...le16(0), ...le32(0), ...le32(offset), ...name]));
    parts.push(local, stored);
    offset += local.byteLength + stored.byteLength;
  }
  const cdSize = central.reduce((n, c) => n + c.byteLength, 0);
  const count = declaredCount ?? entries.length;
  const eocd = new Uint8Array([...le32(0x06054b50), ...le16(0), ...le16(0), ...le16(count), ...le16(count), ...le32(cdSize), ...le32(offset), ...le16(0)]);
  const all = [...parts, ...central, eocd];
  const out = new Uint8Array(all.reduce((n, p) => n + p.byteLength, 0));
  let at = 0;
  for (const p of all) { out.set(p, at); at += p.byteLength; }
  return out;
}

const app = (id, name, more = []) => [{ name: "manifest.toml", data: manifest(id, name) }, { name: "code.wasm", data: WASM }, ...more];
const picture = (n) => { const b = new Uint8Array(40_000); for (let i = 0; i < b.length; i++) b[i] = (i * 31 + n * 7) & 0xff; return b; };

/// Each case: file name, how it is made, and the door's answer.
export const CASES = [
  { file: "minimal.krate", kind: "valid minimal", make: () => zip(app("dev.krate.corpus.minimal", "Minimal")), status: 200 },
  { file: "source-carrying.krate", kind: "valid, carries its source", make: () => zip(app("dev.krate.corpus.source", "Source", [{ name: "source/Cargo.toml", data: enc.encode('[package]\nname = "corpus"\nversion = "0.1.0"\n') }, { name: "source/src/lib.rs", data: enc.encode("pub fn run() {}\n") }])), status: 200 },
  { file: "asset-heavy.krate", kind: "valid, asset-heavy (24 assets, ~1 MB)", make: () => zip(app("dev.krate.corpus.assets", "Assets", Array.from({ length: 24 }, (_, n) => ({ name: `assets/picture-${n}.bin`, data: picture(n) })))), status: 200 },
  { file: "duplicate.krate", kind: "a name twice", make: () => zip([...app("dev.krate.corpus.dup", "Dup"), { name: "code.wasm", data: WASM }]), status: 422, reason: "same file twice" },
  { file: "traversal.krate", kind: "a path that climbs out", make: () => zip([...app("dev.krate.corpus.trav", "Trav"), { name: "assets/../../etc/passwd", data: WASM }]), status: 422, reason: "unsafe entry path" },
  { file: "compression-bomb.krate", kind: "an entry inflating far past its declared size", make: () => zip([{ name: "manifest.toml", data: new Uint8Array(4 * 1024 * 1024), deflate: true, size: 1 }, { name: "code.wasm", data: WASM }]), status: 422, reason: "not a valid .krate bundle" },
  { file: "malformed-marker.krate", kind: "a zip marker and two file names, no archive", make: () => enc.encode("PK\x03\x04 manifest.toml code.wasm words"), status: 422, reason: "no central directory" },
  { file: "empty.krate", kind: "no bytes", make: () => new Uint8Array(0), status: 400, reason: "empty body" },
  { file: "no-component.krate", kind: "a manifest and no code", make: () => zip([{ name: "manifest.toml", data: manifest("dev.krate.corpus.nocode", "No code") }]), status: 422, reason: "not a valid .krate bundle" },
  { file: "too-large.krate", kind: "one byte over the 5 MiB door (made at test time, not committed)", make: () => new Uint8Array(5 * 1024 * 1024 + 1), status: 413, committed: false },
];

export const sha256 = (b) => createHash("sha256").update(b).digest("hex");

/// What a file's central directory says it expands to, or null for one it cannot read.
export function expanded(bytes) {
  try { return directory(bytes).reduce((n, e) => n + e.size, 0); } catch { return null; }
}

/// The manifest's id, name and version, for the files that carry one.
export function manifestOf(bytes) {
  try {
    const e = directory(bytes).find((x) => x.name === "manifest.toml");
    if (!e || e.method !== 0) return null;
    const lh = e.local;
    const start = lh + 30 + (bytes[lh + 26] | (bytes[lh + 27] << 8)) + (bytes[lh + 28] | (bytes[lh + 29] << 8));
    const text = new TextDecoder().decode(bytes.subarray(start, start + e.size));
    const field = (k) => (new RegExp(`^${k} = "([^"]*)"`, "m").exec(text) || [])[1] || null;
    return { id: field("id"), name: field("name"), version: field("version") };
  } catch { return null; }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const record = [];
  for (const c of CASES) {
    const bytes = c.make();
    if (c.committed !== false) writeFileSync(join(here, c.file), bytes);
    record.push({ file: c.file, kind: c.kind, committed: c.committed !== false, size: bytes.length, sha256: sha256(bytes), expanded: expanded(bytes), manifest: manifestOf(bytes), expect: { status: c.status, ...(c.reason ? { reason: c.reason } : {}) } });
  }
  writeFileSync(join(here, "corpus.json"), JSON.stringify(record, null, 2) + "\n");
  console.log(`wrote ${record.length} cases`);
}
