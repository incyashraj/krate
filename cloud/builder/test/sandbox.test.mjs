/* The one door every build-time child goes through (K-1293).
 *
 * A build runs code nobody here wrote -- a dependency's build script, a
 * proc-macro, `include_str!("/proc/1/environ")` -- and it used to run as
 * the service's own user, able to read the hub secret and the Anthropic key
 * out of /proc. These pin the three guards: the child gets the build user's
 * uid and gid, an environment built from an allow-list with a key only when
 * the caller hands one over, and in production no build at all when there
 * is no build user to hand it to.
 *
 *   node --test cloud/builder/test/sandbox.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { lstat, mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readFileSync } from "node:fs";
import { createSandbox, readPasswd } from "../src/sandbox.js";

const dir = await mkdtemp(join(tmpdir(), "krate-sandbox-test-"));
const passwd = join(dir, "passwd");
await writeFile(passwd, [
  "root:x:0:0:root:/root:/bin/bash",
  "kratebuild:x:10001:10001::/work/build:/usr/sbin/nologin",
  "",
].join("\n"));
const noUser = join(dir, "passwd-without");
await writeFile(noUser, "root:x:0:0:root:/root:/bin/bash\n");

// What the builder's environment holds on Fly: the toolchain, the engine's
// settings, and the secrets no build may see.
const SERVICE_ENV = {
  PATH: "/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin",
  HOME: "/work",
  TMPDIR: "/work/tmp",
  RUSTUP_HOME: "/usr/local/rustup",
  CARGO_HOME: "/usr/local/cargo",
  RUST_VERSION: "1.94.1",
  WASI_SDK_PATH: "/opt/wasi-sdk",
  CC_wasm32_wasip1: "/opt/wasi-sdk/bin/clang",
  CFLAGS_wasm32_wasip1: "--target=wasm32-wasip1",
  KRATE_MAKER_SIGNING: "off",
  KRATE_ANTHROPIC_MODEL: "claude-opus-5",
  KRATE_BUILD_BUDGET_USD: "3.50",
  KRATE_BIN: "/usr/local/bin/krate",
  KRATE_HUB: "https://hub.krate.tech",
  FLY_APP_NAME: "krate-builder",
  FLY_MACHINE_ID: "abc",
  KRATE_BUILDER_SECRET: "the-hub-secret",
  ANTHROPIC_API_KEY: "sk-ant-house",
  OPENAI_API_KEY: "sk-openai-house",
  FLY_API_TOKEN: "fly-token",
  SOME_DATABASE_PASSWORD: "pw",
};
const SECRET_VALUES = ["the-hub-secret", "sk-ant-house", "sk-openai-house", "fly-token", "pw"];

const quiet = { log() {}, error() {} };
function sandboxWith(overrides = {}, { file = passwd, uid = 0 } = {}) {
  const calls = [];
  const box = createSandbox({
    env: { ...SERVICE_ENV, ...overrides },
    passwd: file,
    getuid: () => uid,
    spawnImpl: (cmd, args, opts) => { calls.push({ cmd, args, opts }); return { pid: 1 }; },
    log: quiet,
  });
  return { box, calls };
}

function assertNoSecrets(env, label) {
  for (const value of SECRET_VALUES) {
    assert.ok(!Object.values(env).includes(value), `${label}: a service secret reached the child`);
  }
  assert.equal(env.KRATE_BUILDER_SECRET, undefined, `${label}: the hub secret`);
}

test("the passwd reader finds the build user and nothing else", () => {
  assert.deepEqual(readPasswd(passwd, "kratebuild"), { name: "kratebuild", uid: 10001, gid: 10001, home: "/work/build" });
  assert.equal(readPasswd(passwd, "nobody"), null);
  assert.equal(readPasswd(join(dir, "missing"), "kratebuild"), null);
});

test("as root with a build user: the child runs as that user, in its home, with no secret", () => {
  const { box, calls } = sandboxWith();
  assert.equal(box.isolated, true);
  assert.equal(box.refusal(), null);
  box.spawn("/usr/local/bin/krate", ["check-app", "/x"], { cwd: "/work/build/tmp/j", detached: true });
  assert.equal(calls.length, 1);
  const { opts } = calls[0];
  assert.equal(opts.uid, 10001, "uid");
  assert.equal(opts.gid, 10001, "gid");
  assert.equal(opts.cwd, "/work/build/tmp/j");
  assert.equal(opts.detached, true);
  assert.equal(opts.env.HOME, "/work/build");
  assert.equal(opts.env.TMPDIR, "/work/build/tmp");
  assert.equal(opts.env.USER, "kratebuild");
  // The toolchain and the engine's settings get through.
  for (const name of ["PATH", "RUSTUP_HOME", "CARGO_HOME", "WASI_SDK_PATH", "CC_wasm32_wasip1", "CFLAGS_wasm32_wasip1", "KRATE_MAKER_SIGNING", "KRATE_ANTHROPIC_MODEL", "KRATE_BUILD_BUDGET_USD"]) {
    assert.equal(opts.env[name], SERVICE_ENV[name], name);
  }
  // The service's own configuration and every secret do not.
  for (const name of ["KRATE_BIN", "KRATE_HUB", "FLY_APP_NAME", "FLY_MACHINE_ID", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "FLY_API_TOKEN", "SOME_DATABASE_PASSWORD"]) {
    assert.equal(opts.env[name], undefined, name);
  }
  assertNoSecrets(opts.env, "keyless");
});

test("an AI path gets exactly the key it was handed, and nothing else secret", () => {
  const { box, calls } = sandboxWith();
  box.spawn("krate", ["create"], { key: { name: "ANTHROPIC_API_KEY", value: "sk-ant-theirs" }, extra: { KRATE_STARTER_SHAPE: "notes" } });
  const { env } = calls[0].opts;
  assert.equal(env.ANTHROPIC_API_KEY, "sk-ant-theirs");
  assert.equal(env.KRATE_STARTER_SHAPE, "notes");
  assert.equal(env.OPENAI_API_KEY, undefined, "only the agent's own vendor");
  assert.equal(env.KRATE_BUILDER_SECRET, undefined);
  assert.equal(env.FLY_API_TOKEN, undefined);
  // A key that is not a model key is refused, not passed.
  assert.throws(() => box.spawn("krate", [], { key: { name: "KRATE_BUILDER_SECRET", value: "x" } }), /not a model key/);
  // An extra setting cannot smuggle one in either.
  assert.throws(() => box.spawn("krate", [], { extra: { KRATE_BUILDER_SECRET: "x" } }), /may not carry/);
  assert.throws(() => box.spawn("krate", [], { extra: { ANTHROPIC_API_KEY: "x" } }), /may not carry/);
  assert.equal(calls.length, 1, "nothing spawned for the refused calls");
});

test("a caller cannot hand the child an environment of its own", () => {
  const { box, calls } = sandboxWith();
  assert.throws(() => box.spawn("krate", [], { env: { ...SERVICE_ENV } }), /builds the environment itself/);
  assert.equal(calls.length, 0);
});

test("the operator's pass list adds names, never a credential", () => {
  const { box, calls } = sandboxWith({
    FAKE_HOOK: "on",
    KRATE_BUILD_ENV_PASS: "FAKE_HOOK, KRATE_BUILDER_SECRET,ANTHROPIC_API_KEY,FLY_API_TOKEN,SOME_DATABASE_PASSWORD",
  });
  box.spawn("krate", []);
  const { env } = calls[0].opts;
  assert.equal(env.FAKE_HOOK, "on");
  assertNoSecrets(env, "pass list");
});

test("production with no build user refuses every build and spawns nothing", () => {
  for (const [label, file, uid] of [
    ["no such user", noUser, 0],
    ["service is not root", passwd, 1000],
    ["service already runs as the build user", passwd, 10001],
  ]) {
    const { box, calls } = sandboxWith({}, { file, uid });
    assert.equal(box.isolated, false, label);
    assert.match(box.refusal() || "", /switched off/, label);
    assert.throws(() => box.spawn("krate", ["create"]), (e) => e.code === "EBUILDUSER" && e.status === 503, label);
    assert.equal(calls.length, 0, `${label}: nothing ran`);
  }
  // KRATE_REQUIRE_BUILD_USER makes any box behave as production.
  const { box } = sandboxWith({ FLY_APP_NAME: undefined, KRATE_REQUIRE_BUILD_USER: "1" }, { file: noUser });
  assert.ok(box.refusal());
});

test("a root build user is no boundary", () => {
  const rootAsBuild = join(dir, "passwd-root");
  return writeFile(rootAsBuild, "kratebuild:x:0:0::/root:/bin/sh\n").then(() => {
    const { box } = sandboxWith({}, { file: rootAsBuild, uid: 0 });
    assert.equal(box.isolated, false);
    assert.ok(box.refusal());
  });
});

test("on a laptop with no build user it still runs, without a uid switch, still allow-listed", () => {
  const { box, calls } = sandboxWith({ FLY_APP_NAME: undefined }, { file: noUser, uid: 501 });
  assert.equal(box.refusal(), null);
  box.spawn("krate", ["plan"]);
  const { opts } = calls[0];
  assert.equal(opts.uid, undefined);
  assert.equal(opts.gid, undefined);
  assert.equal(opts.env.HOME, "/work", "the service's HOME when nothing switches");
  assertNoSecrets(opts.env, "laptop");
});

test("a child that stays this service's user keeps its HOME and gets no key", () => {
  const { box } = sandboxWith();
  const env = box.childEnv({ as: "service" });
  assert.equal(env.HOME, "/work");
  assert.equal(env.TMPDIR, "/work/tmp");
  assertNoSecrets(env, "service child");
  assert.equal(env.ANTHROPIC_API_KEY, undefined);
});

test("scratch directories and handed-over files belong to the build user", async () => {
  // Root cannot be had in a test, so the build user is this very user:
  // the same chown calls run, to an id the test may give files to.
  const me = process.getuid();
  const mine = process.getgid();
  const home = join(dir, "home");
  await mkdir(home);
  const selfPasswd = join(dir, "passwd-self");
  await writeFile(selfPasswd, `kratebuild:x:${me}:${mine}::${home}:/bin/sh\n`);
  const box = createSandbox({ env: { PATH: process.env.PATH }, passwd: selfPasswd, getuid: () => me, log: quiet, spawnImpl: () => ({}) });
  assert.equal(box.switching, true);
  assert.equal(box.isolated, false, "the same uid is no boundary, and says so");
  const scratch = await box.scratch("krate-build-");
  assert.ok(scratch.startsWith(join(home, "tmp") + "/"), scratch);
  const st = await lstat(scratch);
  assert.equal(st.uid, me);
  assert.equal(st.mode & 0o777, 0o700, "a job's folder is its own");
  await mkdir(join(scratch, "a", "b"), { recursive: true });
  await writeFile(join(scratch, "a", "b", "f"), "x");
  await box.handOver(scratch);
  assert.equal((await lstat(join(scratch, "a", "b", "f"))).uid, me);
});

test("server.js starts no child of its own: every one goes through the sandbox", () => {
  // The structural half of the guard. A direct child_process import in the
  // server is a door around the uid switch and the allow-list.
  const src = readFileSync(new URL("../src/server.js", import.meta.url), "utf8");
  assert.doesNotMatch(src, /from\s+["']node:child_process["']/);
  assert.doesNotMatch(src, /require\(["'](node:)?child_process["']\)/);
  assert.doesNotMatch(src, /withoutServiceSecrets|\.\.\.process\.env\b/, "no child built from a copy of process.env");
});
