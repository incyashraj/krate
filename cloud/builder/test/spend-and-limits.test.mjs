/* What Krate pays for is always written down, and a limit reaches the person (2026-10-06).
 *
 * Three web builds were killed mid-run and cost about $5 of the Krate key;
 * a stopped build sent no spend to the hub, so it showed nowhere and no
 * limit could see it. And when the hub refused a case, the builder said
 * "We could not start that just now" instead of the hub's sentence.
 *
 * Drives the real server against a fake hub and a fake engine.
 *
 *   node cloud/builder/test/spend-and-limits.test.mjs
 */
import assert from "node:assert";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const HUB_PORT = 8951;
const PORT = 8952;
const BASE = `http://127.0.0.1:${PORT}`;

let refuse = true;
const spends = [];
const attempts = [];
const hub = createServer((req, res) => {
  let raw = "";
  req.on("data", (c) => { raw += c; });
  req.on("end", () => {
    const send = (code, body) => { res.statusCode = code; res.setHeader("content-type", "application/json"); res.end(JSON.stringify(body)); };
    if (req.url === "/me") return send(200, { user: { login: "pat" }, plan: { active: false } });
    if (req.url === "/plan/get") return send(200, { n: 0 });
    if (req.url === "/case/open") {
      if (refuse) return send(429, { wall: true, tries: true, message: "Today's free tries are used. Try again tomorrow." });
      return send(200, { id: "c1", n: 0 });
    }
    if (req.url === "/case/attempt") { attempts.push(JSON.parse(raw || "{}")); return send(200, { ok: true }); }
    if (req.url === "/spend") { spends.push(JSON.parse(raw || "{}")); return send(200, { ok: true }); }
    return send(200, {});
  });
});
await new Promise((r) => hub.listen(HUB_PORT, r));

const dir = await mkdtemp(join(tmpdir(), "krate-spend-"));
const bin = join(dir, "krate");
// An engine that has paid for two rounds and is still working when stopped.
await writeFile(bin, `#!/bin/sh
if [ "$1" = "create" ]; then
  echo "reading what krate can do"
  echo 'krate-spend: {"usd": 0.61, "model": "claude-opus-5", "rounds": 1}' >&2
  echo 'krate-spend: {"usd": 1.37, "model": "claude-opus-5", "rounds": 2}' >&2
  sleep 30
fi
exit 1
`);
await chmod(bin, 0o755);

const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: { ...process.env, PORT: String(PORT), KRATE_BIN: bin, KRATE_HUB: `http://127.0.0.1:${HUB_PORT}`,
    KRATE_STATE_DIR: join(dir, "state"), KRATE_AGENT: "claude", KRATE_BUILDER_SECRET: "s" },
  stdio: ["ignore", "pipe", "pipe"],
});
let log = "";
proc.stdout.on("data", (b) => { log += b; });
proc.stderr.on("data", (b) => { log += b; });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, ms = 10000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { const v = await what(); if (v) return v; } catch (e) { /* not yet */ } await sleep(100); }
  throw new Error("timed out waiting\n" + log);
}
const call = (path, body) => fetch(BASE + path, {
  method: body ? "POST" : "GET",
  headers: { authorization: "Bearer pat-token", "content-type": "application/json" },
  body: body ? JSON.stringify(body) : undefined,
});

try {
  await until(async () => (await fetch(BASE + "/health")).ok);

  // ---- a refused case is the hub's sentence, as a wall ----------------------
  const refused = await call("/build", { request: "an expense tracker", device: "d".repeat(64) });
  assert.strictEqual(refused.status, 429, await refused.clone().text());
  const wall = await refused.json();
  assert.strictEqual(wall.wall, true);
  assert.match(wall.message, /Today's free tries are used/);
  assert.doesNotMatch(wall.message, /could not start/);

  // ...and the account is not left locked by the refused start.
  refuse = false;
  const started = await call("/build", { request: "an expense tracker", device: "d".repeat(64) });
  assert.strictEqual(started.status, 200, "a refusal must not lock the account: " + (await started.clone().text()));
  const { id } = await started.json();

  // ---- a stopped build reports what it had spent ----------------------------
  await until(async () => { const j = await (await call(`/build/${id}`)).json(); return j.state === "working"; });
  await sleep(1200); // the engine has printed its two paid rounds
  const stop = await call(`/build/${id}/stop`, {});
  assert.strictEqual(stop.status, 200);
  await until(() => attempts.some((a) => a.outcome === "stopped"));
  await until(() => spends.length > 0);
  assert.strictEqual(spends[0].usd, 1.37, "the LAST running total is what it cost");
  assert.strictEqual(spends[0].paid_by, "krate");
  console.log("spend and limits: a refusal is the hub's sentence; a stopped build is on the ledger");
} finally {
  proc.kill("SIGTERM");
  hub.close();
}
