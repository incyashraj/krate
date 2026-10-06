/* The builder never stops itself mid-build, and does stop when idle (2026-10-06).
 *
 * A person on web Studio lost three builds, each ended as "stopped" by the
 * service at 3, 6 and 8 minutes. Fly's auto-stop judged the machine idle from
 * its HTTP traffic while a build ran as a child process, stopped it, and the
 * shutdown handler ended the builds. Fly's auto-stop is now off; the builder
 * leaves on its own, only when no build is working and nothing has arrived
 * for KRATE_IDLE_EXIT_MS.
 *
 * Drives the real server with a fake engine whose build outlasts the idle
 * time, and nobody polling while it runs -- the case that killed the builds.
 *
 *   node cloud/builder/test/idle-exit.test.mjs
 */
import assert from "node:assert";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = 8941;
const BASE = `http://127.0.0.1:${PORT}`;
const IDLE_MS = 1200;
const BUILD_SECONDS = 3; // well past the idle time, with no requests at all

const dir = await mkdtemp(join(tmpdir(), "krate-idle-"));
const bin = join(dir, "krate");
await writeFile(bin, `#!/bin/sh
if [ "$1" = "create" ]; then
  out=""; prev=""
  for a in "$@"; do if [ "$prev" = "--output" ]; then out="$a"; fi; prev="$a"; done
  echo "reading what krate can do"
  sleep ${BUILD_SECONDS}
  echo "==> packing"
  printf 'not-a-real-bundle' > "$out"
  exit 0
fi
exit 1
`);
await chmod(bin, 0o755);

const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: { ...process.env, PORT: String(PORT), KRATE_BIN: bin, KRATE_STATE_DIR: join(dir, "state"),
    KRATE_AGENT: "claude", KRATE_BUILDER_DEV: "1", KRATE_IDLE_EXIT_MS: String(IDLE_MS) },
  stdio: ["ignore", "pipe", "pipe"],
});
let out = "";
proc.stdout.on("data", (b) => { out += b; });
proc.stderr.on("data", (b) => { out += b; });
let exited = null;
proc.on("exit", (code) => { exited = { code, at: Date.now() }; });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, ms = 8000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await what(); if (v) return v; } catch (e) { /* not yet */ } await sleep(100); }
  throw new Error("timed out waiting\n" + out);
}
const call = (path, init) => fetch(BASE + path, { ...init, headers: { authorization: "Bearer dev", "content-type": "application/json", ...(init && init.headers) } });

try {
  await until(async () => (await fetch(BASE + "/health")).ok);
  const started = await (await call("/build", { method: "POST", body: JSON.stringify({ request: "a notes app" }) })).json();
  assert.ok(started.id, "a build starts: " + JSON.stringify(started));

  // Nobody asks anything for longer than the idle time while it builds.
  await sleep(BUILD_SECONDS * 1000 - 400);
  assert.strictEqual(exited, null, "the builder must not leave while a build is working\n" + out);

  const done = await until(async () => { const j = await (await call(`/build/${started.id}`)).json(); return j.state !== "working" && j; });
  assert.strictEqual(done.state, "done", "the build ran to the end: " + JSON.stringify(done));

  // With the build over and no more requests, it leaves on its own, cleanly.
  const leftAt = Date.now();
  await until(async () => exited, IDLE_MS * 4 + 2000);
  assert.strictEqual(exited.code, 0, "an idle exit is a clean exit, so the machine stops rather than restarts");
  assert.ok(exited.at - leftAt >= IDLE_MS - 300, "it waited out the idle time after the last request");
  assert.match(out, /no build running; stopping/);
  console.log("idle exit: stays up through a build, leaves when idle");
} finally {
  if (!exited) proc.kill("SIGTERM");
}
