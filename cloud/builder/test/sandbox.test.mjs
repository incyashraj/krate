/* The one door every build-time child goes through (K-1293, K-1297).
 *
 * A build runs code nobody here wrote -- a dependency's build script, a
 * proc-macro, `include_str!("/proc/1/environ")` -- and it used to run as
 * the service's own user, able to read the hub secret and the Anthropic key
 * out of /proc. Then all builds ran as ONE build user, so two at once could
 * read each other's environment and folders, and share caches either could
 * poison. These pin the guards:
 *
 *   - a job leases a build user from a pool for its whole life; two running
 *     jobs never share a uid, a home, a TMPDIR or a CARGO_HOME;
 *   - each home is 0700 to its own uid, and is emptied before a job gets it
 *     and after it hands it back, once every process of that uid is dead;
 *   - a full pool says busy; a uid that will not let go is never reused;
 *   - the environment is an allow-list with a key only when handed over;
 *   - the toolchain and the image's cargo home must be root's and read-only
 *     to build users, or a production box refuses to build;
 *   - in production, no build at all without the build users.
 *
 * As root with uids that do not exist here, the file system is an in-memory
 * stand-in that records owners and modes; as this very user, the real one.
 *
 *   node --test cloud/builder/test/sandbox.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { lstat, mkdir, mkdtemp, readdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readFileSync } from "node:fs";
import { BUSY, createSandbox, pidsOfUid, readPasswd, sharedWritable } from "../src/sandbox.js";

const dir = await mkdtemp(join(tmpdir(), "krate-sandbox-test-"));
const passwd = join(dir, "passwd");
await writeFile(passwd, [
  "root:x:0:0:root:/root:/bin/bash",
  "kratebuild:x:10000:10000::/work/build/0:/usr/sbin/nologin",
  "kratebuild1:x:10001:10001::/work/build/1:/usr/sbin/nologin",
  "kratebuild2:x:10002:10002::/work/build/2:/usr/sbin/nologin",
  "kratebuild3:x:10003:10003::/work/build/3:/usr/sbin/nologin",
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
  KRATE_BUILD_SLOTS: "3",
};
const SECRET_VALUES = ["the-hub-secret", "sk-ant-house", "sk-openai-house", "fly-token", "pw"];
const SHARED = ["/usr/local/rustup", "/usr/local/rustup/toolchains", "/usr/local/cargo", "/usr/local/cargo/bin", "/usr/local/cargo/registry", "/usr/local/bin", "/opt/wasi-sdk"];

/* A file system that records owners and modes, for root's side of things. */
function memFs(overrides = {}) {
  const nodes = new Map();
  const ops = [];
  const parentOf = (p) => p.slice(0, p.lastIndexOf("/")) || "/";
  const missing = (p) => Object.assign(new Error(`ENOENT ${p}`), { code: "ENOENT" });
  const fs = {
    nodes,
    ops,
    failRm: null,
    async lstat(p) {
      const n = nodes.get(p);
      if (!n) throw missing(p);
      return { uid: n.uid, gid: n.gid, mode: n.mode, isDirectory: () => n.dir, isSymbolicLink: () => Boolean(n.link) };
    },
    async readdir(p) {
      if (!nodes.has(p)) throw missing(p);
      return [...nodes.keys()].filter((k) => k !== p && parentOf(k) === p).map((k) => k.slice(p.length + 1));
    },
    async rm(p) {
      ops.push(["rm", p]);
      if (fs.failRm && p.startsWith(fs.failRm)) throw new Error("EBUSY");
      for (const k of [...nodes.keys()]) if (k === p || k.startsWith(`${p}/`)) nodes.delete(k);
    },
    async mkdir(p, o = {}) {
      ops.push(["mkdir", p]);
      if (!nodes.has(p)) nodes.set(p, { uid: 0, gid: 0, mode: 0o40000 | (o.mode ?? 0o755), dir: true });
    },
    async chmod(p, m) {
      const n = nodes.get(p);
      if (!n) throw missing(p);
      n.mode = (n.mode & ~0o777) | m;
    },
    async lchown(p, uid, gid) {
      const n = nodes.get(p);
      if (!n) throw missing(p);
      n.uid = uid;
      n.gid = gid;
    },
    async symlink(target, p) {
      nodes.set(p, { uid: 0, gid: 0, mode: 0o120777, dir: false, link: target });
    },
    async mkdtemp(prefix) {
      const p = `${prefix}${Math.random().toString(36).slice(2, 8)}`;
      nodes.set(p, { uid: 0, gid: 0, mode: 0o40700, dir: true });
      return p;
    },
  };
  for (const p of SHARED) nodes.set(p, { uid: 0, gid: 0, mode: 0o40755, dir: true });
  for (const [p, n] of Object.entries(overrides)) nodes.set(p, { uid: 0, gid: 0, mode: 0o40755, dir: true, ...n });
  return fs;
}

const quiet = { log() {}, error() {} };
function fakeChild(pid) {
  const listeners = {};
  return {
    pid,
    once(event, fn) { (listeners[event] ||= []).push(fn); },
    emit(event) { for (const fn of listeners[event] || []) fn(); },
  };
}
function sandboxWith(overrides = {}, { file = passwd, uid = 0, fs = memFs(), pids = () => [], log = quiet, onKill = () => {} } = {}) {
  const calls = [];
  const kills = [];
  let nextPid = 700;
  const box = createSandbox({
    env: { ...SERVICE_ENV, ...overrides },
    passwd: file,
    getuid: () => uid,
    spawnImpl: (cmd, args, opts) => { calls.push({ cmd, args, opts }); return fakeChild(nextPid++); },
    fs,
    pidsOf: pids,
    killImpl: (pid, signal) => { kills.push([pid, signal]); onKill(pid); },
    log,
  });
  return { box, calls, kills, fs };
}

function assertNoSecrets(env, label) {
  for (const value of SECRET_VALUES) {
    assert.ok(!Object.values(env).includes(value), `${label}: a service secret reached the child`);
  }
  assert.equal(env.KRATE_BUILDER_SECRET, undefined, `${label}: the hub secret`);
}

test("the passwd reader finds the build users and nothing else", () => {
  assert.deepEqual(readPasswd(passwd, "kratebuild2"), { name: "kratebuild2", uid: 10002, gid: 10002, home: "/work/build/2" });
  assert.equal(readPasswd(passwd, "nobody"), null);
  assert.equal(readPasswd(join(dir, "missing"), "kratebuild"), null);
});

test("as root with a pool: two running jobs are two users, two homes, two environments", async () => {
  const { box, calls, fs } = sandboxWith();
  await box.ready;
  assert.equal(box.isolated, true);
  assert.equal(box.refusal(), null);
  assert.equal(box.size, 3);
  const a = await box.lease();
  const b = await box.lease();
  assert.notEqual(a.user.uid, b.user.uid, "two jobs at once never share a uid");
  assert.deepEqual([a.user.uid, b.user.uid], [10001, 10002]);
  assert.equal(box.free(), 1);

  a.spawn("/usr/local/bin/krate", ["create"], { cwd: "/work/build/1/tmp/j", detached: true, key: { name: "ANTHROPIC_API_KEY", value: "sk-ant-alices-own" } });
  b.spawn("/usr/local/bin/krate", ["create"], { detached: true, key: { name: "ANTHROPIC_API_KEY", value: "sk-ant-bobs-own" } });
  const [ea, eb] = calls.map((c) => c.opts);
  assert.deepEqual([ea.uid, ea.gid, eb.uid, eb.gid], [10001, 10001, 10002, 10002]);
  assert.equal(ea.cwd, "/work/build/1/tmp/j");
  assert.equal(ea.detached, true);
  // Everything a build writes is in its own home: HOME, TMPDIR, CARGO_HOME.
  assert.deepEqual([ea.env.HOME, ea.env.TMPDIR, ea.env.CARGO_HOME, ea.env.USER], ["/work/build/1", "/work/build/1/tmp", "/work/build/1/.cargo", "kratebuild1"]);
  assert.deepEqual([eb.env.HOME, eb.env.TMPDIR, eb.env.CARGO_HOME, eb.env.USER], ["/work/build/2", "/work/build/2/tmp", "/work/build/2/.cargo", "kratebuild2"]);
  // Each environment holds its own key and nothing of the other's.
  assert.equal(ea.env.ANTHROPIC_API_KEY, "sk-ant-alices-own");
  assert.equal(eb.env.ANTHROPIC_API_KEY, "sk-ant-bobs-own");
  assert.ok(!Object.values(eb.env).includes("sk-ant-alices-own"), "bob's build cannot see alice's key in its environment");
  assert.ok(!Object.values(ea.env).some((v) => String(v).includes("/work/build/2")), "nothing in a's environment points into b's home");
  // The toolchain and the engine's settings get through; nothing else.
  for (const name of ["PATH", "RUSTUP_HOME", "WASI_SDK_PATH", "CC_wasm32_wasip1", "CFLAGS_wasm32_wasip1", "KRATE_MAKER_SIGNING", "KRATE_ANTHROPIC_MODEL", "KRATE_BUILD_BUDGET_USD"]) {
    assert.equal(ea.env[name], SERVICE_ENV[name], name);
  }
  for (const name of ["KRATE_BIN", "KRATE_HUB", "FLY_APP_NAME", "FLY_MACHINE_ID", "OPENAI_API_KEY", "FLY_API_TOKEN", "SOME_DATABASE_PASSWORD"]) {
    assert.equal(ea.env[name], undefined, name);
  }
  assertNoSecrets(eb.env, "b");

  // Each home and each job folder is its own uid's, 0700: another build
  // user can neither list nor read it.
  for (const [lease, uid] of [[a, 10001], [b, 10002]]) {
    const home = fs.nodes.get(lease.home);
    assert.equal(home.uid, uid);
    assert.equal(home.mode & 0o777, 0o700, `${lease.home} is 0700`);
    for (const sub of ["tmp", ".cargo"]) assert.equal(fs.nodes.get(join(lease.home, sub)).uid, uid, sub);
    assert.equal(fs.nodes.get(join(lease.home, ".cargo", "bin")).link, "/usr/local/cargo/bin", "the image's cargo tools, by link");
    const job = await lease.scratch("krate-build-");
    assert.ok(job.startsWith(`${lease.home}/tmp/krate-build-`), job);
    assert.equal(fs.nodes.get(job).uid, uid);
    assert.equal(fs.nodes.get(job).mode & 0o777, 0o700);
  }
});

test("a full pool is busy; a job's user goes back killed and emptied, and the next job gets a clean home", async () => {
  // Two processes of uid 10001 that die when killed.
  let running = [4242, 4243];
  const pids = (uid) => (uid === 10001 ? running : []);
  const onKill = (pid) => { running = running.filter((p) => p !== pid); };
  const { box, kills, fs } = sandboxWith({ KRATE_BUILD_SLOTS: "2" }, { pids, onKill });
  const a = await box.lease();
  const b = await box.lease();
  assert.equal(await box.lease(), null, "every build user is leased: the door says busy");
  assert.equal(box.free(), 0);
  assert.match(BUSY, /busy/);

  // What a's job left behind: a child it started, a daemon a build script
  // forked off, a file in its home and a poisoned cache.
  const child = a.spawn("krate", ["create"], { detached: true });
  await fs.mkdir("/work/build/1/.cache");
  await fs.mkdir("/work/build/1/.cache/krate");
  await fs.mkdir("/work/build/1/.cache/krate/poisoned.rlib");
  await a.release();
  assert.ok(kills.some(([pid, sig]) => pid === -child.pid && sig === "SIGKILL"), "the job's own process group is killed");
  assert.ok(kills.some(([pid]) => pid === 4242) && kills.some(([pid]) => pid === 4243), "and every other process of its uid");
  assert.deepEqual(running, []);
  assert.ok(!fs.nodes.has("/work/build/1"), "its home is emptied");
  assert.equal(box.free(), 1);
  assert.strictEqual(a.release(), a.release(), "handing back twice is one hand-back");
  assert.throws(() => a.spawn("krate", []), /handed back/);

  // The next job, maybe someone else's, takes uid 10001 -- and none of it.
  const c = await box.lease();
  assert.equal(c.user.uid, 10001);
  assert.ok(!fs.nodes.has("/work/build/1/.cache/krate/poisoned.rlib"), "nothing of the last job is there");
  assert.deepEqual((await fs.readdir("/work/build/1")).sort(), [".cargo", "tmp"]);
  await b.release();
  await c.release();
  assert.equal(box.free(), 2);
});

test("a user that will not let go is retired, never handed to another job", async () => {
  const errors = [];
  const log = { log() {}, error: (m) => errors.push(m) };
  // A process of uid 10001 that does not die.
  const { box } = sandboxWith({ KRATE_BUILD_SLOTS: "2" }, { pids: (uid) => (uid === 10001 ? [99] : []), log });
  const a = await box.lease();
  await a.release();
  assert.equal(box.free(), 1, "10001 stays out of the pool");
  assert.match(errors.join("\n"), /kratebuild1 still runs processes/);
  const next = await box.lease();
  assert.equal(next.user.uid, 10002);
  assert.equal(await box.lease(), null);

  // A home that will not empty: the same.
  const fs = memFs();
  const { box: other } = sandboxWith({ KRATE_BUILD_SLOTS: "1" }, { fs, log });
  const l = await other.lease();
  fs.failRm = "/work/build/1";
  await l.release();
  assert.equal(other.free(), 0);
  assert.match(errors.join("\n"), /could not empty \/work\/build\/1/);
});

test("a build user that can write the toolchain or the image's cargo home: production refuses", async () => {
  for (const [label, bad] of [
    ["cargo home a+w, as the rust image ships it", { "/usr/local/cargo": { mode: 0o40777 } }],
    ["the registry owned by a build user", { "/usr/local/cargo/registry": { uid: 10001 } }],
    ["a toolchain folder group-writable", { "/usr/local/rustup/toolchains": { mode: 0o40775 } }],
  ]) {
    const { box, calls } = sandboxWith({}, { fs: memFs(bad) });
    await box.ready;
    assert.match(box.refusal() || "", /switched off/, label);
    await assert.rejects(box.lease(), (e) => e.code === "EBUILDUSER", label);
    assert.equal(calls.length, 0, label);
  }
  assert.deepEqual(await sharedWritable(SHARED, { fs: memFs() }), [], "root's and 0755 is fine");
  const links = memFs({ "/usr/local/cargo/bin/cargo": { link: "/usr/local/rustup/x", uid: 10001, mode: 0o120777, dir: false } });
  assert.deepEqual(await sharedWritable(SHARED, { fs: links }), [], "a link is judged by itself, never followed");
});

test("an AI path gets exactly the key it was handed, and nothing else secret", async () => {
  const { box, calls } = sandboxWith();
  const l = await box.lease();
  l.spawn("krate", ["create"], { key: { name: "ANTHROPIC_API_KEY", value: "sk-ant-theirs" }, extra: { KRATE_STARTER_SHAPE: "notes" } });
  const { env } = calls[0].opts;
  assert.equal(env.ANTHROPIC_API_KEY, "sk-ant-theirs");
  assert.equal(env.KRATE_STARTER_SHAPE, "notes");
  assert.equal(env.OPENAI_API_KEY, undefined, "only the agent's own vendor");
  assert.equal(env.KRATE_BUILDER_SECRET, undefined);
  assert.equal(env.FLY_API_TOKEN, undefined);
  assert.throws(() => l.spawn("krate", [], { key: { name: "KRATE_BUILDER_SECRET", value: "x" } }), /not a model key/);
  assert.throws(() => l.spawn("krate", [], { extra: { KRATE_BUILDER_SECRET: "x" } }), /may not carry/);
  assert.throws(() => l.spawn("krate", [], { extra: { ANTHROPIC_API_KEY: "x" } }), /may not carry/);
  assert.equal(calls.length, 1, "nothing spawned for the refused calls");
  // A keyless child of the same lease has no key at all.
  l.spawn("krate", ["run"]);
  assert.equal(calls[1].opts.env.ANTHROPIC_API_KEY, undefined);
});

test("a caller cannot hand the child an environment of its own", async () => {
  const { box, calls } = sandboxWith();
  const l = await box.lease();
  assert.throws(() => l.spawn("krate", [], { env: { ...SERVICE_ENV } }), /builds the environment itself/);
  assert.equal(calls.length, 0);
});

test("the operator's pass list adds names, never a credential", async () => {
  const { box, calls } = sandboxWith({
    FAKE_HOOK: "on",
    KRATE_BUILD_ENV_PASS: "FAKE_HOOK, KRATE_BUILDER_SECRET,ANTHROPIC_API_KEY,FLY_API_TOKEN,SOME_DATABASE_PASSWORD",
  });
  (await box.lease()).spawn("krate", []);
  const { env } = calls[0].opts;
  assert.equal(env.FAKE_HOOK, "on");
  assertNoSecrets(env, "pass list");
});

test("production without its build users refuses every build and spawns nothing", async () => {
  const twins = join(dir, "passwd-twins");
  await writeFile(twins, "kratebuild1:x:10001:10001::/work/build/1:/bin/sh\nkratebuild2:x:10001:10001::/work/build/2:/bin/sh\n");
  const shared = join(dir, "passwd-shared-home");
  await writeFile(shared, "kratebuild1:x:10001:10001::/work/build/1:/bin/sh\nkratebuild2:x:10002:10002::/work/build/1:/bin/sh\n");
  for (const [label, file, uid, extra] of [
    ["no such user", noUser, 0, {}],
    ["one of the pool missing", passwd, 0, { KRATE_BUILD_SLOTS: "4" }],
    ["two slots, one uid", twins, 0, { KRATE_BUILD_SLOTS: "2" }],
    ["two slots, one home", shared, 0, { KRATE_BUILD_SLOTS: "2" }],
    ["service is not root", passwd, 1000, {}],
    ["service already runs as a build user", passwd, 10001, { KRATE_BUILD_SLOTS: "1" }],
  ]) {
    const { box, calls } = sandboxWith(extra, { file, uid });
    await box.ready;
    assert.equal(box.isolated, false, label);
    assert.match(box.refusal() || "", /switched off/, label);
    await assert.rejects(box.lease(), (e) => e.code === "EBUILDUSER" && e.status === 503, label);
    await assert.rejects(box.spawnOnce("krate", ["judge"]), (e) => e.code === "EBUILDUSER", label);
    assert.equal(calls.length, 0, `${label}: nothing ran`);
  }
  // KRATE_REQUIRE_BUILD_USER makes any box behave as production.
  const { box } = sandboxWith({ FLY_APP_NAME: undefined, KRATE_REQUIRE_BUILD_USER: "1" }, { file: noUser });
  assert.ok(box.refusal());
});

test("a root build user is no boundary", async () => {
  const rootAsBuild = join(dir, "passwd-root");
  await writeFile(rootAsBuild, "kratebuild1:x:0:0::/work/build/1:/bin/sh\n");
  const { box } = sandboxWith({ KRATE_BUILD_SLOTS: "1" }, { file: rootAsBuild, uid: 0 });
  assert.equal(box.isolated, false);
  assert.ok(box.refusal());
});

test("on a laptop with no build user it still runs, without a uid switch or a pool, still allow-listed", async () => {
  const { box, calls, fs } = sandboxWith({ FLY_APP_NAME: undefined }, { file: noUser, uid: 501 });
  assert.equal(box.refusal(), null);
  const l = await box.lease();
  assert.ok(await box.lease(), "no pool, no limit");
  l.spawn("krate", ["plan"]);
  const { opts } = calls[0];
  assert.equal(opts.uid, undefined);
  assert.equal(opts.gid, undefined);
  assert.equal(opts.env.HOME, "/work", "the service's HOME when nothing switches");
  assert.equal(opts.env.CARGO_HOME, "/usr/local/cargo", "and its own cargo home");
  assertNoSecrets(opts.env, "laptop");
  await l.release();
  assert.ok(!fs.ops.some(([op]) => op === "rm"), "a laptop's HOME is never emptied");
});

test("a child that stays this service's user keeps its HOME and gets no key", () => {
  const { box } = sandboxWith();
  const env = box.childEnv({ as: "service" });
  assert.equal(env.HOME, "/work");
  assert.equal(env.TMPDIR, "/work/tmp");
  assertNoSecrets(env, "service child");
  assert.equal(env.ANTHROPIC_API_KEY, undefined);
});

test("on the real file system: homes and job folders are 0700, and a killed job's processes and files go with it", async () => {
  // Root cannot be had in a test, so every build user is this very user:
  // the same chown, chmod, rm and kill calls run, to an id the test may
  // give files to.
  const me = process.getuid();
  const mine = process.getgid();
  const homes = [join(dir, "real", "1"), join(dir, "real", "2")];
  await mkdir(join(dir, "real"));
  const selfPasswd = join(dir, "passwd-self");
  await writeFile(selfPasswd, homes.map((h, i) => `kratebuild${i + 1}:x:${me}:${mine}::${h}:/bin/sh\n`).join(""));
  const box = createSandbox({ env: { PATH: process.env.PATH, KRATE_BUILD_SLOTS: "2", CARGO_HOME: "/usr/local/cargo" }, passwd: selfPasswd, getuid: () => me, log: quiet });
  assert.equal(box.switching, true);
  assert.equal(box.isolated, false, "the same uid is no boundary, and says so");
  const a = await box.lease();
  const b = await box.lease();
  assert.equal(await box.lease(), null);
  for (const l of [a, b]) {
    const st = await lstat(l.home);
    assert.equal(st.mode & 0o777, 0o700, `${l.home} is its own`);
    const job = await l.scratch("krate-build-");
    assert.ok(job.startsWith(join(l.home, "tmp") + "/"), job);
    assert.equal((await lstat(job)).mode & 0o777, 0o700, "a job's folder is its own");
  }
  assert.notEqual(a.home, b.home);
  const stray = join(a.home, ".cache", "krate", "build");
  await mkdir(stray, { recursive: true });
  await writeFile(join(stray, "left.rlib"), "x");
  await a.handOver(a.home);

  // A job still running when it is stopped or times out: the hand-back
  // kills it -- its whole process group -- and empties the home.
  const sleeper = a.spawn("sh", ["-c", "sleep 30 & sleep 30"], { detached: true, stdio: "ignore" });
  await new Promise((r) => setTimeout(r, 100));
  assert.doesNotThrow(() => process.kill(sleeper.pid, 0), "running before the hand-back");
  await a.release();
  await new Promise((r) => setTimeout(r, 100));
  assert.throws(() => process.kill(-sleeper.pid, 0), "its process group is gone");
  await assert.rejects(lstat(a.home), "its home is gone, and the stray cache with it");
  assert.equal(box.free(), 1, "and its user is back in the pool");
  const c = await box.lease();
  assert.equal(c.home, a.home);
  assert.deepEqual((await readdir(c.home)).sort(), [".cargo", "tmp"], "the next job starts clean");
  await b.release();
  await c.release();
  assert.equal(box.free(), 2);
});

test("a one-run lease hands its user back when the child closes", async () => {
  const me = process.getuid();
  const home = join(dir, "once", "1");
  await mkdir(join(dir, "once"));
  const p = join(dir, "passwd-once");
  await writeFile(p, `kratebuild1:x:${me}:${process.getgid()}::${home}:/bin/sh\n`);
  const box = createSandbox({ env: { PATH: process.env.PATH, KRATE_BUILD_SLOTS: "1" }, passwd: p, getuid: () => me, log: quiet });
  const child = await box.spawnOnce("sh", ["-c", "echo $HOME"], { stdio: ["ignore", "pipe", "ignore"] });
  let out = "";
  child.stdout.on("data", (b) => { out += b; });
  assert.equal(box.free(), 0);
  await assert.rejects(box.spawnOnce("true", []), (e) => e.code === "EBUSY" && e.status === 503 && e.message === BUSY);
  await child.handedBack;
  assert.equal(out.trim(), home);
  assert.equal(box.free(), 1);
});

test("the processes of a uid are read from /proc by real and effective uid", async () => {
  const proc = join(dir, "proc");
  for (const [pid, uids] of [["123", "10001\t10001\t10001\t10001"], ["124", "0\t0\t0\t0"], ["125", "0\t10001\t10001\t10001"], ["self", "10001\t10001\t10001\t10001"]]) {
    await mkdir(join(proc, pid), { recursive: true });
    await writeFile(join(proc, pid, "status"), `Name:\tx\nUid:\t${uids}\nGid:\t0\t0\t0\t0\n`);
  }
  assert.deepEqual(pidsOfUid(10001, proc).sort(), [123, 125]);
  assert.deepEqual(pidsOfUid(10001, join(dir, "no-proc")), []);
});

test("server.js starts no child of its own: every one goes through a lease", () => {
  // The structural half of the guard. A direct child_process import in the
  // server is a door around the uid switch and the allow-list.
  const src = readFileSync(new URL("../src/server.js", import.meta.url), "utf8");
  assert.doesNotMatch(src, /from\s+["']node:child_process["']/);
  assert.doesNotMatch(src, /require\(["'](node:)?child_process["']\)/);
  assert.doesNotMatch(src, /withoutServiceSecrets|\.\.\.process\.env\b/, "no child built from a copy of process.env");
});
