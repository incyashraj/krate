/* The compile door for Krate in Claude (IC-989 phase 2).
 *
 * A chat model writes an app's files; the hub hands them here; this box
 * runs `krate check-app` and `krate pack` and nothing else -- no model, no
 * key. Drives the REAL server, spawned as a process, with a fake `krate`
 * that logs every way it is called and the model keys it was given.
 *
 *   node cloud/builder/test/compile.test.mjs
 */
import assert from "node:assert";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer as probeServer } from "node:net";

const freePort = () => new Promise((resolve, reject) => {
  const probe = probeServer();
  probe.once("error", reject);
  probe.listen(0, "127.0.0.1", () => { const { port } = probe.address(); probe.close(() => resolve(port)); });
});

const SECRET = "builder-secret-for-tests";
const PORT = await freePort();
const BUILDER = `http://127.0.0.1:${PORT}`;
const work = await mkdtemp(join(tmpdir(), "krate-compile-test-"));
const LOG = join(work, "calls.log");

/* A krate that compiles in a blink. check-app fails when src/lib.rs says
 * BREAKME, with check-app's own JSON shape; otherwise it leaves the crate's
 * wasm where the real one brings it home. */
const bin = join(work, "krate");
await writeFile(bin, `#!/bin/sh
echo "$1|key:\${ANTHROPIC_API_KEY:-none}" >> "${LOG}"
if [ "$1" = "krate-mode" ]; then echo "# Krate Mode"; echo "Write Cargo.toml, src/lib.rs, manifest.toml."; exit 0; fi
if [ "$1" = "check-app" ]; then
  for a in "$@"; do root="$a"; done
  if grep -q BREAKME "$root/src/lib.rs" 2>/dev/null; then
    echo '{"ok":false,"dir":"x","stage":"build","kind":"compile","detail":"error[E0425]: cannot find value BREAKME","fix":"Remove BREAKME; use krate::ui::label."}'
    exit 3
  fi
  mkdir -p "$root/target/wasm32-wasip1/release"
  printf 'wasm-of-hello' > "$root/target/wasm32-wasip1/release/hello_app.wasm"
  echo '{"ok":true,"stages":["build","imports","run"],"link_notes":[]}'
  exit 0
fi
if [ "$1" = "pack" ]; then
  out=""; prev=""
  for a in "$@"; do if [ "$prev" = "--output" ]; then out="$a"; fi; prev="$a"; done
  printf 'packed:%s' "$(cat "$2")" > "$out"
  exit 0
fi
echo "unexpected: $*" >&2
exit 9
`);
await chmod(bin, 0o755);

const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: {
    ...process.env,
    PORT: String(PORT),
    KRATE_BIN: bin,
    KRATE_HUB: "http://127.0.0.1:9", // never asked on this path
    KRATE_STATE_DIR: join(work, "state"),
    KRATE_AGENT: "claude",
    KRATE_BUILDER_SECRET: SECRET,
    ANTHROPIC_API_KEY: "sk-krate-own-key",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
proc.stderr.on("data", (b) => process.stderr.write(`[builder] ${b}`));

try {
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(`${BUILDER}/health`)).ok) break; } catch (e) {}
    await new Promise((r) => setTimeout(r, 100));
  }

  const HUB = (account) => ({ "x-krate-builder": SECRET, ...(account ? { "x-krate-account": account } : {}) });
  const b64 = (t) => Buffer.from(t).toString("base64");
  const FILES = [
    { path: "Cargo.toml", data: b64('[package]\nname = "hello-app"\nversion = "0.1.0"\n') },
    { path: "src/lib.rs", data: b64("// hello\n") },
    { path: "manifest.toml", data: b64('[app]\nid = "hello"\n') },
  ];
  const compile = (account, body) =>
    fetch(`${BUILDER}/compile`, { method: "POST", headers: { "content-type": "application/json", ...HUB(account) }, body: JSON.stringify(body) });
  const status = (id, headers) => fetch(`${BUILDER}/build/${id}`, { headers });
  async function settled(id, account) {
    for (let i = 0; i < 100; i++) {
      const s = await (await status(id, HUB(account))).json();
      if (s.state !== "working") return s;
      await new Promise((r) => setTimeout(r, 50));
    }
    throw new Error("never settled");
  }

  /* ---- the hub's doors only ---------------------------------------------- */
  assert.strictEqual((await fetch(`${BUILDER}/guide`)).status, 404, "the guide is the hub's to ask for");
  const guide = await fetch(`${BUILDER}/guide`, { headers: HUB() });
  assert.strictEqual(guide.status, 200);
  assert.match(await guide.text(), /^# Krate Mode/);
  assert.strictEqual((await fetch(`${BUILDER}/compile`, { method: "POST", body: "{}" })).status, 404, "no secret, no door");
  assert.strictEqual((await compile(null, { name: "x", files: FILES })).status, 404, "the hub names an account");
  assert.strictEqual((await compile("alice", { name: "x", files: FILES })).status, 404, "only a hub: account, never a login");
  assert.strictEqual((await compile("hub:github:alice", { name: "x", files: [{ path: "../x", data: "" }] })).status, 400);
  assert.strictEqual((await compile("hub:github:alice", { name: "", files: FILES })).status, 400);
  assert.strictEqual(
    (await compile("hub:github:alice", { name: "x", files: Array.from({ length: 65 }, (_, i) => ({ path: `f${i}`, data: "" })) })).status,
    413,
  );

  /* ---- compiled, checked, packed ----------------------------------------- */
  const started = await compile("hub:github:alice", { name: "Hello App", files: FILES });
  assert.strictEqual(started.status, 202);
  const { id } = await started.json();
  assert.match(id, /^[0-9a-f]{32}$/);
  const done = await settled(id, "hub:github:alice");
  assert.strictEqual(done.state, "done", JSON.stringify(done));
  assert.strictEqual(done.kind, "compile");
  assert.strictEqual(done.result.name, "Hello App");
  const file = await fetch(`${BUILDER}/build/${id}/file`, { headers: HUB("hub:github:alice") });
  assert.strictEqual(file.status, 200);
  assert.strictEqual(await file.text(), "packed:wasm-of-hello", "the crate's own component, packed");

  /* ---- only that account's job ------------------------------------------- */
  assert.strictEqual((await status(id, HUB("hub:github:bob"))).status, 404, "another account's job does not exist");
  assert.strictEqual((await status(id, {})).status, 401, "no sign-in, nothing");
  assert.strictEqual((await status(id, { "x-krate-account": "hub:github:alice" })).status, 401, "the account header alone is not the hub");

  /* ---- a failure carries check-app's own words --------------------------- */
  const broken = FILES.map((f) => (f.path === "src/lib.rs" ? { ...f, data: b64("BREAKME\n") } : f));
  const bad = await (await compile("hub:github:alice", { name: "Broken", files: broken })).json();
  const failed = await settled(bad.id, "hub:github:alice");
  assert.strictEqual(failed.state, "failed");
  assert.deepStrictEqual(
    [failed.check.stage, failed.check.fix],
    ["build", "Remove BREAKME; use krate::ui::label."],
  );
  assert.match(failed.check.detail, /E0425/);

  /* ---- no model, no key ------------------------------------------------- */
  // gift-opener is the box warming its gift tool at start, not this path.
  const calls = (await readFile(LOG, "utf8")).trim().split("\n").filter((c) => !c.startsWith("gift-opener|"));
  assert.ok(calls.every((c) => /^(krate-mode|check-app|pack)\|/.test(c)), `only check-app, pack and the guide: ${calls}`);
  assert.ok(calls.every((c) => c.endsWith("key:none")), "the engine never sees a model key on this path");
  console.log("compile door: all assertions passed");
} finally {
  proc.kill("SIGKILL");
}
