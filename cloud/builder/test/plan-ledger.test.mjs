/* Planning on our key has a ceiling that survives the machine sleeping (K-862).
 *
 * `/plan` calls the model on Krate's key and consumes no allowance. Its only
 * limit was twelve an hour per account, counted in a Map in memory -- and
 * the machine stops when idle and starts on the next request, so every wake
 * began at zero. Nor was there any ceiling on the day: accounts are free,
 * so a per-account limit alone bounds nothing.
 *
 * This drives the REAL server, spawned as a process with a stub hub and a
 * fake `krate`, and restarts it on the same state directory the way Fly
 * does -- which is the one thing the old count did not survive.
 *
 *   node cloud/builder/test/plan-ledger.test.mjs
 */
import assert from "node:assert";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const HUB_PORT = 8941;
const BUILDER_PORT = 8942;
const BUILDER = `http://127.0.0.1:${BUILDER_PORT}`;

const TOKENS = { "alice-token": "alice", "bob-token": "bob", "carol-token": "carol", "dan-token": "dan" };
const hub = createServer((req, res) => {
  const who = TOKENS[(req.headers.authorization || "").replace(/^Bearer\s+/i, "")];
  if (req.url === "/me" && who) {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ user: { login: who } }));
  }
  res.statusCode = req.url === "/me" ? 401 : 404;
  res.end("no");
});
await new Promise((r) => hub.listen(HUB_PORT, "127.0.0.1", r));

const work = await mkdtemp(join(tmpdir(), "krate-plan-ledger-"));
const krate = join(work, "krate");
// A plan that answers, and says what it cost when FAKE_PLAN_USD is set --
// the same `krate-spend:` line the real engine prints.
await writeFile(
  krate,
  `#!/bin/sh
if [ "$1" = "plan" ]; then
  [ -n "$FAKE_PLAN_USD" ] && echo "krate-spend: {\\"usd\\": $FAKE_PLAN_USD, \\"model\\": \\"claude-opus-5\\", \\"rounds\\": 1}" >&2
  echo '{"plan":"a small notes app","needs":[]}'
  exit 0
fi
exit 1
`,
);
await chmod(krate, 0o755);

// Every builder started, so a failing assertion cannot leave one holding
// the port for the next run.
const started = [];

function start(stateDir, extra) {
  const proc = spawn("node", ["cloud/builder/src/server.js"], {
    env: {
      ...process.env,
      PORT: String(BUILDER_PORT),
      KRATE_BIN: krate,
      KRATE_HUB: `http://127.0.0.1:${HUB_PORT}`,
      KRATE_STATE_DIR: stateDir,
      KRATE_AGENT: "claude",
      ...extra,
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  proc.stderr.on("data", (b) => process.stderr.write(`[builder] ${b}`));
  started.push(proc);
  return proc;
}

async function up() {
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(`${BUILDER}/health`)).ok) return; } catch (e) {}
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error("builder did not start");
}

async function stop(proc) {
  proc.kill("SIGKILL");
  await new Promise((r) => proc.on("exit", r));
}

async function plan(token) {
  const res = await fetch(`${BUILDER}/plan`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ request: "a tiny notes app" }),
  });
  return { status: res.status, body: await res.text() };
}

try {
  /* ---- the hourly count survives a restart -------------------------------- */
  {
    const state = join(work, "hourly");
    let builder = start(state, { KRATE_PLAN_PER_HOUR: "2" });
    await up();
    assert.strictEqual((await plan("alice-token")).status, 200, "first plan");
    assert.strictEqual((await plan("alice-token")).status, 200, "second plan");
    const third = await plan("alice-token");
    assert.strictEqual(third.status, 429, "the third in the hour is refused");
    assert.match(third.body, /a lot of planning/i);

    // The machine stops when idle and wakes on the next request.
    await stop(builder);
    builder = start(state, { KRATE_PLAN_PER_HOUR: "2" });
    await up();
    const afterWake = await plan("alice-token");
    assert.strictEqual(afterWake.status, 429,
      `a wake does not hand the hour's plans back: ${afterWake.status} ${afterWake.body}`);
    assert.strictEqual((await plan("bob-token")).status, 200, "and it is per account");
    await stop(builder);
  }

  /* ---- the day has a ceiling across every account ------------------------- */
  {
    const state = join(work, "daily");
    const env = { KRATE_PLAN_BUDGET_USD_PER_DAY: "1", FAKE_PLAN_USD: "0.6" };
    let builder = start(state, env);
    await up();
    assert.strictEqual((await plan("alice-token")).status, 200, "0.60 of a 1.00 day");
    assert.strictEqual((await plan("bob-token")).status, 200, "1.20: this one was allowed to start");
    const over = await plan("carol-token");
    assert.strictEqual(over.status, 429, "a third account is refused: new accounts do not reset the day");
    assert.match(over.body, /start building/i, "and told what still works");
    await stop(builder);
    builder = start(state, env);
    await up();
    assert.strictEqual((await plan("dan-token")).status, 429, "the day's spend survives a restart");
    await stop(builder);

    // The spend is recorded, priced from the engine's own line.
    const log = (await readFile(join(state, "audit.log"), "utf8")).trim().split("\n").map((l) => JSON.parse(l));
    const plans = log.filter((e) => e.action === "plan");
    assert.strictEqual(plans.length, 2);
    assert.deepStrictEqual(plans.map((e) => [e.account, e.usd, e.priced, e.paid_by]), [
      ["alice", 0.6, true, "krate"],
      ["bob", 0.6, true, "krate"],
    ]);
  }

  /* ---- a plan with no price line is not free ------------------------------ */
  {
    const state = join(work, "unpriced");
    const builder = start(state, { KRATE_PLAN_BUDGET_USD_PER_DAY: "1", KRATE_PLAN_UNPRICED_USD: "0.6" });
    await up();
    assert.strictEqual((await plan("alice-token")).status, 200);
    assert.strictEqual((await plan("bob-token")).status, 200);
    assert.strictEqual((await plan("carol-token")).status, 429,
      "an engine that prints no price is charged the high guess, so the day still ends");
    await stop(builder);
  }

  console.log("OK -- planning on our key is counted per account and per day, on the volume, and priced");
} finally {
  for (const proc of started) if (proc.exitCode === null) proc.kill("SIGKILL");
  hub.close();
}
