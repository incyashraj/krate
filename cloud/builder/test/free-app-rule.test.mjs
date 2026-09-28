/* One free app per account on Krate's key; as many as you like on your own.
 *
 * The web's whole bargain, driven end to end with the REAL hub and the REAL
 * build service talking to each other -- not a stub hub that answers what
 * the test wants. The builder's own tests stub the hub; the hub's own tests
 * never see a build. The rule lives in the seam between them (the builder
 * asks the hub what an account has made, and asks the hub for the person's
 * own key), so this is the one place it is checked whole:
 *
 *   1. a signed-in person's first app is made, on Krate's key
 *   2. their second new app is refused, and the refusal offers the two ways
 *      on: their own key here, or Studio on their own computer
 *   3. once they add their own key, they make app after app, on THEIR key
 *   4. take the key away and the wall is back
 *   5. another person still gets their own first app
 *   6. a build that fails does not use up the free app
 *
 * The engine is a fake that "builds" in half a second and prints which key
 * it ran with, so the test can see whose money each build spent.
 *
 *   node cloud/builder/test/free-app-rule.test.mjs
 */
import assert from "node:assert";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, writeFile } from "node:fs/promises";
import { readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import worker from "../../worker/src/index.js";
import { r2Mock } from "../../worker/test/r2-mock.mjs";

const HUB_PORT = 8941;
const BUILDER_PORT = 8942;
const HUB = `http://127.0.0.1:${HUB_PORT}`;
const BUILDER = `http://127.0.0.1:${BUILDER_PORT}`;
const BUILDER_SECRET = "free-app-rule-builder-secret";
const HOUSE_KEY = "sk-ant-krate-house-key-for-tests-0000";

/* ---- the real hub, in memory ------------------------------------------- */
const KV = new Map();
const hubEnv = {
  APPS: {
    get: async (k, t) => {
      const v = KV.get(k) ?? null;
      return v && t === "json" ? JSON.parse(v) : v;
    },
    put: async (k, v) => { KV.set(k, typeof v === "string" ? v : String(v)); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix = "", limit = 1000 } = {}) => ({
      keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).slice(0, limit).map((name) => ({ name })),
      list_complete: true,
    }),
  },
  BUNDLES: r2Mock(),
  PUBLIC_BASE: HUB,
  KEY_WRAP_SECRET: "free-app-rule-wrap-secret",
  KRATE_BUILDER_SECRET: BUILDER_SECRET,
};
for (const who of ["alice", "bob", "carol"]) {
  KV.set(`session:krs_${who}`, `u-${who}`);
  KV.set(`user:u-${who}`, JSON.stringify({ id: `u-${who}`, login: who }));
}

const hub = createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const body = Buffer.concat(chunks);
  const request = new Request(`${HUB}${req.url}`, {
    method: req.method,
    headers: req.headers,
    body: ["GET", "HEAD"].includes(req.method) ? undefined : body,
  });
  const out = await worker.fetch(request, hubEnv, { waitUntil() {}, passThroughOnException() {} });
  res.statusCode = out.status;
  out.headers.forEach((v, k) => res.setHeader(k, v));
  res.end(Buffer.from(await out.arrayBuffer()));
});

/* ---- an engine that says whose key it ran on ---------------------------- */
async function fakeKrate(dir) {
  const bin = join(dir, "krate");
  await writeFile(
    bin,
    `#!/bin/sh
if [ "$1" = "create" ]; then
  case "$*" in *FAIL*) echo "the engine fell over" >&2; exit 1;; esac
  out=""; transcript=""; prev=""
  for a in "$@"; do
    [ "$prev" = "--output" ] && out="$a"
    [ "$prev" = "--transcript" ] && transcript="$a"
    prev="$a"
  done
  sleep 0.3
  printf 'not-a-real-bundle' > "$out"
  # Which key this build was handed, beside the request that asked.
  req=""; after=""
  for a in "$@"; do [ "$after" = "1" ] && req="$a"; [ "$a" = "--" ] && after=1; done
  [ -n "$KRATE_KEYLOG" ] && echo "$req|\${ANTHROPIC_API_KEY:-none}" >> "$KRATE_KEYLOG"
  echo 'krate-spend: {"usd": 0.40, "model": "claude-opus-5", "rounds": 3}' >&2
  [ -n "$transcript" ] && printf '{"ok":true,"requested_permissions":["ui.window:create"],"verdict":"authored a working, permission-gated .krate that serves the request"}' > "$transcript"
  exit 0
fi
if [ "$1" = "revise" ]; then
  src=""; out=""; prev=""; after=""; change=""
  for a in "$@"; do
    [ "$prev" = "--output" ] && out="$a"
    if [ "$after" = "1" ]; then if [ -z "$src" ]; then src="$a"; else change="$a"; fi; fi
    [ "$a" = "--" ] && after=1
    prev="$a"
  done
  sleep 0.3
  printf '%s-changed' "$(cat "$src")" > "\${out:-$src}"
  [ -n "$KRATE_KEYLOG" ] && echo "$change|\${ANTHROPIC_API_KEY:-none}" >> "$KRATE_KEYLOG"
  exit 0
fi
exit 1
`,
  );
  await chmod(bin, 0o755);
  return bin;
}

/* ---- driving it the way a browser does ---------------------------------- */
const token = (who) => `krs_${who}`;

async function startBuild(who, request) {
  const res = await fetch(`${BUILDER}/build`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token(who)}` },
    body: JSON.stringify({ request, device: `device-${who}` }),
  });
  const text = await res.text();
  let body = null;
  try { body = JSON.parse(text); } catch (_) { body = { text }; }
  return { status: res.status, body };
}

async function finish(who, id) {
  for (let i = 0; i < 100; i++) {
    const res = await fetch(`${BUILDER}/build/${id}`, { headers: { authorization: `Bearer ${token(who)}` } });
    const job = await res.json();
    if (job.state === "done" || job.state === "failed") return job;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`build ${id} never finished`);
}

async function make(who, request) {
  const started = await startBuild(who, request);
  if (started.status !== 200) return { refused: started };
  return { job: { ...(await finish(who, started.body.id)), id: started.body.id } };
}

async function change(who, parentId, words) {
  const res = await fetch(`${BUILDER}/build/${parentId}/revise`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token(who)}` },
    body: JSON.stringify({ change: words, device: `device-${who}` }),
  });
  const text = await res.text();
  let body = null;
  try { body = JSON.parse(text); } catch (_) { body = { text }; }
  if (res.status !== 200) return { refused: { status: res.status, body } };
  return { job: { ...(await finish(who, body.id)), id: body.id } };
}

async function hubPost(who, path, body) {
  const res = await fetch(`${HUB}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token(who)}` },
    body: JSON.stringify(body),
  });
  return { status: res.status, text: await res.text() };
}

/* whose key the build for `request` ran on, as the engine was handed it */
let KEYLOG = "";
function keyUsed(request) {
  const lines = readFileSync(KEYLOG, "utf8").trim().split("\n");
  const line = lines.reverse().find((l) => l.startsWith(`${request}|`));
  return line ? line.slice(request.length + 1) : null;
}

/* what the real hub recorded as paid for this person */
function paidBy(who) {
  const rows = [];
  for (const [k, v] of KV) if (k.startsWith("spend:") && k.includes(who)) rows.push(v);
  return rows.join("\n");
}

async function run() {
  await new Promise((r) => hub.listen(HUB_PORT, r));
  const dir = await mkdtemp(join(tmpdir(), "krate-free-app-"));
  KEYLOG = join(dir, "keys.log");
  writeFileSync(KEYLOG, "");
  const builder = spawn("node", ["cloud/builder/src/server.js"], {
    env: {
      ...process.env,
      PORT: String(BUILDER_PORT),
      KRATE_BIN: await fakeKrate(dir),
      KRATE_HUB: HUB,
      KRATE_STATE_DIR: join(dir, "state"),
      // Production's configuration: the API agent, on Krate's own key.
      KRATE_AGENT: "anthropic",
      ANTHROPIC_API_KEY: HOUSE_KEY,
      KRATE_BUILDER_SECRET: BUILDER_SECRET,
      KRATE_KEYLOG: KEYLOG,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let builderLog = "";
  builder.stdout.on("data", (b) => { builderLog += b; });
  builder.stderr.on("data", (b) => { builderLog += b; });
  try {
    for (let i = 0; i < 100; i++) {
      try { if ((await fetch(`${BUILDER}/health`)).ok) break; } catch (_) {}
      await new Promise((r) => setTimeout(r, 100));
    }

    // 1. The first app is on Krate.
    const first = await make("alice", "a notes app");
    assert.ok(first.job, `alice's first app was refused: ${JSON.stringify(first.refused)}`);
    assert.strictEqual(first.job.state, "done", `alice's first app did not finish: ${JSON.stringify(first.job)}`);
    assert.strictEqual(keyUsed("a notes app"), HOUSE_KEY, "the free app runs on Krate's key");

    // 2. The second new app meets the wall, which names both ways on.
    const second = await make("alice", "a second, different app");
    assert.ok(second.refused, `alice's second funded app was built: ${JSON.stringify(second.job)}`);
    assert.strictEqual(second.refused.status, 402, `the wall answers 402: ${JSON.stringify(second.refused)}`);
    const wall = String(second.refused.body.message || "");
    assert.match(wall, /own API key/i, `the wall offers bringing a key: ${wall}`);
    assert.match(wall, /Studio/i, `the wall offers Studio on their computer: ${wall}`);
    assert.doesNotMatch(wall, /\$\d/, `the wall quotes no price: ${wall}`);

    // 1b. The free app comes with one free change, on Krate's key.
    const firstChange = await change("alice", first.job.id, "make it blue");
    assert.ok(firstChange.job && firstChange.job.state === "done",
      `alice's free change was refused: ${JSON.stringify(firstChange)}`);
    assert.strictEqual(keyUsed("make it blue"), HOUSE_KEY, "the free change runs on Krate's key");
    // ... and a second change meets the same kind of wall.
    const secondChange = await change("alice", firstChange.job.id, "and make it bigger");
    assert.ok(secondChange.refused && secondChange.refused.status === 402,
      `alice's second funded change went through: ${JSON.stringify(secondChange)}`);

    // 3. With their own key they make as many as they like, on their key.
    const saved = await hubPost("alice", "/keys", { vendor: "anthropic", key: "sk-ant-alice-own-key-0123456789" });
    assert.strictEqual(saved.status, 200, `saving a key failed: ${saved.status} ${saved.text}`);
    for (const n of [1, 2, 3]) {
      const own = await make("alice", `app number ${n} on my own key`);
      assert.ok(own.job, `app ${n} on alice's own key was refused: ${JSON.stringify(own.refused)}`);
      assert.strictEqual(own.job.state, "done", `app ${n} on her key did not finish`);
      assert.strictEqual(keyUsed(`app number ${n} on my own key`), "sk-ant-alice-own-key-0123456789",
        `app ${n} must run on HER key, never Krate's`);
    }

    // 3b. And change them as often as they like, on their key.
    for (const n of [1, 2]) {
      const words = `own-key change ${n}`;
      const c = await change("alice", firstChange.job.id, words);
      assert.ok(c.job && c.job.state === "done", `change ${n} on alice's own key was refused: ${JSON.stringify(c)}`);
      assert.strictEqual(keyUsed(words), "sk-ant-alice-own-key-0123456789", `change ${n} runs on HER key`);
    }

    // 4. Take the key away and the wall is back.
    const forgot = await hubPost("alice", "/keys/forget", { vendor: "anthropic" });
    assert.strictEqual(forgot.status, 200, `forgetting the key failed: ${forgot.text}`);
    const after = await make("alice", "one more without a key");
    assert.ok(after.refused && after.refused.status === 402,
      `without her key alice got another funded app: ${JSON.stringify(after)}`);

    // 5. Somebody else still has their own first app.
    const bob = await make("bob", "a habit tracker");
    assert.ok(bob.job && bob.job.state === "done", `bob's first app was refused: ${JSON.stringify(bob)}`);

    // 6. A failed build keeps the free app.
    const failed = await make("carol", "FAIL a timer");
    assert.ok(failed.job, `carol's first attempt was refused outright: ${JSON.stringify(failed.refused)}`);
    assert.strictEqual(failed.job.state, "failed", "the engine failed, so the build failed");
    const retry = await make("carol", "a timer");
    assert.ok(retry.job && retry.job.state === "done",
      `a failed build used up carol's free app: ${JSON.stringify(retry)}`);

    console.log("ok  the first app is on Krate's key, the second meets a wall that offers a key or Studio");
    console.log("ok  with their own key a person makes app after app, all on their key");
    console.log("ok  the free app has one free change; on their own key changes are unlimited too");
    console.log("ok  without the key the wall is back; others keep their free app; a failure keeps it");
  } catch (err) {
    process.stderr.write(builderLog.slice(-4000));
    throw err;
  } finally {
    builder.kill();
    hub.close();
  }
}

run().then(() => process.exit(0), (err) => { console.error(err); process.exit(1); });
