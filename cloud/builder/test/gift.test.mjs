/* A gift for a friend without Krate, made from a web build (2026-10-07).
 *
 * The browser has no engine to put in a gift, so the build service makes
 * it: the built app plus the release's notarized opener and both signed Mac
 * engines (or, for Linux, its own engine). These checks hold the parts that
 * would hurt if they drifted:
 *
 *   - the Mac parts are the release's, each matched to its SHA256SUMS line,
 *     and a part that does not match makes no gift at all;
 *   - the engine is told to use exactly those parts (opener + one player
 *     per architecture);
 *   - only the account that made the app can get a gift of it;
 *   - Windows says plainly it is not ready, rather than handing out a file
 *     that installs nothing;
 *   - an engine too old to make gifts is never asked to.
 *
 * Drives the real server with a fake engine and a fake release server.
 *
 *   node cloud/builder/test/gift.test.mjs
 */
import assert from "node:assert";
import { createServer } from "node:http";
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const VERSION = "9.9.9";
const PORT = 8962;
const REL_PORT = 8963;
const BASE = `http://127.0.0.1:${PORT}`;

const dir = await mkdtemp(join(tmpdir(), "krate-gift-"));

// ---- a fake release: the opener zip and two Mac tarballs, with checksums --
const rel = join(dir, "release");
await mkdir(join(rel, "Krate Opener.app/Contents/MacOS"), { recursive: true });
await writeFile(join(rel, "Krate Opener.app/Contents/MacOS/open"), "#!/bin/sh\n");
execFileSync("zip", ["-qr", `krate-opener-${VERSION}.zip`, "Krate Opener.app"], { cwd: rel });
for (const triple of ["aarch64-apple-darwin", "x86_64-apple-darwin"]) {
  const d = join(rel, `krate-${VERSION}-${triple}`);
  await mkdir(d, { recursive: true });
  await writeFile(join(d, "krate"), `engine for ${triple}`);
  execFileSync("tar", ["-czf", `krate-${VERSION}-${triple}.tar.gz`, `krate-${VERSION}-${triple}`], { cwd: rel });
}
const assets = [`krate-opener-${VERSION}.zip`, `krate-${VERSION}-aarch64-apple-darwin.tar.gz`, `krate-${VERSION}-x86_64-apple-darwin.tar.gz`];
let sums = "";
for (const a of assets) sums += `${createHash("sha256").update(await readFile(join(rel, a))).digest("hex")}  ${a}\n`;
let tamper = false;
const fetched = [];
const release = createServer(async (req, res) => {
  const name = req.url.split("/").pop();
  fetched.push(name);
  if (name === "SHA256SUMS") { res.end(sums); return; }
  if (!assets.includes(name)) { res.statusCode = 404; res.end(); return; }
  let bytes = await readFile(join(rel, name));
  if (tamper && name.includes("x86_64")) bytes = Buffer.concat([bytes, Buffer.from("x")]);
  res.end(bytes);
});
await new Promise((r) => release.listen(REL_PORT, r));

// ---- a fake engine that builds an app and makes gifts ---------------------
const log = join(dir, "engine.log");
const bin = join(dir, "krate");
await writeFile(bin, `#!/bin/sh
echo "$*" >> "${log}"
if [ "$1" = "--version" ]; then echo "krate ${VERSION}"; exit 0; fi
if [ "$1" = "gift-opener" ]; then echo 'installed="$("$work/krate" player-install)"'; exit 0; fi
if [ "$1" = "create" ]; then
  out=""; prev=""
  for a in "$@"; do if [ "$prev" = "--output" ]; then out="$a"; fi; prev="$a"; done
  echo "==> packing"; printf 'APPBYTES' > "$out"; exit 0
fi
if [ "$1" = "wrap" ]; then
  out=""; prev=""; for a in "$@"; do if [ "$prev" = "-o" ]; then out="$a"; fi; prev="$a"; done
  printf 'GIFT:%s' "$3" > "$out"; echo "Gift written: $out (1 MB)"; exit 0
fi
exit 0
`);
await chmod(bin, 0o755);

const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: { ...process.env, PORT: String(PORT), KRATE_BIN: bin, KRATE_STATE_DIR: join(dir, "state"),
    KRATE_AGENT: "claude", KRATE_BUILDER_DEV: "1", KRATE_RELEASE_BASE: `http://127.0.0.1:${REL_PORT}` },
  stdio: ["ignore", "pipe", "pipe"],
});
let out = "";
proc.stdout.on("data", (b) => { out += b; });
proc.stderr.on("data", (b) => { out += b; });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, ms = 10000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await what(); if (v) return v; } catch (e) { /* not yet */ } await sleep(100); }
  throw new Error("timed out\n" + out);
}
const call = (path, init = {}, token = "dev") => fetch(BASE + path, { ...init, headers: { authorization: `Bearer ${token}`, "content-type": "application/json", ...(init.headers || {}) } });

try {
  await until(async () => (await fetch(BASE + "/health")).ok);
  const health = await (await fetch(BASE + "/health")).json();
  assert.strictEqual(health.gifts, true, "an engine whose opener installs the player can make gifts");

  const started = await (await call("/build", { method: "POST", body: JSON.stringify({ request: "a tip calculator" }) })).json();
  await until(async () => (await (await call(`/build/${started.id}`)).json()).state === "done");

  // ---- Mac: the release's parts, checked, handed to the engine -------------
  const mac = await call(`/build/${started.id}/gift?for=mac`);
  assert.strictEqual(mac.status, 200, await mac.clone().text());
  assert.match(mac.headers.get("content-disposition"), /-for-Mac\.zip"/);
  assert.match(await mac.text(), /^GIFT:mac/);
  const wrapLine = (await readFile(log, "utf8")).split("\n").find((l) => l.startsWith("wrap --for mac"));
  assert.ok(wrapLine, "the engine was asked for a Mac gift");
  assert.match(wrapLine, /--opener \S*gift-kit\/9\.9\.9\/opener\/Krate Opener\.app/, "with the release's opener");
  assert.match(wrapLine, /--player \S*\/arm64\/krate --player \S*\/x86_64\/krate/, "and both players");
  const kit = join(dir, "state/gift-kit", VERSION);
  assert.strictEqual(await readFile(join(kit, "arm64/krate"), "utf8"), "engine for aarch64-apple-darwin");
  assert.strictEqual(await readFile(join(kit, "x86_64/krate"), "utf8"), "engine for x86_64-apple-darwin");

  // Kept: a second gift fetches nothing.
  const before = fetched.length;
  assert.strictEqual((await call(`/build/${started.id}/gift?for=mac`)).status, 200);
  assert.strictEqual(fetched.length, before, "the parts are fetched once");

  // ---- Linux: the engine carries itself -----------------------------------
  const linux = await call(`/build/${started.id}/gift?for=linux`);
  assert.strictEqual(linux.status, 200);
  assert.match(linux.headers.get("content-disposition"), /-for-Linux\.sh"/);

  // ---- Windows: said plainly ---------------------------------------------
  const win = await call(`/build/${started.id}/gift?for=windows`);
  assert.strictEqual(win.status, 400);
  assert.match(await win.text(), /not ready yet.*link/);

  // ---- somebody else's app: no such build ---------------------------------
  // (dev mode maps every token to one account, so prove the 404 on an id)
  assert.strictEqual((await call(`/build/nope/gift?for=mac`)).status, 404);

  console.log("gifts: the release's parts, checked once, Mac and Linux made, Windows said plainly");
} finally {
  proc.kill("SIGTERM");
}

// ---- a tampered part makes no gift ----------------------------------------
{
  const dir2 = await mkdtemp(join(tmpdir(), "krate-gift2-"));
  tamper = true;
  const p2 = spawn("node", ["cloud/builder/src/server.js"], {
    env: { ...process.env, PORT: String(PORT), KRATE_BIN: bin, KRATE_STATE_DIR: join(dir2, "state"),
      KRATE_AGENT: "claude", KRATE_BUILDER_DEV: "1", KRATE_RELEASE_BASE: `http://127.0.0.1:${REL_PORT}` },
    stdio: ["ignore", "pipe", "pipe"],
  });
  try {
    await until(async () => (await fetch(BASE + "/health")).ok);
    const s = await (await call("/build", { method: "POST", body: JSON.stringify({ request: "a timer" }) })).json();
    await until(async () => (await (await call(`/build/${s.id}`)).json()).state === "done");
    const r = await call(`/build/${s.id}/gift?for=mac`);
    assert.strictEqual(r.status, 500);
    assert.match(await r.text(), /does not match the release's checksum/);
    console.log("gifts: a part that does not match its checksum makes no gift");
  } finally {
    p2.kill("SIGTERM");
    release.close();
  }
}
