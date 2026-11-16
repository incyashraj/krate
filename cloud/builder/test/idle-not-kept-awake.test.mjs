/* Health checks and scanners do not keep the builder awake (2026-11-09).
 *
 * Fly asks /health every 30 seconds, and anything on the internet asks for
 * "/" or "/sellers.json". Each request used to reset the idle clock, so the
 * machine never stopped and was billed around the clock with nobody
 * building. Drives the real server, asks only those things, faster than the
 * idle time, and expects it to leave anyway.
 *
 *   node cloud/builder/test/idle-not-kept-awake.test.mjs
 */
import assert from "node:assert";
import { spawn } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer as probeServer } from "node:net";

// An OS-chosen free port for a child process to listen on.
const freePort = () => new Promise((resolve, reject) => {
  const probe = probeServer();
  probe.once("error", reject);
  probe.listen(0, "127.0.0.1", () => { const { port } = probe.address(); probe.close(() => resolve(port)); });
});
const PORT = await freePort();
const BASE = `http://127.0.0.1:${PORT}`;
const IDLE_MS = 1200;

const dir = await mkdtemp(join(tmpdir(), "krate-awake-"));
const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: { ...process.env, PORT: String(PORT), KRATE_BIN: "/bin/false", KRATE_STATE_DIR: join(dir, "state"),
    KRATE_AGENT: "claude", KRATE_BUILDER_DEV: "1", KRATE_IDLE_EXIT_MS: String(IDLE_MS) },
  stdio: ["ignore", "pipe", "pipe"],
});
let out = "";
proc.stdout.on("data", (b) => { out += b; });
proc.stderr.on("data", (b) => { out += b; });
let exited = null;
proc.on("exit", (code) => { exited = { code, at: Date.now() }; });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
try {
  const start = Date.now();
  // Every 200 ms, well inside the idle time: a health check, a HEAD uptime
  // ping, and what a scanner asks for.
  while (!exited && Date.now() - start < IDLE_MS * 6) {
    for (const [path, method] of [["/health", "GET"], ["/health", "HEAD"], ["/", "GET"], ["/sellers.json", "GET"]]) {
      try { await fetch(BASE + path, { method }); } catch (e) { /* not up yet, or gone */ }
    }
    await sleep(200);
  }
  assert.ok(exited, "health checks and scanners kept the builder awake\n" + out);
  assert.strictEqual(exited.code, 0, "an idle exit is a clean exit");
  assert.match(out, /no build running; stopping/);
  console.log("idle exit: health checks and scanners do not keep it awake");
} finally {
  if (!exited) proc.kill("SIGTERM");
}
