// The source download, against a REAL .krate built by the engine.
//
// Two halves have to be right: reading the archive Krate writes, and
// writing one the rest of the world can open. The second is checked by
// unzipping the result with the system `unzip`, because a zip only we can
// read is not a zip.
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");

// Lift the four zip functions out of the shipped bridge.
function grab(name) {
  const at = src.search(new RegExp(`^(async )?function ${name}\\(`, "m"));
  assert.ok(at >= 0, `${name} is defined`);
  const firstLineEnd = src.indexOf("\n", at);
  const firstLine = src.slice(at, firstLineEnd);
  // A one-line function (the u16/u32 helpers) ends on its own line.
  if (firstLine.trimEnd().endsWith("}")) return firstLine;
  const end = src.indexOf("\n}\n", at);
  assert.ok(end > at, `${name} closes`);
  return src.slice(at, end + 2);
}
const code = ["zipU16", "zipU32", "zipEntries", "zipRead", "zipWrite"].map(grab).join("\n");
const { zipEntries, zipRead, zipWrite } = await import(
  "data:text/javascript," + encodeURIComponent(`${code}\nexport { zipEntries, zipRead, zipWrite };`)
);

// A real bundle the engine wrote, kept beside the test: the whole point is
// reading what Krate actually produces, so a hand-made fixture would prove
// nothing. Any .krate works; this one is small.
const KRATE = new URL("./source-fixture.krate", import.meta.url).pathname;
const bytes = new Uint8Array(readFileSync(KRATE));

// Reading: the source entries are found, and their bytes come back whole.
const entries = zipEntries(bytes);
assert.ok(entries.length >= 5, `the archive lists its entries: ${entries.length}`);
const source = entries.filter((e) => e.name.startsWith("source/") && !e.name.endsWith("/"));
assert.ok(source.length >= 3, `source files found: ${source.map((e) => e.name).join(", ")}`);

const files = [];
for (const e of source) {
  const body = await zipRead(bytes, e);
  assert.ok(body, `${e.name} inflates`);
  assert.equal(body.length, e.size, `${e.name} is its declared length`);
  files.push([e.name.slice("source/".length), body]);
}

// The lib.rs we pulled out is real Rust, not a fragment.
const lib = files.find(([n]) => n.endsWith("lib.rs"));
assert.ok(lib, "src/lib.rs is in there");
const text = new TextDecoder().decode(lib[1]);
assert.match(text, /#!\[no_std\]/, "it is a real Krate guest");
assert.match(text, /fn /, "and it has code in it");

// Writing: a zip the SYSTEM can open, with the files intact.
const dir = mkdtempSync(join(tmpdir(), "krate-src-"));
const zipPath = join(dir, "out.zip");
const blob = zipWrite(files);
writeFileSync(zipPath, Buffer.from(await blob.arrayBuffer()));
execFileSync("unzip", ["-q", "-o", zipPath, "-d", join(dir, "out")]);
const back = readFileSync(join(dir, "out", lib[0]));
assert.equal(back.length, lib[1].length, "the file survives the round trip");
assert.deepEqual(new Uint8Array(back), lib[1], "byte for byte");

console.log(`ok  read ${files.length} source files out of a real .krate`);
console.log("ok  wrote a zip the system unzip opens, byte for byte");
