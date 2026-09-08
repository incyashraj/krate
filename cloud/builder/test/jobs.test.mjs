/* Builder jobs are owned, opaque, durable and honest (IC-416).
 *
 * This drives the REAL server -- spawned as a process, with a stub hub and a
 * fake `krate` binary -- not a mirror of its logic. The routes it guards
 * used to take no token at all: a truncated UUID was the only thing between
 * anyone on the internet and another person's app file, their build's
 * status, and a working stop button for it.
 *
 *   node cloud/builder/test/jobs.test.mjs
 */
import assert from "node:assert";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const HUB_PORT = 8931;
const BUILDER_PORT = 8932;
const BUILDER = `http://127.0.0.1:${BUILDER_PORT}`;

/* ---- a hub that knows two people ---------------------------------------- */
const TOKENS = {
  "alice-token": "alice",
  "alice-token-2": "alice", // the same person after a sign-in refresh
  "bob-token": "bob",
};

const hub = createServer((req, res) => {
  const token = (req.headers.authorization || "").replace(/^Bearer\s+/i, "");
  const who = TOKENS[token];
  if (req.url === "/me") {
    if (!who) { res.statusCode = 401; return res.end("no"); }
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ user: { login: who }, plan: { active: true } }));
  }
  if (req.url === "/plan/count") { res.statusCode = 200; return res.end("{}"); }
  if (req.url === "/case/open" || req.url === "/case/attempt") {
    let raw = "";
    req.on("data", (c) => { raw += c; });
    req.on("end", () => {
      const body = JSON.parse(raw || "{}");
      caseCalls.push({ path: req.url, who, body });
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(req.url === "/case/open"
        ? { id: "case-" + caseCalls.length, n: 0 }
        : { id: body.id, made: body.outcome === "made" }));
    });
    return;
  }
  res.statusCode = 404;
  res.end("no");
});

// Every ledger call the builder makes, in order, for the assertions below.
const caseCalls = [];

/* ---- a krate that "builds" in half a second ------------------------------ */
async function fakeKrate(dir) {
  const bin = join(dir, "krate");
  await writeFile(
    bin,
    `#!/bin/sh
# create <request> --output <path> --agent <agent> | run <bundle> --shoot ...
if [ "$1" = "create" ]; then
  case "$*" in *FAIL*) echo "the engine fell over" >&2; exit 1;; esac
  out=""
  prev=""
  for a in "$@"; do
    if [ "$prev" = "--output" ]; then out="$a"; fi
    prev="$a"
  done
  echo "reading what krate can do"
  sleep 0.4
  echo "==> packing"
  printf 'not-a-real-bundle' > "$out"
  exit 0
fi
# the screenshot run: fail quietly, a build with no picture is still a build
exit 1
`,
  );
  await chmod(bin, 0o755);
  return bin;
}

/* ---- driving the server -------------------------------------------------- */
function startBuilder(stateDir, krateBin, extraEnv = {}) {
  const proc = spawn("node", ["cloud/builder/src/server.js"], {
    env: {
      ...process.env,
      PORT: String(BUILDER_PORT),
      KRATE_BIN: krateBin,
      KRATE_HUB: `http://127.0.0.1:${HUB_PORT}`,
      KRATE_STATE_DIR: stateDir,
      KRATE_AGENT: "claude", // a CLI provider: no API key needed to switch on
      ...extraEnv,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  proc.stderr.on("data", (b) => process.stderr.write(`[builder] ${b}`));
  return proc;
}

async function until(what, tries = 50) {
  for (let i = 0; i < tries; i++) {
    try {
      const out = await what();
      if (out !== undefined && out !== false) return out;
    } catch (e) { /* not yet */ }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("timed out waiting");
}

const asAlice = { authorization: "Bearer alice-token" };
const asAliceRefreshed = { authorization: "Bearer alice-token-2" };
const asBob = { authorization: "Bearer bob-token" };

async function get(path, headers = {}) {
  const res = await fetch(`${BUILDER}${path}`, { headers });
  return { status: res.status, body: await res.text(), res };
}
async function post(path, body, headers = {}) {
  const res = await fetch(`${BUILDER}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
  return { status: res.status, body: await res.text() };
}

/* ---- the cases ----------------------------------------------------------- */
const stateDir = await mkdtemp(join(tmpdir(), "krate-builder-state-"));
const krateBin = await fakeKrate(await mkdtemp(join(tmpdir(), "krate-fake-")));

// A leftover server from an earlier failed run would answer the health check
// and every assertion after it would test the WRONG process -- which
// happened: a sabotage run died on its assertion, its builder kept the port,
// and the restored code appeared to fail. An orphan here is fatal, not
// something to talk around.
try {
  await fetch(`${BUILDER}/health`);
  throw new Error(
    `something is already listening on ${BUILDER} -- kill it first ` +
      `(pkill -f cloud/builder/src/server.js) or every assertion below ` +
      `tests a stale process`,
  );
} catch (err) {
  if (!String(err.cause?.code || err.message).match(/ECONNREFUSED|fetch failed/i)) throw err;
}

// However an assertion fails, the processes this test started must die with
// it, or the next run tests this run's leftovers.
process.on("exit", () => {
  try { builder && builder.kill("SIGKILL"); } catch (e) {}
  try { hub.close(); } catch (e) {}
});

hub.listen(HUB_PORT);
let builder = startBuilder(stateDir, krateBin);
await until(async () => (await get("/health")).status === 200);

// Alice starts a build.
const started = await post("/build", { request: "a tiny notes app" }, asAlice);
assert.strictEqual(started.status, 200, `start: ${started.body}`);
const jobId = JSON.parse(started.body).id;

// The id is opaque: 32 hex characters, not a truncated UUID.
assert.match(jobId, /^[0-9a-f]{32}$/, `id must be 128 random bits, got ${jobId}`);

// No token, no answers -- not even "does this id exist".
assert.strictEqual((await get(`/build/${jobId}`)).status, 401, "status without a token");
assert.strictEqual((await get(`/build/${jobId}/file`)).status, 401, "file without a token");

// An expired or invalid session is the same as none.
assert.strictEqual(
  (await get(`/build/${jobId}`, { authorization: "Bearer expired-token" })).status,
  401,
  "an expired session reads nothing",
);

// Bob, signed in, holding Alice's id: the job does not exist for him.
assert.strictEqual((await get(`/build/${jobId}`, asBob)).status, 404, "another account's status");
assert.strictEqual((await get(`/build/${jobId}/file`, asBob)).status, 404, "another account's file");
const bobStop = await post(`/build/${jobId}/stop`, {}, asBob);
assert.strictEqual(bobStop.status, 404, "another account's stop");

// A guessed id looks exactly the same as a denied one.
const guessed = await get(`/build/${"0".repeat(32)}`, asAlice);
assert.strictEqual(guessed.status, 404, "a guessed id");

// Alice, and Alice after a sign-in refresh, both see the job.
assert.strictEqual((await get(`/build/${jobId}`, asAlice)).status, 200, "the owner polls");
assert.strictEqual(
  (await get(`/build/${jobId}`, asAliceRefreshed)).status,
  200,
  "a refreshed sign-in still owns its job",
);

// Bob's stop attempt must not have touched it.
const mid = JSON.parse((await get(`/build/${jobId}`, asAlice)).body);
assert.notStrictEqual(mid.state, "stopped", "bob's stop must not stop alice's build");

// It finishes, and the owner downloads it.
await until(async () => {
  const s = JSON.parse((await get(`/build/${jobId}`, asAlice)).body);
  return s.state === "done";
});
const file = await get(`/build/${jobId}/file`, asAlice);
assert.strictEqual(file.status, 200, "the owner downloads");
assert.strictEqual(file.body, "not-a-real-bundle", "and gets the actual bytes");

// Stop on a finished job: idempotent, truthful, twice.
for (const round of [1, 2]) {
  const stop = await post(`/build/${jobId}/stop`, {}, asAlice);
  assert.strictEqual(stop.status, 200, `stop round ${round}`);
  assert.strictEqual(JSON.parse(stop.body).state, "done", "stopping a finished build changes nothing");
}

/* ---- restart ------------------------------------------------------------- */
// Alice starts another build and the machine dies mid-build.
const second = await post("/build", { request: "a clock" }, asAlice);
const secondId = JSON.parse(second.body).id;
builder.kill("SIGKILL");
await new Promise((r) => builder.on("exit", r));

builder = startBuilder(stateDir, krateBin);
await until(async () => (await get("/health")).status === 200);

// The finished job survived: same id, still owned, still downloadable.
const revived = await get(`/build/${jobId}`, asAlice);
assert.strictEqual(revived.status, 200, "a finished job survives a restart");
assert.strictEqual(JSON.parse(revived.body).state, "done");
const fileAfter = await get(`/build/${jobId}/file`, asAlice);
assert.strictEqual(fileAfter.status, 200, "the file survives a restart");
assert.strictEqual(fileAfter.body, "not-a-real-bundle");

// The interrupted job is failed with a sentence, not forgotten and not
// "working" forever.
const lost = JSON.parse((await get(`/build/${secondId}`, asAlice)).body);
assert.strictEqual(lost.state, "failed", "a mid-build restart reads as failed");
assert.match(lost.error, /restarted/i, "and says the machine restarted");

// Ownership survives the restart too.
assert.strictEqual((await get(`/build/${jobId}`, asBob)).status, 404, "ownership survives restart");

/* ---- expiry -------------------------------------------------------------- */
builder.kill("SIGKILL");
await new Promise((r) => builder.on("exit", r));
builder = startBuilder(stateDir, krateBin, { KRATE_RESULT_TTL_MS: "1" });
await until(async () => (await get("/health")).status === 200);

const expired = JSON.parse((await get(`/build/${jobId}`, asAlice)).body);
assert.strictEqual(expired.state, "expired", "an old result expires");
assert.strictEqual((await get(`/build/${jobId}/file`, asAlice)).status, 404, "an expired file is gone");

/* ---- audit --------------------------------------------------------------- */
const log = await readFile(join(stateDir, "audit.log"), "utf8");
const actions = log.trim().split("\n").map((l) => JSON.parse(l));
for (const wanted of ["start", "done", "download", "denied", "expire"]) {
  assert.ok(
    actions.some((a) => a.action === wanted),
    `the audit log records ${wanted}: ${log}`,
  );
}
const denied = actions.find((a) => a.action === "denied");
assert.strictEqual(denied.account, "bob", "the denial names who was refused");
assert.strictEqual(denied.job, jobId, "and which job they reached for");

/* ---- the funded case (IC-001) -------------------------------------------- */
// The successful first build must have opened a case and recorded "made";
// the old counter route must not have been touched at all.
const opens = caseCalls.filter((c) => c.path === "/case/open");
const attempts = caseCalls.filter((c) => c.path === "/case/attempt");
assert.ok(opens.length >= 1, "a build opens a case");
assert.ok(
  attempts.some((c) => c.body.outcome === "made"),
  `the successful build records a made attempt: ${JSON.stringify(attempts)}`,
);

// A build that dies on our side records a free failure, never "made".
const before = attempts.length;
const broken = await post("/build", { request: "FAIL on purpose" }, asAlice);
assert.strictEqual(broken.status, 200, broken.body);
const brokenId = JSON.parse(broken.body).id;
await until(async () => {
  const s = JSON.parse((await get(`/build/${brokenId}`, asAlice)).body);
  return s.state === "failed";
});
await until(async () => caseCalls.filter((c) => c.path === "/case/attempt").length > before);
const failedAttempt = caseCalls.filter((c) => c.path === "/case/attempt").slice(before).pop();
assert.strictEqual(
  failedAttempt.body.outcome,
  "krate-failed",
  `an engine failure is recorded as krate-failed, free to retry: ${JSON.stringify(failedAttempt)}`,
);
assert.ok(
  !caseCalls.slice(-3).some((c) => c.body.outcome === "made"),
  "a failed build must never record made",
);

builder.kill("SIGKILL");
hub.close();
console.log("OK -- jobs are owned on every operation, survive restarts honestly, expire, leave an audit trail, and live inside funded cases");
