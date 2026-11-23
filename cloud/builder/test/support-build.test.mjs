/* The hub can read everything about a reported build; nobody else can.
 *
 * When someone sends a report, the hub asks the builder for the build it is
 * about -- what was asked, cost and rounds, how it ended, the account's
 * other recent builds, the end of the engine log -- and mails it to the
 * founder (2026-11-23: a report said a build failed and nothing could say
 * why). Drives the real builder, booted on a state folder that already holds
 * two jobs and one kept log, the way a restarted machine finds them.
 *
 *   node cloud/builder/test/support-build.test.mjs
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

const port = await new Promise((resolve, reject) => {
  const probe = createServer();
  probe.once("error", reject);
  probe.listen(0, "127.0.0.1", () => { const { port } = probe.address(); probe.close(() => resolve(port)); });
});
const SECRET = "support-secret-for-tests";
const state = await mkdtemp(join(tmpdir(), "krate-support-"));
await mkdir(join(state, "jobs"), { recursive: true });
await mkdir(join(state, "logs"), { recursive: true });
const now = Date.now();
const job = (id, extra) => ({
  id, account: "sam", device: "d", request: "a desktop simulator", state: "failed", stage: "test",
  line: "checking it builds", error: "That one grew bigger than a single build can carry.",
  started: now - 600000, finished: now - 60000, parent: null, paidBy: "krate",
  spend: { usd: 2.58, model: "claude-opus-5", rounds: 16 }, ...extra,
});
await writeFile(join(state, "jobs", "aaaa1111aaaa1111aaaa1111aaaa1111.json"), JSON.stringify(job("aaaa1111aaaa1111aaaa1111aaaa1111", {})));
await writeFile(join(state, "jobs", "bbbb2222bbbb2222bbbb2222bbbb2222.json"),
  JSON.stringify(job("bbbb2222bbbb2222bbbb2222bbbb2222", { started: now - 1200000, spend: { usd: 2.5825, rounds: 15 } })));
await writeFile(join(state, "jobs", "cccc3333cccc3333cccc3333cccc3333.json"),
  JSON.stringify(job("cccc3333cccc3333cccc3333cccc3333", { account: "someone-else" })));
const engineLog = `${"round 3: error[E0599]: no method named `drag_to` found\n".repeat(2000)}krate-spend: {"usd":2.58,"rounds":16}\n`;
await writeFile(join(state, "logs", "aaaa1111aaaa1111aaaa1111aaaa1111.log"), engineLog);

const proc = spawn("node", ["cloud/builder/src/server.js"], {
  env: { ...process.env, PORT: String(port), KRATE_STATE_DIR: state, KRATE_BUILDER_SECRET: SECRET, KRATE_AGENT: "claude" },
  stdio: ["ignore", "pipe", "pipe"],
});
const base = `http://127.0.0.1:${port}`;
try {
  for (let i = 0; i < 50; i++) {
    try { if ((await fetch(`${base}/health`)).ok) break; } catch (e) { /* booting */ }
    await new Promise((r) => setTimeout(r, 100));
  }
  const ask = (id, secret) => fetch(`${base}/support/build/${id}`, { headers: secret ? { "x-krate-builder": secret } : {} });

  const r = await ask("aaaa1111aaaa1111aaaa1111aaaa1111", SECRET);
  assert.equal(r.status, 200, await r.clone().text());
  const b = await r.json();
  assert.equal(b.account, "sam");
  assert.equal(b.request, "a desktop simulator");
  assert.equal(b.rounds, 16);
  assert.equal(b.usd, 2.58);
  assert.equal(b.error, "That one grew bigger than a single build can carry.");
  assert.deepEqual(b.recent.map((x) => x.id), ["bbbb2222bbbb2222bbbb2222bbbb2222"], "the same account's other builds, not anyone else's");
  assert.equal(b.recent[0].rounds, 15);
  assert.ok(b.log.endsWith('krate-spend: {"usd":2.58,"rounds":16}\n'), "the end of the engine log");
  assert.ok(b.log.length <= 48 * 1024, "the log is bounded to its last 48 KB");
  assert.equal(b.logBytes, Buffer.byteLength(engineLog), "and says how big the whole one is");
  console.log("ok  the hub reads the build, its cost, the account's other builds and the engine log");

  assert.equal((await ask("aaaa1111aaaa1111aaaa1111aaaa1111", null)).status, 404, "no secret, no door");
  assert.equal((await ask("aaaa1111aaaa1111aaaa1111aaaa1111", "wrong")).status, 404, "a wrong secret meets the same 404");
  const none = await ask("dddd4444dddd4444dddd4444dddd4444", SECRET);
  assert.equal(none.status, 404);
  const noLog = await (await ask("bbbb2222bbbb2222bbbb2222bbbb2222", SECRET)).json();
  assert.equal(noLog.log, null, "a build with no kept log says so");
  console.log("ok  only the hub's secret opens it");
} finally {
  proc.kill();
}
