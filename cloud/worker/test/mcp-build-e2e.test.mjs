/* Krate in Claude, phase 2 (IC-989), the two halves together: the hub's
 * build tools against the REAL build service.
 *
 * mcp-build.test.mjs stands a stub in for the builder; this one starts
 * cloud/builder/src/server.js as a process, the way the builder's own tests
 * do, with a fake `krate` engine (no compiler, no model), and drives the
 * hub's MCP tools at it through the real worker: get_krate_guide, build_app,
 * build_status. What it pins is the contract between the two:
 *
 *   - the hub's x-krate-builder + x-krate-account: hub:<owner> is what the
 *     builder takes for /compile, /build/<id> and /build/<id>/file;
 *   - the builder's refusal words ({ error }) reach the model as words;
 *   - "building" reads as in progress, a failed check-app comes back as
 *     check-app's stage, detail and fix;
 *   - the file the builder hands back is unsigned and the hub signs it as
 *     the account, judges it and publishes it unlisted;
 *   - the builder counts its per-account limit on the vouched account.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/mcp-build-e2e.test.mjs
 */
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createServer as probeServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { orgEnv, call } from "./org-tools.mjs";
import { __forgetGuide } from "../src/mcp-build.js";
import { CARGO_TEMPLATE } from "../../builder/src/compile.js";

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, "..", "..", "..");
const FINISHED = join(here, "fixtures", "notes-0.1.0.krate");
const BASE = "https://hub.example";
const SECRET = "e2e-builder-secret";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* ---- the real builder, with a fake engine -------------------------------- */
const work = await mkdtemp(join(tmpdir(), "krate-mcp-e2e-"));
const calls = join(work, "calls.log");
const krateBin = join(work, "krate");
await writeFile(
  krateBin,
  `#!/bin/sh
echo "$1 | key=\${ANTHROPIC_API_KEY:+set} secret=\${KRATE_BUILDER_SECRET:+set}" >> "${calls}"
case "$1" in
  --version) echo "krate 9.9.9-e2e"; exit 0 ;;
  krate-mode)
    echo "# Krate Mode (e2e)"
    echo 'krate = { path = "PREFIX/crates/bindings-rust", features = ["gui"] }'
    echo "# The handoff: you cannot build this"
    exit 0 ;;
  check-app)
    dir="$2"
    if grep -q FAIL_BUILD "$dir/src/lib.rs"; then
      echo "{\\"ok\\":false,\\"dir\\":\\"$dir\\",\\"stage\\":\\"build\\",\\"kind\\":\\"compile\\",\\"detail\\":\\"error[E0425]: cannot find value x in $dir/src/lib.rs:2\\",\\"fix\\":\\"Declare x before using it.\\"}"
      exit 3
    fi
    crate=$(grep -m1 '^name' "$dir/Cargo.toml" | sed 's/.*"\\(.*\\)".*/\\1/' | tr '-' '_')
    mkdir -p "$dir/target/wasm32-wasip1/release"
    printf 'wasm' > "$dir/target/wasm32-wasip1/release/$crate.wasm"
    echo "{\\"ok\\":true,\\"dir\\":\\"$dir\\",\\"stages\\":[\\"build\\",\\"imports\\",\\"run\\"],\\"imports\\":[],\\"usability_notes\\":[],\\"link_notes\\":[]}"
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
    # A real, unsigned bundle: the hub signs, judges and admits it.
    cp "${FINISHED}" "$out"
    exit 0 ;;
esac
exit 1
`
    .replaceAll("${calls}", calls)
    .replaceAll("${FINISHED}", FINISHED),
);
await chmod(krateBin, 0o755);

const port = await new Promise((ok, fail) => {
  const probe = probeServer();
  probe.once("error", fail);
  probe.listen(0, "127.0.0.1", () => {
    const { port } = probe.address();
    probe.close(() => ok(port));
  });
});
const stateDir = await mkdtemp(join(work, "state-"));
const builderProc = spawn("node", [join(REPO, "cloud", "builder", "src", "server.js")], {
  cwd: REPO,
  env: {
    ...process.env,
    PORT: String(port),
    HOME: await mkdtemp(join(work, "home-")),
    XDG_CACHE_HOME: "",
    TMPDIR: await mkdtemp(join(work, "tmp-")),
    KRATE_BIN: krateBin,
    KRATE_HUB: "http://127.0.0.1:9",
    KRATE_STATE_DIR: stateDir,
    KRATE_AGENT: "anthropic",
    KRATE_BUILDER_SECRET: SECRET,
    ANTHROPIC_API_KEY: "sk-e2e-must-never-reach-a-child",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let builderLog = "";
builderProc.stdout.on("data", (b) => (builderLog += b));
builderProc.stderr.on("data", (b) => (builderLog += b));
after(() => builderProc.kill("SIGKILL"));
const BUILDER = `http://127.0.0.1:${port}`;
for (let i = 0; i < 100; i += 1) {
  try {
    if ((await fetch(`${BUILDER}/health`)).ok) break;
  } catch (_) {
    /* not up yet */
  }
  await sleep(100);
}

/* ---- GitHub's token exchange, stood in for; the builder is real ---------- */
let nextLogin = "alice";
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  if (url === "https://github.com/login/oauth/access_token") {
    return new Response(JSON.stringify({ access_token: `gho_${nextLogin}` }), { headers: { "content-type": "application/json" } });
  }
  return before(input, init);
};

const hubEnv = () => {
  const env = orgEnv({
    PUBLIC_BASE: BASE,
    KEY_WRAP_SECRET: "test-wrapping-secret",
    GITHUB_CLIENT_SECRET: "gh-secret",
    KRATE_BUILDER_URL: BUILDER,
    KRATE_BUILDER_SECRET: SECRET,
  });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

/* ---- a connector's sign-in, as mcp-build.test.mjs does it --------------- */
const b64url = (buf) => Buffer.from(buf).toString("base64url");
const REDIRECT = "https://claude.ai/api/mcp/auth_callback";
async function connect(env, login) {
  const reg = await call(env, null, "POST", "/oauth/register", { json: { client_name: "Claude", redirect_uris: [REDIRECT] } });
  assert.equal(reg.status, 201, JSON.stringify(reg.body));
  const client = reg.body;
  const verifier = b64url(randomBytes(48));
  const challenge = b64url(createHash("sha256").update(verifier).digest());
  const q = new URLSearchParams({
    response_type: "code",
    client_id: client.client_id,
    redirect_uri: REDIRECT,
    state: "st",
    code_challenge: challenge,
    code_challenge_method: "S256",
    scope: "krate.apps",
    resource: `${BASE}/mcp`,
  });
  const page = await call(env, null, "GET", `/oauth/authorize?${q}`);
  const req = JSON.parse(/const REQ=("[^"]+")/.exec(page.body)[1]);
  nextLogin = login;
  const nonce = randomBytes(16).toString("hex");
  const start = await call(env, null, "GET", `/login/start?from=oauth&nonce=${nonce}`);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  const back = await call(env, null, "GET", `/login/callback?state=${state}&code=abc`);
  const session = new URLSearchParams(back.headers.get("location").split("#")[1]).get("token");
  const yes = await call(env, null, "POST", "/oauth/approve", { json: { req, approve: true }, headers: { authorization: `Bearer ${session}` } });
  const code = new URL(yes.body.redirect).searchParams.get("code");
  const tok = await call(env, null, "POST", "/oauth/token", {
    bytes: new TextEncoder().encode(
      new URLSearchParams({ grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: client.client_id, code_verifier: verifier }).toString(),
    ),
    headers: { "content-type": "application/x-www-form-urlencoded" },
  });
  assert.equal(tok.status, 200, JSON.stringify(tok.body));
  return tok.body.access_token;
}

let nextId = 1;
const tool = (env, name, args, token) =>
  call(env, null, "POST", "/mcp", {
    json: { jsonrpc: "2.0", id: nextId++, method: "tools/call", params: { name, arguments: args } },
    headers: { accept: "application/json, text/event-stream", ...(token ? { authorization: `Bearer ${token}` } : {}) },
  });

const MANIFEST = `[app]
id = "dev.krate.notes"
name = "Notes"
version = "0.1.0"
entry = "target/wasm32-wasip1/release/notes.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "ui.window:create"
rationale = "Open the window"
required = true
`;
const files = (lib) => [
  { path: "Cargo.toml", content: CARGO_TEMPLATE.replace('name = "your-app"', 'name = "notes"').replace("krate:your-app", "krate:notes") },
  { path: "manifest.toml", content: MANIFEST },
  { path: "src/lib.rs", content: lib },
];

/* build_status until it stops saying "building". */
async function settle(env, id, token) {
  let s;
  for (let i = 0; i < 100; i += 1) {
    const r = await tool(env, "build_status", { build_id: id }, token);
    assert.equal(r.status, 200, JSON.stringify(r.body));
    s = r.body.result;
    if (s.structuredContent.status !== "building") return s;
    await sleep(100);
  }
  throw new Error(`still building: ${JSON.stringify(s)}\n${builderLog.slice(-2000)}`);
}

test("build_app -> build_status against the real builder: words, a fix, a published link", async () => {
  const env = hubEnv();
  const token = await connect(env, "alice");

  // The guide comes from the builder: Krate Mode, its handoff cut, then how
  // to send the files.
  __forgetGuide();
  const guide = await tool(env, "get_krate_guide", {});
  assert.equal(guide.status, 200);
  const text = guide.body.result.content[0].text;
  assert.match(text, /# Krate Mode \(e2e\)/);
  assert.match(text, /# Sending the app to Krate to build/);
  assert.doesNotMatch(text, /you cannot build this/);

  // Something the builder refuses before it builds: its own words reach the
  // model, not a JSON blob and not a status code.
  const refused = await tool(env, "build_app", { name: "Notes", files: [...files("// ok\n"), { path: "build.rs", content: "fn main(){}" }] }, token);
  assert.equal(refused.body.result.isError, true);
  assert.match(refused.body.result.content[0].text, /Krate could not start the build: build\.rs is not accepted: a build script runs on the build machine/);
  assert.doesNotMatch(refused.body.result.content[0].text, /\{"error"/);

  // A failing build: check-app's stage, detail and fix, through both halves.
  const first = await tool(env, "build_app", { name: "Notes", files: files("fn f() { x }\n// FAIL_BUILD\n") }, token);
  const s1 = first.body.result.structuredContent;
  assert.equal(s1.status, "building", JSON.stringify(first.body) + builderLog.slice(-2000));
  const failed = await settle(env, s1.build_id, token);
  assert.deepEqual(
    [failed.structuredContent.status, failed.structuredContent.stage, failed.structuredContent.fix],
    ["failed", "build", "Declare x before using it."],
  );
  assert.match(failed.structuredContent.detail, /cannot find value x in src\/lib\.rs:2/, "the writer's own paths, not the box's");
  assert.doesNotMatch(failed.content[0].text, /krate-compile-/);

  // The fix: built, the unsigned file signed by the hub as alice, judged and
  // published unlisted.
  const second = await tool(env, "build_app", { name: "Notes", files: files("#![no_std]\nextern crate alloc;\n") }, token);
  const id2 = second.body.result.structuredContent.build_id;
  assert.match(id2, /^[0-9a-f]{32}$/);
  const done = await settle(env, id2, token);
  const p = done.structuredContent;
  assert.equal(p.status, "published", JSON.stringify(done) + builderLog.slice(-2000));
  assert.match(p.link, /^https:\/\/hub\.example\/a\/v[0-9a-f]{11}$/);
  const row = JSON.parse(env._kv.get(`app:${p.app_id}`));
  assert.equal(row.unlisted, true);
  assert.equal(row.signature.state, "valid", "the hub signed the builder's unsigned file as the account");
  assert.equal(row.owner, "github:alice");

  // The builder kept the jobs to the vouched account and counted them there.
  const direct = await fetch(`${BUILDER}/build/${id2}`, { headers: { "x-krate-builder": SECRET, "x-krate-account": "hub:github:bob" } });
  assert.equal(direct.status, 404, "the builder does not show alice's job to bob's account");
  const ledger = JSON.parse(await readFile(join(stateDir, "compiles.json"), "utf8"));
  assert.equal(ledger.accounts["hub:github:alice"].length, 2, "two accepted compiles, on hub:github:alice");

  // Bob, through the hub, learns nothing of it.
  const bob = await connect(env, "bob");
  const asBob = await tool(env, "build_status", { build_id: id2 }, bob);
  assert.equal(asBob.body.result.structuredContent.status, "unknown");

  // Nothing the builder ran saw a key or the hub secret, and no model ran.
  for (const line of (await readFile(calls, "utf8")).split("\n").filter((l) => l && !l.startsWith("gift-opener"))) {
    assert.match(line, /key= secret=$/, line);
    assert.ok(["--version", "krate-mode", "check-app", "pack"].includes(line.split(" ")[0]), line);
  }
});
