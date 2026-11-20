/* Claude writes, Krate builds (IC-989 phase 2): POST /compile and GET /guide.
 *
 * The hub's connector sends source a chat model wrote; the builder checks
 * and packs it with `krate check-app` and `krate pack` and NO model of
 * ours. Drives the REAL server with a fake hub and a fake `krate`, like
 * port.test.mjs. Pinned here:
 *   - accepted source becomes a job that ends done, signed by the hub as
 *     the account, with the SDK placeholder resolved before the build;
 *   - a check-app failure comes back as check-app's own stage, words and fix;
 *   - traversal, links, build scripts, too many or too big files, no
 *     manifest, a git dependency or an include of /proc are refused before
 *     anything is written or spawned;
 *   - no hub secret, no compile; per-account limits per hour and per day,
 *     one at a time, and a wall clock;
 *   - nothing on this path ever runs a model, sees a key, or touches the
 *     AI allowance (no case, no plan count, no key, no spend).
 *
 *   node cloud/builder/test/compile.test.mjs   (from the repo root)
 */
import assert from "node:assert";
import test from "node:test";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer as probeServer } from "node:net";
import { CARGO_TEMPLATE } from "../src/compile.js";

const freePort = () => new Promise((resolve, reject) => {
  const probe = probeServer();
  probe.once("error", reject);
  probe.listen(0, "127.0.0.1", () => { const { port } = probe.address(); probe.close(() => resolve(port)); });
});
const BUILDER_PORT = await freePort();
const BUILDER = `http://127.0.0.1:${BUILDER_PORT}`;
const SECRET = "builder-secret-for-tests";
const MODEL_KEY = "sk-test-must-never-reach-a-child";

/* ---- a hub that knows four people and signs ------------------------------ */
const TOKENS = { "alice-token": "alice", "bob-token": "bob", "carol-token": "carol", "dave-token": "dave" };
const hubHits = [];
const hub = createServer((req, res) => {
  let raw = [];
  req.on("data", (c) => raw.push(c));
  req.on("end", () => {
    const body = Buffer.concat(raw);
    hubHits.push(req.url);
    const who = TOKENS[(req.headers.authorization || "").replace(/^Bearer\s+/i, "")];
    if (req.url === "/me") {
      if (!who) { res.statusCode = 401; return res.end("no"); }
      res.setHeader("content-type", "application/json");
      return res.end(JSON.stringify({ user: { login: who }, plan: { active: false } }));
    }
    if (req.url === "/sign/web-build") {
      if (req.headers["x-krate-builder"] !== SECRET || !who) { res.statusCode = 403; return res.end("no"); }
      return res.end(Buffer.concat([body, Buffer.from(`|SIGNED-AS-${who}`)]));
    }
    res.statusCode = 404;
    res.end("");
  });
});
await new Promise((r) => hub.listen(0, "127.0.0.1", r));
const HUB_PORT = hub.address().port;

/* ---- a fake engine that writes down every call --------------------------- */
const fake = await mkdtemp(join(tmpdir(), "krate-compile-fake-"));
const calls = join(fake, "calls.log");
const seen = join(fake, "seen");
await mkdir(seen);
const krateBin = join(fake, "krate");
await writeFile(krateBin, `#!/bin/sh
echo "$* | key=\${ANTHROPIC_API_KEY:+set} secret=\${KRATE_BUILDER_SECRET:+set}" >> "${calls}"
case "$1" in
  --version) echo "krate 9.9.9-test"; exit 0 ;;
  authoring-context)
    echo "# The Krate authoring pack (fake)"
    echo "files in the folder: $(ls -A "$2" | wc -l | tr -d ' ')"
    echo 'krate = { path = "<sdk>/crates/bindings-rust", features = ["gui"] }'
    exit 0 ;;
  check-app)
    dir="$2"
    cp "$dir/Cargo.toml" "${seen}/Cargo.toml"
    (cd "$dir" && find . -type f | sort) > "${seen}/files.txt"
    echo "   Compiling your-app v0.1.0-dev"
    if grep -q SLOW "$dir/src/lib.rs"; then sleep 20; fi
    if grep -q FAIL_BUILD "$dir/src/lib.rs"; then
      echo "{\\"ok\\":false,\\"dir\\":\\"$dir\\",\\"stage\\":\\"build\\",\\"kind\\":\\"compile\\",\\"detail\\":\\"error[E0425]: cannot find value x in $dir/src/lib.rs:3\\",\\"fix\\":\\"Define x before using it.\\"}"
      exit 3
    fi
    crate=$(grep -m1 '^name' "$dir/Cargo.toml" | sed 's/.*"\\(.*\\)".*/\\1/' | tr '-' '_')
    mkdir -p "$dir/target/wasm32-wasip1/release"
    printf 'wasm' > "$dir/target/wasm32-wasip1/release/$crate.wasm"
    echo "{\\"ok\\":true,\\"dir\\":\\"$dir\\",\\"stages\\":[\\"layout\\",\\"manifest\\",\\"build\\",\\"imports\\",\\"run\\"],\\"imports\\":[\\"krate:ui/window\\"],\\"usability_notes\\":[],\\"link_notes\\":[]}"
    exit 0 ;;
  pack)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
    case "$out" in
      *nudge.krate)
        # What the real engine does first: lay its SDK down in the cache.
        mkdir -p "\${XDG_CACHE_HOME:-$HOME/.cache}/krate/sdk/fake-sdk-1"
        touch "\${XDG_CACHE_HOME:-$HOME/.cache}/krate/sdk/fake-sdk-1/.complete"
        echo "not a component" >&2
        exit 1 ;;
    esac
    echo "$2" > "${seen}/packed-from.txt"
    printf 'PK-fake-bundle' > "$out"
    exit 0 ;;
esac
echo "unexpected engine call: $*" >&2
exit 1
`.replaceAll("${calls}", calls).replaceAll("${seen}", seen));
await chmod(krateBin, 0o755);

const home = await mkdtemp(join(tmpdir(), "krate-compile-home-"));
const scratch = await mkdtemp(join(tmpdir(), "krate-compile-tmp-"));
const stateDir = await mkdtemp(join(tmpdir(), "krate-compile-state-"));
// dave has used his day already, written before the server started: the
// ledger lives on the volume, so a wake does not forget it.
const twoHoursAgo = Date.now() - 2 * 60 * 60 * 1000;
await writeFile(join(stateDir, "compiles.json"), JSON.stringify({ accounts: { dave: [twoHoursAgo, twoHoursAgo + 1, twoHoursAgo + 2, twoHoursAgo + 3] } }));

const server = spawn("node", ["cloud/builder/src/server.js"], {
  env: {
    ...process.env,
    PORT: String(BUILDER_PORT),
    HOME: home,
    XDG_CACHE_HOME: "",
    TMPDIR: scratch,
    KRATE_HUB: `http://127.0.0.1:${HUB_PORT}`,
    KRATE_BUILDER_SECRET: SECRET,
    KRATE_BIN: krateBin,
    KRATE_STATE_DIR: stateDir,
    KRATE_AGENT: "anthropic",
    ANTHROPIC_API_KEY: MODEL_KEY,
    KRATE_COMPILE_PER_HOUR: "3",
    KRATE_COMPILE_PER_DAY: "4",
    KRATE_COMPILE_TIMEOUT_MS: "4000",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let serverLog = "";
server.stdout.on("data", (b) => { serverLog += b; });
server.stderr.on("data", (b) => { serverLog += b; });
for (let i = 0; i < 100; i += 1) {
  try { if ((await fetch(`${BUILDER}/health`)).ok) break; } catch (e) {}
  await new Promise((r) => setTimeout(r, 100));
}

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

[[capabilities]]
cap = "store.kv"
rationale = "Remember the harbour"
required = false
`;
const app = (lib = "#![no_std]\nextern crate alloc;\n", extra = []) => [
  { path: "Cargo.toml", data: b64(CARGO_TEMPLATE) },
  { path: "manifest.toml", data: b64(MANIFEST) },
  { path: "src/lib.rs", data: b64(lib) },
  ...extra,
];
const compile = (body, { token = "alice-token", secret = SECRET } = {}) => fetch(`${BUILDER}/compile`, {
  method: "POST",
  headers: {
    "content-type": "application/json",
    authorization: `Bearer ${token}`,
    ...(secret ? { "x-krate-builder": secret } : {}),
  },
  body: JSON.stringify(body),
});
const status = (id, token = "alice-token") => fetch(`${BUILDER}/build/${id}`, { headers: { authorization: `Bearer ${token}` } }).then((r) => r.json());
async function settled(id, token) {
  let job;
  for (let i = 0; i < 150; i += 1) {
    job = await status(id, token);
    if (job.state !== "building") return job;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`still building: ${JSON.stringify(job)}\n${serverLog.slice(-2000)}`);
}
// Every engine run but the server's own boot probe for gifts (gift.js asks
// `gift-opener` once, whatever anybody does), which is not this path.
async function engineCalls() {
  return (await readFile(calls, "utf8").catch(() => "")).split("\n").filter((l) => l && !l.startsWith("gift-opener "));
}
async function compileDirs() {
  return (await readdir(scratch)).filter((n) => n.startsWith("krate-compile-"));
}

test("no hub secret, no compile; a wrong one or no account is refused too", async () => {
  for (const [opts, code] of [[{ secret: null }, 403], [{ secret: "guess" }, 403], [{ token: "nobody" }, 401]]) {
    const res = await compile({ name: "x", files: app() }, opts);
    assert.equal(res.status, code, JSON.stringify(opts));
    assert.ok((await res.json()).error, "refusals carry { error }");
  }
  const guide = await fetch(`${BUILDER}/guide`, { headers: { authorization: "Bearer alice-token" } });
  assert.equal(guide.status, 403, "the guide is the hub's too");
});

test("bad uploads are refused before anything is written or spawned", async () => {
  const cases = [
    [[{ path: "../escape.rs", data: b64("x") }], 400, /outside/],
    [[{ path: "/etc/passwd", data: b64("x") }], 400, /relative/],
    [[{ path: "src/../../x.rs", data: b64("x") }], 400, /outside/],
    [[{ path: "src//lib.rs", data: b64("x") }], 400, /outside/],
    [[{ path: "build.rs", data: b64("fn main(){}") }], 400, /build script/],
    [[{ path: ".cargo/config.toml", data: b64("[build]\nrustc-wrapper='/bin/sh'") }], 400, /\.cargo/],
    [[{ path: "target/wasm32-wasip1/release/your_app.wasm", data: b64("x") }], 400, /build output/],
    [[{ path: "src/evil.rs", type: "symlink", target: "/etc/passwd", data: "" }], 400, /link/],
    [[{ path: "notes.txt", data: b64("x") }], 400, /not part of a Krate app/],
  ];
  for (const [extra, code, words] of cases) {
    const res = await compile({ name: "x", files: app(undefined, extra) });
    const body = await res.json();
    assert.equal(res.status, code, `${extra[0].path}: ${JSON.stringify(body)}`);
    assert.match(body.error, words, extra[0].path);
  }
  // No manifest is not a Krate app; a manifest with no [app] neither.
  let res = await compile({ files: app().filter((f) => f.path !== "manifest.toml") });
  assert.equal(res.status, 400);
  assert.match((await res.json()).error, /no manifest\.toml/);
  res = await compile({ files: [...app().filter((f) => f.path !== "manifest.toml"), { path: "manifest.toml", data: b64("[package]\nname='x'\n") }] });
  assert.equal(res.status, 400);
  // Too many files, one file too big, too much in total.
  const many = Array.from({ length: 601 }, (_, i) => ({ path: `assets/f${i}.txt`, data: b64("x") }));
  res = await compile({ files: app(undefined, many) });
  assert.equal(res.status, 413, "601 files");
  const big = Buffer.alloc(2 * 1024 * 1024 + 1, 1).toString("base64");
  res = await compile({ files: app(undefined, [{ path: "assets/big.bin", data: big }]) });
  assert.equal(res.status, 413, "one file over 2 MB");
  const chunk = Buffer.alloc(2 * 1024 * 1024, 2).toString("base64");
  res = await compile({ files: app(undefined, Array.from({ length: 9 }, (_, i) => ({ path: `assets/c${i}.bin`, data: chunk }))) });
  assert.equal(res.status, 413, "18 MB in all");
  // Cargo.toml asking cargo to fetch or run something of the uploader's.
  const withCargo = (text) => [{ path: "Cargo.toml", data: b64(text) }, ...app().filter((f) => f.path !== "Cargo.toml")];
  for (const cargo of [
    CARGO_TEMPLATE.replace("[dependencies]", '[dependencies]\nevil = { git = "https://example.com/evil" }'),
    CARGO_TEMPLATE.replace("[dependencies]", '[dependencies]\nserde = "1"'),
    CARGO_TEMPLATE.replace("[workspace]", '[workspace]\n\n[build-dependencies]\ncc = "1"\n'),
    CARGO_TEMPLATE.replace('rust-version = "1.91"', 'rust-version = "1.91"\nbuild = "src/sneaky.rs"'),
    CARGO_TEMPLATE.replace("{KRATE_SDK}/crates/bindings-rust", "/etc"),
  ]) {
    res = await compile({ files: withCargo(cargo) });
    const said = await res.text();
    assert.equal(res.status, 400, said);
  }
  // Reading the service's own environment into the app at compile time.
  for (const lib of ['const S: &str = include_str!("/proc/1/environ");', 'const S: &[u8] = include_bytes!(concat!("/proc/", "1/environ"));', '#[path = "../../../../state/audit.log"] mod a;']) {
    res = await compile({ files: app(lib) });
    assert.equal(res.status, 400, lib);
  }
  assert.deepEqual(await compileDirs(), [], "no job folder was made for any refusal");
  assert.deepEqual(await engineCalls(), [], "the engine was never run for a refusal");
});

test("accepted source is checked, packed and signed as the account, with the SDK resolved", async () => {
  const res = await compile({ name: "Tide clock", files: app(undefined, [{ path: "assets/icon.png", data: b64("png") }, { path: "src/tides.rs", data: b64("pub fn f(){}") }]) });
  const body = await res.json();
  assert.equal(res.status, 202, JSON.stringify(body));
  assert.match(body.id, /^[0-9a-f]{32}$/);
  const job = await settled(body.id);
  assert.equal(job.state, "done", JSON.stringify(job) + serverLog.slice(-2000));
  assert.equal(job.kind, "compile");
  assert.equal(job.result.name, "Tide clock");
  assert.deepEqual(job.result.asks, ["ui.window:create", "store.kv"]);
  assert.equal(job.check.ok, true);
  assert.equal(job.check.dir, undefined, "the box's own folder is not sent back");
  assert.equal(job.result.download, `/build/${body.id}/file`);

  const file = await fetch(`${BUILDER}${job.result.download}`, { headers: { authorization: "Bearer alice-token" } });
  assert.equal(file.status, 200);
  assert.equal(Buffer.from(await file.arrayBuffer()).toString(), "PK-fake-bundle|SIGNED-AS-alice", "signed by the hub as the person");
  // Somebody else cannot see or take it.
  assert.equal((await fetch(`${BUILDER}/build/${body.id}/file`, { headers: { authorization: "Bearer bob-token" } })).status, 404);

  const cargo = await readFile(join(seen, "Cargo.toml"), "utf8");
  assert.doesNotMatch(cargo, /\{KRATE_SDK\}/, "the placeholder was resolved before the build");
  assert.match(cargo, new RegExp(`${home}/\\.cache/krate/sdk/fake-sdk-1/crates/bindings-rust`), "to the SDK the engine laid down");
  const files = await readFile(join(seen, "files.txt"), "utf8");
  for (const f of ["./Cargo.toml", "./manifest.toml", "./src/lib.rs", "./src/tides.rs", "./assets/icon.png"]) assert.match(files, new RegExp(f.replace(/\./g, "\\.")));
  assert.match(await readFile(join(seen, "packed-from.txt"), "utf8"), /target\/wasm32-wasip1\/release\/your_app\.wasm/, "packed from the component check-app built");
  // A compiled app's change goes back through the person's own AI.
  const revise = await fetch(`${BUILDER}/build/${body.id}/revise`, {
    method: "POST", headers: { "content-type": "application/json", authorization: "Bearer alice-token" }, body: JSON.stringify({ change: "make it blue" }),
  });
  assert.equal(revise.status, 409);
  for (let i = 0; i < 50 && (await compileDirs()).length; i += 1) await new Promise((r) => setTimeout(r, 50));
  assert.deepEqual(await compileDirs(), [], "the job folder is gone once the bundle is kept");
});

test("a check-app failure answers with its own stage, words and fix", async () => {
  const res = await compile({ name: "Broken", files: app("fn f() { x }\n// FAIL_BUILD\n") }, { token: "bob-token" });
  assert.equal(res.status, 202);
  const job = await settled((await res.json()).id, "bob-token");
  assert.equal(job.state, "failed");
  assert.equal(job.check.ok, false);
  assert.equal(job.check.stage, "build");
  assert.equal(job.check.fix, "Define x before using it.");
  assert.match(job.check.detail, /cannot find value x in src\/lib\.rs:3/, "paths read as the writer's own");
  assert.doesNotMatch(JSON.stringify(job.check), /krate-compile-/, "no path on this box leaks");
  assert.match(job.error, /build stage/);
  assert.equal(job.result, null);
});

test("limits: one at a time, so many an hour and a day, and a wall clock", async () => {
  // One at a time, and a compile past the clock is stopped and said so.
  const slow = await compile({ files: app("// SLOW\n") }, { token: "carol-token" });
  assert.equal(slow.status, 202);
  const second = await compile({ files: app() }, { token: "carol-token" });
  assert.equal(second.status, 429, "a second compile while one runs");
  assert.match((await second.json()).error, /already being built/);
  const timedOut = await settled((await slow.json()).id, "carol-token");
  assert.equal(timedOut.state, "failed");
  assert.match(timedOut.error, /took longer than/);
  // carol has used one; two more fit the hour of three, the fourth does not.
  for (let i = 0; i < 2; i += 1) {
    const ok = await compile({ files: app() }, { token: "carol-token" });
    assert.equal(ok.status, 202);
    assert.equal((await settled((await ok.json()).id, "carol-token")).state, "done");
  }
  const over = await compile({ files: app() }, { token: "carol-token" });
  assert.equal(over.status, 429);
  assert.match((await over.json()).error, /3 builds in an hour/);
  // dave's four from two hours ago were read back from the volume.
  const daily = await compile({ files: app() }, { token: "dave-token" });
  assert.equal(daily.status, 429);
  assert.match((await daily.json()).error, /4 builds today/);
  // A refused upload never used one up: alice is still under her hour.
  const saved = JSON.parse(await readFile(join(stateDir, "compiles.json"), "utf8"));
  assert.equal(saved.accounts.alice.length, 1, "only the accepted compile was counted");
  assert.equal(saved.accounts.carol.length, 3);
});

test("the guide is the engine's authoring pack for a fresh folder, then how to send source, made once", async () => {
  const get = () => fetch(`${BUILDER}/guide`, { headers: { authorization: "Bearer alice-token", "x-krate-builder": SECRET } });
  const first = await get();
  assert.equal(first.status, 200);
  assert.match(first.headers.get("content-type"), /text\/markdown/);
  const text = await first.text();
  assert.match(text, /^# The Krate authoring pack \(fake\)/);
  assert.match(text, /files in the folder: 0/, "a fresh, empty folder");
  assert.match(text, /# Sending the app to Krate to build/);
  assert.match(text, /\{KRATE_SDK\}\/crates\/bindings-rust/);
  assert.match(text, /wit-bindgen-rt, libm/);
  assert.equal(await (await get()).text(), text);
  const packs = (await engineCalls()).filter((c) => c.startsWith("authoring-context"));
  assert.equal(packs.length, 1, "computed once per engine version");
});

test("nothing on this path runs a model, sees a key, or touches the AI allowance", async () => {
  const all = await engineCalls();
  assert.ok(all.length >= 6, all.join("\n"));
  for (const call of all) {
    const [args, env] = call.split(" | ");
    const sub = args.split(" ")[0];
    assert.ok(["--version", "authoring-context", "check-app", "pack"].includes(sub), `unexpected engine command: ${args}`);
    assert.doesNotMatch(args, /--agent|--author-cmd/, args);
    assert.equal(env, "key= secret=", `a child saw a secret: ${call}`);
  }
  for (const door of ["/case/open", "/case/attempt", "/plan/get", "/keys/use", "/spend"]) {
    assert.ok(!hubHits.includes(door), `the hub was asked ${door}`);
  }
});

test("the Cargo.toml in the guide is the one `krate create` writes", async () => {
  const rust = await readFile("crates/author/src/lib.rs", "utf8");
  const fn = rust.indexOf("fn checklist_cargo_toml(");
  assert.ok(fn > 0, "checklist_cargo_toml is where it was");
  const start = rust.indexOf('r#"', fn) + 3;
  const end = rust.indexOf('"#', start);
  const written = rust.slice(start, end)
    .replaceAll("{{", "{").replaceAll("}}", "}")
    .replaceAll("{sdk_prefix}", "{KRATE_SDK}")
    .replaceAll("{name}", "your-app");
  assert.equal(CARGO_TEMPLATE, written, "the guide's Cargo.toml drifted from the starter's");
});

test.after(() => { server.kill("SIGKILL"); hub.close(); });
