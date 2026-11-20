/* Every door of the real builder hands its children to the build user, with
 * only the key that door needs (K-1293).
 *
 * Drives the REAL server with a fake engine that writes down, for every
 * call, which user it ran as, its HOME and TMPDIR, and which secrets it
 * could see. The build user here is this very user (a test cannot be
 * root), mapped through KRATE_BUILD_PASSWD: the server takes the same uid
 * switching path it takes on Fly, and the HOME/TMPDIR it hands over prove
 * it did. The uid itself is pinned in sandbox.test.mjs.
 *
 * Per door, what the child may see:
 *   /plan, /build (create)         the agent's own key, nothing else
 *   the picture of a built app     no key
 *   /compile, /guide, /port/plan   no key
 *   /judge                         no key
 * and never, anywhere, the hub secret or another vendor's key.
 *
 * Then the production half: on a box with FLY_APP_NAME and no build user,
 * every build door answers 503 and the engine is never started.
 *
 *   node --test cloud/builder/test/build-user.test.mjs
 */
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, realpath, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer as probeServer } from "node:net";
import { CARGO_TEMPLATE } from "../src/compile.js";

const SECRET = "hub-secret-for-build-user-test";
const HOUSE_KEY = "sk-ant-house-key-for-build-user-test";
const OTHER_KEY = "sk-openai-house-key-for-build-user-test";
const FLY_TOKEN = "fly-token-for-build-user-test";

const freePort = () => new Promise((resolve, reject) => {
  const probe = probeServer();
  probe.once("error", reject);
  probe.listen(0, "127.0.0.1", () => { const { port } = probe.address(); probe.close(() => resolve(port)); });
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* ---- a hub that says yes to everything and serves one upload ------------- */
const UPLOAD = Buffer.from("a big app, judged on the builder");
const UPLOAD_SHA = createHash("sha256").update(UPLOAD).digest("hex");
const verdicts = [];
const hub = createServer((req, res) => {
  if (req.url.startsWith("/internal/big/") && req.url.endsWith("/bytes")) return res.end(UPLOAD);
  let raw = "";
  req.on("data", (c) => { raw += c; });
  req.on("end", () => {
    if (req.url.endsWith("/verdict")) verdicts.push(JSON.parse(raw));
    res.setHeader("content-type", "application/json");
    res.end("{}");
  });
});
await new Promise((r) => hub.listen(0, "127.0.0.1", r));
const HUB = `http://127.0.0.1:${hub.address().port}`;

/* ---- a fake engine that writes down who it ran as and what it saw -------- */
const work = await mkdtemp(join(tmpdir(), "krate-build-user-"));
const calls = join(work, "calls.log");
const krateBin = join(work, "krate");
await writeFile(krateBin, `#!/bin/sh
echo "$1|uid=$(id -u)|home=$HOME|tmp=$TMPDIR|user=\${USER:-}|secret=\${KRATE_BUILDER_SECRET:-none}|anthropic=\${ANTHROPIC_API_KEY:-none}|openai=\${OPENAI_API_KEY:-none}|fly=\${FLY_API_TOKEN:-none}|cwd=$(pwd)" >> "${calls}"
case "$1" in
  --version) echo "krate 9.9.9-test"; exit 0 ;;
  authoring-context) echo "# pack"; exit 0 ;;
  plan) echo '{"plan":"a small notes app","needs":[]}'; exit 0 ;;
  port) echo '{"verdict":"portable","source":"x"}'; exit 0 ;;
  judge) echo '{"ok":true,"judgement":{}}'; exit 0 ;;
  create)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "--output" ] && out="$a"; prev="$a"; done
    printf 'not-a-real-bundle' > "$out"
    exit 0 ;;
  run) exit 1 ;;
  check-app)
    dir="$2"
    crate=$(grep -m1 '^name' "$dir/Cargo.toml" | sed 's/.*"\\(.*\\)".*/\\1/' | tr '-' '_')
    mkdir -p "$dir/target/wasm32-wasip1/release"
    printf 'wasm' > "$dir/target/wasm32-wasip1/release/$crate.wasm"
    echo "{\\"ok\\":true,\\"dir\\":\\"$dir\\",\\"stages\\":[\\"build\\"],\\"imports\\":[],\\"usability_notes\\":[],\\"link_notes\\":[]}"
    exit 0 ;;
  pack)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
    case "$out" in
      *nudge.krate)
        mkdir -p "$HOME/.cache/krate/sdk/fake-sdk-1"
        touch "$HOME/.cache/krate/sdk/fake-sdk-1/.complete"
        exit 1 ;;
    esac
    printf 'PK-fake-bundle' > "$out"
    exit 0 ;;
esac
exit 1
`.replaceAll("${calls}", calls));
await chmod(krateBin, 0o755);

const started = [];
after(() => { for (const p of started) p.kill("SIGKILL"); hub.close(); });

async function startBuilder(extra) {
  const port = await freePort();
  const stateDir = await mkdtemp(join(work, "state-"));
  const proc = spawn("node", ["cloud/builder/src/server.js"], {
    env: {
      ...process.env,
      PORT: String(port),
      HOME: join(work, "service-home"),
      TMPDIR: join(work, "service-tmp"),
      KRATE_BIN: krateBin,
      KRATE_HUB: HUB,
      KRATE_STATE_DIR: stateDir,
      KRATE_AGENT: "anthropic",
      KRATE_BUILDER_DEV: "1",
      KRATE_BUILDER_SECRET: SECRET,
      ANTHROPIC_API_KEY: HOUSE_KEY,
      OPENAI_API_KEY: OTHER_KEY,
      FLY_API_TOKEN: FLY_TOKEN,
      ...extra,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  started.push(proc);
  let log = "";
  proc.stdout.on("data", (b) => { log += b; });
  proc.stderr.on("data", (b) => { log += b; });
  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i += 1) {
    try { if ((await fetch(`${base}/health`)).ok) break; } catch (e) {}
    await sleep(100);
  }
  return { base, proc, log: () => log };
}

async function engineCalls() {
  return (await readFile(calls, "utf8").catch(() => "")).split("\n").filter(Boolean).map((line) => {
    const [verb, ...fields] = line.split("|");
    return { verb, line, ...Object.fromEntries(fields.map((f) => [f.slice(0, f.indexOf("=")), f.slice(f.indexOf("=") + 1)])) };
  });
}
async function until(what, ms = 15000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { const v = await what(); if (v) return v; await sleep(100); }
  throw new Error("timed out");
}

const post = (base, path, body, headers = {}) => fetch(`${base}${path}`, {
  method: "POST",
  headers: { "content-type": "application/json", authorization: "Bearer dev", "x-krate-builder": SECRET, ...headers },
  body: JSON.stringify(body),
});
const b64 = (s) => Buffer.from(s).toString("base64");
const MANIFEST = `[app]
id = "dev.krate.your_app"
name = "Tide Clock"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/your_app.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "ui.window:create"
rationale = "Open the window"
required = true
`;

test("every door hands its children to the build user, with only the key that door needs", async () => {
  const me = process.getuid();
  const buildHome = join(work, "build-home");
  await mkdir(buildHome);
  const passwd = join(work, "passwd");
  await writeFile(passwd, `kratebuild:x:${me}:${process.getgid()}::${buildHome}:/bin/sh\n`);
  const b = await startBuilder({ KRATE_BUILD_PASSWD: passwd });
  const health = await (await fetch(`${b.base}/health`)).json();
  // This user, not root, so no boundary -- the server says so honestly.
  assert.equal(health.isolation, "none");

  // /plan: a model call on the house key.
  const plan = await post(b.base, "/plan", { request: "a notes app" });
  assert.equal(plan.status, 200, await plan.text());

  // /build: the agent on the house key, then the finished app photographed.
  const build = await post(b.base, "/build", { request: "a notes app" });
  assert.equal(build.status, 200, b.log());
  const { id } = await build.json();
  await until(async () => {
    const job = await (await fetch(`${b.base}/build/${id}`, { headers: { authorization: "Bearer dev" } })).json();
    return job.state === "done" || job.state === "failed" ? job : null;
  });

  // /compile: somebody else's source, no model at all.
  const compile = await post(b.base, "/compile", {
    name: "Tide Clock",
    files: [
      { path: "Cargo.toml", data: b64(CARGO_TEMPLATE) },
      { path: "manifest.toml", data: b64(MANIFEST) },
      { path: "src/lib.rs", data: b64("#![no_std]\nextern crate alloc;\n") },
    ],
  });
  assert.equal(compile.status, 202, b.log());
  const compiled = (await compile.json()).id;
  await until(async () => {
    const job = await (await fetch(`${b.base}/build/${compiled}`, { headers: { authorization: "Bearer dev" } })).json();
    return job.state === "done" || job.state === "failed" ? job : null;
  });

  // /guide, /port/plan, /judge: the engine reading, never a model.
  assert.equal((await fetch(`${b.base}/guide`, { headers: { authorization: "Bearer dev", "x-krate-builder": SECRET } })).status, 200, b.log());
  const portPlan = await post(b.base, "/port/plan", { project: { name: "p", files: [{ path: "main.py", bytes: b64("print(1)") }] } });
  assert.equal(portPlan.status, 200, b.log());
  const judged = await post(b.base, "/judge", { id: "bigupload1", kind: "bundle", size: UPLOAD.length, sha256: UPLOAD_SHA });
  assert.equal(judged.status, 202);
  await until(() => verdicts.length > 0);

  const seen = await engineCalls();
  const verbs = new Set(seen.map((c) => c.verb));
  for (const verb of ["plan", "create", "run", "pack", "check-app", "--version", "authoring-context", "port", "judge"]) {
    assert.ok(verbs.has(verb), `the engine was never asked to ${verb}: ${[...verbs].join(", ")}\n${b.log()}`);
  }
  for (const c of seen) {
    // Never, on any path.
    assert.equal(c.secret, "none", `the hub secret reached \`krate ${c.verb}\``);
    assert.equal(c.openai, "none", `another vendor's key reached \`krate ${c.verb}\``);
    assert.equal(c.fly, "none", `a Fly token reached \`krate ${c.verb}\``);
    assert.ok(!c.line.includes(SECRET), `the hub secret, by value, in \`krate ${c.verb}\``);
    if (c.verb === "gift-opener") {
      // The gift probe stays the service's user: its HOME, no key.
      assert.equal(c.anthropic, "none", "a gift needs no key");
      continue;
    }
    // The build user's own home and scratch, which only the switching path sets.
    assert.equal(c.uid, String(me));
    assert.equal(c.home, buildHome, `\`krate ${c.verb}\` ran outside the build user's home`);
    assert.equal(c.tmp, join(buildHome, "tmp"), `\`krate ${c.verb}\` got the service's TMPDIR`);
    assert.equal(c.user, "kratebuild");
    const wantsKey = c.verb === "plan" || c.verb === "create";
    assert.equal(c.anthropic, wantsKey ? HOUSE_KEY : "none", `\`krate ${c.verb}\` key`);
  }
  // A build's own folder is in the build user's scratch, not the service's.
  const created = seen.find((c) => c.verb === "create");
  assert.ok(created.cwd.startsWith(join(await realpath(buildHome), "tmp") + "/krate-build-"), created.cwd);
  b.proc.kill("SIGTERM");
});

test("production with no build user: every build door is refused and nothing runs", async () => {
  await writeFile(calls, "");
  const empty = join(work, "passwd-empty");
  await writeFile(empty, "root:x:0:0:root:/root:/bin/sh\n");
  const b = await startBuilder({ KRATE_BUILD_PASSWD: empty, FLY_APP_NAME: "krate-builder-test" });
  assert.equal((await (await fetch(`${b.base}/health`)).json()).isolation, "none");
  assert.match(b.log(), /refusing every build/);

  const doors = [
    ["/build", () => post(b.base, "/build", { request: "a notes app" })],
    ["/plan", () => post(b.base, "/plan", { request: "a notes app" })],
    ["/compile", () => post(b.base, "/compile", { name: "x", files: [] })],
    ["/port/plan", () => post(b.base, "/port/plan", { project: { name: "p", files: [{ path: "a.py", bytes: b64("x") }] } })],
    ["/port", () => post(b.base, "/port", { project: { name: "p", files: [{ path: "a.py", bytes: b64("x") }] } })],
    ["/judge", () => post(b.base, "/judge", { id: "bigupload2", kind: "bundle", size: UPLOAD.length, sha256: UPLOAD_SHA })],
    ["/guide", () => fetch(`${b.base}/guide`, { headers: { authorization: "Bearer dev", "x-krate-builder": SECRET } })],
    ["/build/x/revise", () => post(b.base, "/build/0123/revise", { change: "bigger" })],
  ];
  for (const [door, call] of doors) {
    const res = await call();
    assert.equal(res.status, 503, `${door} answered ${res.status}`);
    assert.match(await res.text(), /switched off/, door);
  }
  await sleep(300);
  const ran = (await engineCalls()).filter((c) => c.verb !== "gift-opener");
  assert.deepEqual(ran.map((c) => c.verb), [], "the engine ran on a box with no build user");
  b.proc.kill("SIGTERM");
});
