// Porting from a phone or tablet: a .zip, read in the tab.
//
// A phone cannot choose a folder (iOS and Android browsers ignore
// webkitdirectory), and Port's Choose a folder did nothing there. The web
// door now asks such a device for a .zip and turns it into exactly the
// {name, files} a chosen folder gives, so the build service runs the same
// `krate port`. Checked against a real zip made by the system `zip` (with
// deflate, a GitHub-style top folder, node_modules, .git and __MACOSX), and
// the device check against real user agents.
import { readFileSync, writeFileSync, mkdirSync, mkdtempSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");
function grab(name) {
  const at = src.search(new RegExp(`^(async )?function ${name}\\(`, "m"));
  assert.ok(at >= 0, `${name} is defined`);
  const firstLineEnd = src.indexOf("\n", at);
  if (src.slice(at, firstLineEnd).trimEnd().endsWith("}")) return src.slice(at, firstLineEnd);
  const end = src.indexOf("\n}\n", at);
  assert.ok(end > at, `${name} closes`);
  return src.slice(at, end + 2);
}
function grabConst(name) {
  const m = src.match(new RegExp(`^const ${name} = .*;$`, "m"));
  assert.ok(m, `${name} is defined`);
  return m[0];
}
const code = [
  ...["PROJECT_SKIP", "PROJECT_FILE_MAX", "PROJECT_FILES_MAX", "PROJECT_BYTES_MAX", "PROJECT_ZIP_MAX"].map(grabConst),
  ...["zipU16", "zipU32", "zipEntries", "zipRead", "bytesBase64", "projectFromZip", "cannotPickFolders"].map(grab),
].join("\n");
const make = new Function("navigator", "matchMedia", `
  function refusal(message) { const e = new Error(message); e.refusal = true; return e; }
  ${code}
  return { projectFromZip, cannotPickFolders };`);

// A project as GitHub's Download ZIP gives it, plus what a port never reads.
const dir = mkdtempSync(join(tmpdir(), "krate-port-zip-"));
const top = join(dir, "my-notes-main");
mkdirSync(join(top, "src"), { recursive: true });
mkdirSync(join(top, "node_modules", "left-pad"), { recursive: true });
mkdirSync(join(top, ".git"), { recursive: true });
mkdirSync(join(dir, "__MACOSX"), { recursive: true });
const main = 'const { app } = require("electron");\n' + "localStorage.setItem('note', 'hello');\n".repeat(200);
writeFileSync(join(top, "package.json"), '{"name":"my-notes","main":"src/main.js"}');
writeFileSync(join(top, "src", "main.js"), main);
writeFileSync(join(top, "node_modules", "left-pad", "index.js"), "module.exports = 1;");
writeFileSync(join(top, ".git", "HEAD"), "ref: refs/heads/main");
writeFileSync(join(dir, "__MACOSX", "._main.js"), "junk");
execFileSync("zip", ["-qr", "-9", "project.zip", "my-notes-main", "__MACOSX"], { cwd: dir });
const zipBytes = readFileSync(join(dir, "project.zip"));
const asFile = (bytes, name) => ({ name, size: bytes.length, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length) });

const { projectFromZip } = make({ userAgent: "Mozilla/5.0 (Linux; Android 14; Pixel 8)", maxTouchPoints: 5 }, () => ({ matches: true }));
const project = await projectFromZip(asFile(zipBytes, "my-notes-main.zip"));
assert.equal(project.name, "my-notes", "named for the project, not GitHub's -main folder");
assert.deepEqual(project.files.map((f) => f.path).sort(), ["package.json", "src/main.js"],
  "the top folder is stripped, and node_modules, .git and __MACOSX are left out");
const back = Buffer.from(project.files.find((f) => f.path === "src/main.js").bytes, "base64").toString("utf8");
assert.equal(back, main, "a deflated file comes back byte for byte");
console.log("ok  a zipped project becomes the same files a chosen folder gives");

// A zip with the project at its root (Files' Compress of the contents).
{
  const flat = join(dir, "flat");
  mkdirSync(flat);
  writeFileSync(join(flat, "index.html"), "<h1>hi</h1>");
  writeFileSync(join(flat, "app.js"), "console.log(1)");
  execFileSync("zip", ["-qr", "../flat.zip", "."], { cwd: flat });
  const p = await projectFromZip(asFile(readFileSync(join(dir, "flat.zip")), "Tip Splitter.zip"));
  assert.equal(p.name, "Tip Splitter");
  assert.deepEqual(p.files.map((f) => f.path).sort(), ["app.js", "index.html"]);
}
// Not a zip, and a zip with nothing a port reads: said, never sent.
await assert.rejects(projectFromZip(asFile(Buffer.from("not a zip at all"), "notes.zip")), (e) => e.refusal && /not a zip Krate can open/.test(e.message));
{
  const junk = join(dir, "junk");
  mkdirSync(join(junk, "node_modules"), { recursive: true });
  writeFileSync(join(junk, "node_modules", "x.js"), "1");
  execFileSync("zip", ["-qr", "../junk.zip", "."], { cwd: junk });
  await assert.rejects(projectFromZip(asFile(readFileSync(join(dir, "junk.zip")), "junk.zip")), (e) => e.refusal && /no files a port can read/.test(e.message));
}
console.log("ok  a flat zip works, and a bad or empty one is refused in words");

// Which devices are asked for a zip: phones and tablets, and only them.
const ask = (ua, touch, coarse) => make({ userAgent: ua, maxTouchPoints: touch }, () => ({ matches: coarse })).cannotPickFolders();
assert.equal(ask("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)", 5, false), true, "iPhone, known by its name alone");
assert.equal(ask("Mozilla/5.0 (Linux; Android 14; Pixel 8)", 5, false), true, "Android, known by its name alone");
assert.equal(ask("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 5, false), true, "an iPad, which calls itself a Mac");
assert.equal(ask("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", 0, false), false, "a Mac");
assert.equal(ask("Mozilla/5.0 (Windows NT 10.0; Win64; x64)", 0, false), false, "a Windows PC");
assert.equal(ask("Mozilla/5.0 (Windows NT 10.0; Win64; x64)", 10, false), false, "a touch-screen laptop with a mouse keeps its folder picker");
assert.equal(ask("Mozilla/5.0 (X11; Linux x86_64)", 5, true), true, "any other touch-only device");
console.log("ok  phones and tablets are asked for a .zip; computers keep Choose a folder");

// And the door uses it: pick_source_folder takes the zip path on such a device.
{
  const at = src.indexOf("async pick_source_folder(");
  const body = src.slice(at, src.indexOf("\n  },", at));
  assert.match(body, /if \(cannotPickFolders\(\)\) \{[\s\S]*pickLocalZip\(\)[\s\S]*projectFromZip\(zip\)/, "the port door asks for a zip there");
}
console.log("ok  the port door asks a phone for a .zip");
