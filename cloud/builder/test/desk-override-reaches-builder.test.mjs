/* The desk's override and its free-apps grant reach the builder's own wall (2026-11-23).
 *
 * The builder keeps its own one-app wall before it asks the hub for a case.
 * It let a person past only when /me said `plan.active`, and /me stopped
 * sending `plan` when paid plans went (F-322): "Lift free limits" on the
 * desk lifted the hub's wall and never this one, so the founder's own
 * account met "the free app is used" here. /me now says `limits_lifted` and
 * `free_apps`; the builder reads both.
 *
 * Drives the real server against a fake hub and an engine that ends at once.
 *
 *   node cloud/builder/test/desk-override-reaches-builder.test.mjs
 */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer as probeServer } from "node:net";

const freePort = () => new Promise((resolve, reject) => {
  const probe = probeServer();
  probe.once("error", reject);
  probe.listen(0, "127.0.0.1", () => { const { port } = probe.address(); probe.close(() => resolve(port)); });
});
const PORT = await freePort();
const BASE = `http://127.0.0.1:${PORT}`;

let me = {};
let made = 1;
const hub = createServer((req, res) => {
  req.on("data", () => {});
  req.on("end", () => {
    const send = (code, body) => { res.statusCode = code; res.setHeader("content-type", "application/json"); res.end(JSON.stringify(body)); };
    if (req.url === "/me") return send(200, { user: { login: "pat" }, ...me });
    if (req.url === "/plan/get") return send(200, { n: made });
    if (req.url === "/case/open") return send(200, { id: `c${Date.now()}`, n: made });
    return send(200, { ok: true });
  });
});
await new Promise((r) => hub.listen(0, "127.0.0.1", r));

const dir = await mkdtemp(join(tmpdir(), "krate-override-"));
const bin = join(dir, "krate");
await writeFile(bin, "#!/bin/sh\nexit 1\n");
await chmod(bin, 0o755);
const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: { ...process.env, PORT: String(PORT), KRATE_BIN: bin, KRATE_HUB: `http://127.0.0.1:${hub.address().port}`,
    KRATE_STATE_DIR: join(dir, "state"), KRATE_AGENT: "claude", KRATE_BUILDER_SECRET: "s" },
  stdio: ["ignore", "pipe", "pipe"],
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const build = async () => {
  const r = await fetch(`${BASE}/build`, {
    method: "POST",
    headers: { authorization: "Bearer pat-token", "content-type": "application/json" },
    body: JSON.stringify({ request: "an expense tracker", device: "d".repeat(64) }),
  });
  const body = await r.json().catch(() => ({}));
  if (r.status === 200) {
    // Let the instant engine end, so the next start is not "one at a time".
    for (let i = 0; i < 50; i++) {
      const j = await (await fetch(`${BASE}/build/${body.id}`, { headers: { authorization: "Bearer pat-token" } })).json();
      if (j.state !== "working") break;
      await sleep(100);
    }
  }
  return { status: r.status, body };
};

try {
  for (let i = 0; i < 50; i++) { try { if ((await fetch(`${BASE}/health`)).ok) break; } catch (e) { /* booting */ } await sleep(100); }

  me = {};
  const walled = await build();
  assert.notEqual(walled.status, 200, "one app made, no override: the wall");
  assert.equal(walled.body.wall, true);
  console.log("ok  without an override, the second app meets the builder's wall");

  me = { limits_lifted: true };
  assert.equal((await build()).status, 200, "the desk's Lift free limits gets past the builder");
  made = 40;
  assert.equal((await build()).status, 200, "however many were made");
  console.log("ok  Lift free limits reaches the builder");

  me = { limits_lifted: false, free_apps: 6 };
  made = 1;
  assert.equal((await build()).status, 200, "a grant of six: the second app starts");
  made = 5;
  assert.equal((await build()).status, 200, "and the sixth");
  made = 6;
  assert.equal((await build()).body.wall, true, "the seventh meets the wall");
  console.log("ok  a free-apps grant moves the builder's wall to its number");

  me = { plan: { active: true } };
  made = 3;
  assert.equal((await build()).status, 200, "an older hub's plan.active still counts");
  console.log("ok  an older hub's answer still works");
} finally {
  proc.kill("SIGTERM");
  hub.close();
}
