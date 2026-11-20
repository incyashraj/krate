/* Who runs a stranger's code on this box (K-1293, K-1297).
 *
 * Everything a build runs -- the engine, the AI agent loop inside it,
 * cargo, rustc, every dependency's build script and proc-macro, the app
 * itself when it is photographed -- is code this service did not write,
 * shaped by a stranger's words or a stranger's upload. It used to run as
 * the same Linux user as this service, and so it could read the service's
 * own environment through /proc/<pid>/environ: the hub secret
 * (KRATE_BUILDER_SECRET) and the Anthropic key. `include_str!("/proc/1/environ")`
 * was enough, and the bytes came back in the bundle or in a compiler error.
 *
 * The boundary is the uid. The service runs as root; every build-time
 * child runs as a build user with nothing of the service's. Linux lets one
 * uid read another's /proc/<pid>/environ only with ptrace rights, and a
 * process that switched from root to another uid has none, so root's
 * environment -- and the volume's /work/state, which is root's and 0700 --
 * is out of reach of anything a build runs.
 *
 * One build user for every build was not enough (K-1297): two builds at
 * the same moment were the same uid, so one could read the other's
 * /proc/<pid>/environ -- another person's own API key, or ours -- and its
 * job folder, and the caches they shared (the engine's compile cache in the
 * build user's home, a world-writable CARGO_HOME) let one build plant what
 * the next one linked. So the build users are a POOL, `kratebuild1..N`
 * (KRATE_BUILD_SLOTS), and a job LEASES one for its whole life:
 *
 *   - no two running jobs share a uid, so neither can read the other's
 *     environment or folders (each home is 0700 to its own uid);
 *   - the uid's home is emptied before a job gets it and again when the
 *     job hands it back, after every process of that uid is killed -- so
 *     nothing one job writes (registry, compile cache, SDK, scratch) is
 *     there when the next job, maybe another person's, takes the same uid;
 *   - everything a build writes lives in that home: HOME, TMPDIR, its own
 *     CARGO_HOME (the registry it fetches), the engine's SDK and its
 *     compile cache. What the image shares -- the toolchain in RUSTUP_HOME
 *     and the image's CARGO_HOME, the engine, wasi-sdk -- is root's and
 *     read-only to every build user (the Dockerfile; `sharedWritable` below
 *     refuses to build on a box where that does not hold).
 *
 * A pool of one is still safe, just one build at a time: a lease is
 * exclusive. When every uid is leased the door says the box is busy.
 *
 * The environment a child gets is the second line, not the boundary: an
 * explicit allow-list of names (toolchain, engine settings), with HOME,
 * TMPDIR and CARGO_HOME pointed into the lease's home, and a model key
 * only when the caller hands one over for a path that talks to a model.
 * Never the hub secret, whatever the operator configures.
 *
 * Every build-time child in this service goes through a lease here. On a
 * machine with no build user (a laptop, the tests) it still runs, without
 * the uid switch and without a pool; in production (FLY_APP_NAME set) it
 * refuses instead, so a box that lost its build users stops building
 * rather than building as root.
 */
import { spawn as nodeSpawn } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import * as realFs from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

/* What only the service may hold. Not passed to a child under any name,
 * not even when an operator lists it in KRATE_BUILD_ENV_PASS. */
export const SERVICE_SECRETS = ["KRATE_BUILDER_SECRET"];

/* The model keys. A child gets one only when its caller passes it as `key`
 * -- the AI paths -- and never by inheritance. */
export const MODEL_KEYS = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"];

/* Names a build may see, copied from the service's environment when set.
 * The toolchain the image bakes in, the engine's own settings, the locale.
 * HOME, TMPDIR, USER, LOGNAME and (when switching) CARGO_HOME are not
 * copied: they are set per lease. */
const ALLOWED = new Set([
  // Rust and the WASI C compiler (Dockerfile).
  "PATH", "RUSTUP_HOME", "CARGO_HOME", "RUST_VERSION", "RUSTUP_TOOLCHAIN",
  "WASI_SDK_PATH", "LIBCLANG_PATH",
  // The engine's settings the operator sets in fly.toml.
  "KRATE_MAKER_SIGNING", "KRATE_ANTHROPIC_MODEL", "KRATE_OPENAI_MODEL", "KRATE_AGENT_MODEL",
  "KRATE_BUILD_BUDGET_USD", "KRATE_AUTHOR_TIMEOUT_SECS", "KRATE_PROBE_TIMEOUT_SECS",
  "KRATE_SDK_ROOT", "KRATE_TOOLCHAINS_DIR", "KRATE_CACHE_DIR", "KRATE_NO_COMPILE_CACHE",
  "KRATE_NO_USAGE", "KRATE_NO_ACCOUNT_SYNC", "KRATE_HUB_URL", "KRATE_PULLEY",
  "KRATE_LOG", "KRATE_TRACE", "KRATE_DEBUG_AGENT", "DO_NOT_TRACK", "NO_COLOR",
  "LANG", "LANGUAGE", "TZ",
]);
// cc-rs reads the C compiler per target: CC_wasm32_wasip1 and friends.
const ALLOWED_PATTERN = /^((CC|CXX|AR|CFLAGS|CXXFLAGS)_wasm32_[a-z0-9_]+|LC_[A-Z]+)$/;

/* A name an operator may not add to the pass list, because it reads like a
 * credential. The pass list is for test hooks and new engine settings. */
const LOOKS_SECRET = /SECRET|TOKEN|PASSWORD|PASSWD|CREDENTIAL|PRIVATE|(^|_)KEY(S)?($|_)/i;

/* What the box says when every build user is leased. */
export const BUSY = "The build machine is busy with other apps. Try again in a minute.";

/** The passwd entry for `name`, or null. Read from the file, not by
 * spawning `id`: this runs before anything may spawn. */
export function readPasswd(path, name) {
  let text = "";
  try { text = readFileSync(path, "utf8"); } catch (e) { return null; }
  for (const line of text.split("\n")) {
    const f = line.split(":");
    if (f.length < 7 || f[0] !== name) continue;
    const uid = Number(f[2]);
    const gid = Number(f[3]);
    if (!Number.isInteger(uid) || !Number.isInteger(gid)) return null;
    return { name, uid, gid, home: f[5] };
  }
  return null;
}

/** The pids whose real or effective uid is `uid`, from /proc. Empty where
 * there is no /proc (a laptop): there the lease kills only what it started. */
export function pidsOfUid(uid, procRoot = "/proc") {
  const out = [];
  let names = [];
  try { names = readdirSync(procRoot); } catch (e) { return out; }
  for (const name of names) {
    if (!/^[0-9]+$/.test(name)) continue;
    let status = "";
    try { status = readFileSync(join(procRoot, name, "status"), "utf8"); } catch (e) { continue; }
    const m = /^Uid:\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)/m.exec(status);
    if (m && m.slice(1).some((id) => Number(id) === uid)) out.push(Number(name));
  }
  return out;
}

/* What the image shares with every build and no build may change: the
 * toolchain, the image's own cargo home, the engine, the C compiler. */
export function sharedPaths(env) {
  return [env.RUSTUP_HOME, env.CARGO_HOME, "/usr/local/bin", env.WASI_SDK_PATH].filter(Boolean);
}

/** Every entry under `roots` (to `depth` levels) a build user could write:
 * not root's, or group- or other-writable. Links are judged by themselves,
 * never followed. Answers a list of problems; empty is good. */
export async function sharedWritable(roots, { fs = realFs, depth = 2 } = {}) {
  const problems = [];
  async function walk(path, left) {
    let st;
    try { st = await fs.lstat(path); } catch (e) { return; }
    if (st.isSymbolicLink()) return;
    if (st.uid !== 0) problems.push(`${path} belongs to uid ${st.uid}, not root`);
    else if (st.mode & 0o022) problems.push(`${path} is writable by others (mode ${(st.mode & 0o777).toString(8)})`);
    if (!st.isDirectory() || left <= 0 || problems.length > 20) return;
    let names = [];
    try { names = await fs.readdir(path); } catch (e) { return; }
    for (const name of names) await walk(join(path, name), left - 1);
  }
  for (const root of roots) await walk(root, depth);
  return problems;
}

export function createSandbox({
  env = process.env,
  passwd = env.KRATE_BUILD_PASSWD || "/etc/passwd",
  user = env.KRATE_BUILD_USER || "kratebuild",
  // How many build users, `kratebuild1..N`. Unset: the one user named
  // `user` (a pool of one: one build at a time).
  slots = Number(env.KRATE_BUILD_SLOTS || 0),
  getuid = () => (typeof process.getuid === "function" ? process.getuid() : -1),
  spawnImpl = nodeSpawn,
  fs = realFs,
  pidsOf = (uid) => pidsOfUid(uid),
  killImpl = (pid, signal) => process.kill(pid, signal),
  log = console,
} = {}) {
  const names = slots > 0 ? Array.from({ length: slots }, (_, i) => `${user}${i + 1}`) : [user];
  const accounts = names.map((name) => readPasswd(passwd, name));
  const self = getuid();
  const allThere = accounts.every(Boolean);
  const uids = new Set(accounts.filter(Boolean).map((a) => a.uid));
  const homes = new Set(accounts.filter(Boolean).map((a) => a.home));
  // A home the lease may empty: absolute, at least two levels deep, and
  // nobody else's (not the service's own, not another slot's).
  const homeOk = (a) => /^\/[^/]+\/[^/]/.test(a.home) && !a.home.includes("..") && a.home !== env.HOME;
  const distinct = allThere && homes.size === accounts.length && accounts.every(homeOk);
  // The boundary holds only when root hands each child to another, non-root
  // user, and no two slots are the same uid. Already running AS the build
  // user (a test) switches nothing -- but still leases, wipes and seeds, so
  // the tests walk the same path production does.
  const isolated = Boolean(allThere && distinct && self === 0 && accounts.every((a) => a.uid !== 0) && uids.size === accounts.length);
  const switching = Boolean(allThere && distinct && (isolated || accounts.every((a) => a.uid === self)));
  const production = Boolean(env.FLY_APP_NAME) || env.KRATE_REQUIRE_BUILD_USER === "1";

  // Operator additions, minus anything that reads like a credential.
  const passed = new Set();
  for (const raw of String(env.KRATE_BUILD_ENV_PASS || "").split(",")) {
    const name = raw.trim();
    if (!name) continue;
    if (SERVICE_SECRETS.includes(name) || MODEL_KEYS.includes(name) || LOOKS_SECRET.test(name)) {
      log.error(`[sandbox] KRATE_BUILD_ENV_PASS names ${name}; a build never gets that, it is left out`);
      continue;
    }
    passed.add(name);
  }

  const missing = names.filter((_, i) => !accounts[i]);
  const why = missing.length
    ? `there is no "${missing[0]}" user${missing.length > 1 ? ` (nor ${missing.length - 1} more)` : ""}`
    : !distinct
    ? "two build users share a home, or a home is not one a build may own alone"
    : accounts.some((a) => a.uid === 0)
    ? `"${accounts.find((a) => a.uid === 0).name}" is root`
    : uids.size !== accounts.length
    ? "two build users share a uid"
    : !switching
    ? `this service runs as uid ${self} and cannot switch to the build users`
    : !isolated
    ? "this service already runs as the build user, so builds share its uid"
    : null;
  if (isolated) {
    log.log(`[sandbox] builds run as ${names.length} build user${names.length === 1 ? "" : "s"} (${names[0]}${names.length > 1 ? `..${names[names.length - 1]}` : ""}), one per running job`);
  } else if (production) {
    log.error(`[sandbox] ${why}: refusing every build until it exists (K-1293)`);
  } else {
    log.error(`[sandbox] ${why}: builds run without a separate user (fine on a laptop, never on the builder)`);
  }

  // What the image shares, checked once on an isolated box: a build user
  // that can write the toolchain or the image's cargo home can plant what
  // every later build runs (K-1297). Refused like a missing build user.
  let shared = null; // null until checked; [] when clean
  async function checkShared() {
    if (!isolated) return [];
    if (!shared) shared = await sharedWritable(sharedPaths(env), { fs });
    if (shared.length) log.error(`[sandbox] shared build paths are writable by build users: ${shared.slice(0, 3).join("; ")} (K-1297)`);
    return shared;
  }
  const sharedReady = checkShared().catch((e) => { shared = [`could not check: ${e.message}`]; return shared; });

  /** Why builds are refused here, or null when they may run. */
  function refusal() {
    if (production && (!isolated || (shared && shared.length))) {
      return "Building is switched off on this machine while it is being fixed. Krate Studio is free and makes apps on your own machine.";
    }
    return null;
  }

  function allowed(name) {
    if (SERVICE_SECRETS.includes(name) || MODEL_KEYS.includes(name)) return false;
    return ALLOWED.has(name) || ALLOWED_PATTERN.test(name) || passed.has(name);
  }

  /** The environment for a child. `as: "build"` takes `place` -- the
   * lease's home -- and points HOME, TMPDIR and (when switching) CARGO_HOME
   * into it; `as: "service"` stays this service's user and keeps its HOME.
   * `key` is { name, value } for the model key of an AI path; `extra` is
   * per-run engine settings. */
  function childEnv({ as = "build", place = null, key = null, extra = {} } = {}) {
    const out = {};
    for (const [name, value] of Object.entries(env)) {
      if (value !== undefined && allowed(name)) out[name] = value;
    }
    if (as === "build") {
      out.HOME = place ? place.home : env.HOME || "";
      out.TMPDIR = place ? place.tmp : env.TMPDIR || tmpdir();
      if (place && place.account) {
        out.USER = place.account.name;
        out.LOGNAME = place.account.name;
        // The registry a build fetches is its own (K-1297).
        out.CARGO_HOME = join(place.home, ".cargo");
      } else if (env.USER) {
        out.USER = env.USER;
      }
    } else {
      if (env.HOME) out.HOME = env.HOME;
      if (env.TMPDIR) out.TMPDIR = env.TMPDIR;
      if (env.USER) out.USER = env.USER;
    }
    for (const [name, value] of Object.entries(extra || {})) {
      if (SERVICE_SECRETS.includes(name) || MODEL_KEYS.includes(name) || LOOKS_SECRET.test(name)) {
        throw new Error(`a build's extra settings may not carry ${name}`);
      }
      out[name] = String(value);
    }
    if (key) {
      if (!MODEL_KEYS.includes(key.name)) throw new Error(`${key.name} is not a model key`);
      if (key.value) out[key.name] = key.value;
    }
    return out;
  }

  /* ---- the pool ------------------------------------------------------- */
  const leased = new Set(); // slot indexes in use (or retired: never freed)
  let open = 0; // leases out when nothing switches (no pool, no limit)

  async function chownTree(path, account) {
    const st = await fs.lstat(path).catch(() => null);
    if (!st) return;
    await fs.lchown(path, account.uid, account.gid).catch(() => {});
    if (st.isSymbolicLink() || !st.isDirectory()) return;
    for (const name of await fs.readdir(path)) await chownTree(join(path, name), account);
  }

  /* A clean home for a lease: whatever was there is gone, then the bare
   * layout -- tmp/ for job folders, .cargo/ for the registry it fetches
   * (with the image's cargo tools at bin/, read-only) -- all the slot's own
   * and 0700. The image's caches are never copied in: a build lays down
   * its SDK from the engine binary and fetches its crates itself. */
  async function prepare(account) {
    await fs.rm(account.home, { recursive: true, force: true });
    await fs.mkdir(account.home, { recursive: true, mode: 0o700 });
    await fs.mkdir(join(account.home, "tmp"), { mode: 0o700 });
    await fs.mkdir(join(account.home, ".cargo"), { mode: 0o700 });
    const tools = env.CARGO_HOME && join(env.CARGO_HOME, "bin");
    if (tools) await fs.symlink(tools, join(account.home, ".cargo", "bin")).catch(() => {});
    await chownTree(account.home, account);
    await fs.chmod(account.home, 0o700);
  }

  /** Take a build user for one job. Answers a lease, or null when every
   * build user is leased (say BUSY). Throws EBUILDUSER where builds are
   * refused. The lease is the job's until `release()`. */
  async function lease() {
    await sharedReady;
    const no = refusal();
    if (no) throw Object.assign(new Error(no), { code: "EBUILDUSER", status: 503 });
    if (!switching) {
      open += 1;
      return makeLease(null, null);
    }
    // Claimed before the first await, so two requests in one turn cannot
    // take the same slot.
    const slot = accounts.findIndex((_, i) => !leased.has(i));
    if (slot < 0) return null;
    leased.add(slot);
    try {
      await prepare(accounts[slot]);
    } catch (err) {
      leased.delete(slot);
      throw err;
    }
    return makeLease(slot, accounts[slot]);
  }

  function makeLease(slot, account) {
    const place = account
      ? { account, home: account.home, tmp: join(account.home, "tmp") }
      : { account: null, home: env.HOME || "", tmp: env.TMPDIR || tmpdir() };
    const children = new Set();
    let released = null;

    function spawn(command, args, options = {}) {
      const { key = null, extra = {}, env: given, ...rest } = options;
      if (given !== undefined) throw new Error("a lease builds the environment itself; pass key/extra, not env");
      if (released) throw new Error("this build user was handed back; take a new lease");
      const opts = { ...rest, env: childEnv({ as: "build", place, key, extra }) };
      if (account) {
        opts.uid = account.uid;
        opts.gid = account.gid;
      }
      const child = spawnImpl(command, args, opts);
      if (child && typeof child.once === "function") {
        children.add(child);
        child.once("close", () => children.delete(child));
      }
      return child;
    }

    /** A fresh directory for this job, the lease's own, 0700. */
    async function scratch(prefix) {
      await fs.mkdir(place.tmp, { recursive: true, mode: 0o700 });
      const dir = await fs.mkdtemp(join(place.tmp, prefix));
      await fs.chmod(dir, 0o700);
      if (account) await fs.lchown(dir, account.uid, account.gid);
      return dir;
    }

    /** Give what this service wrote under `path` to the lease's user, so
     * the build can read and change it. Links are re-owned, never followed. */
    async function handOver(path) {
      if (account) await chownTree(path, account);
    }

    /** Hand the build user back: every process it runs is killed, its home
     * emptied, and only then is the uid free for another job. Safe to call
     * twice; answers the same promise. A uid whose home will not empty is
     * retired, never handed to another job. */
    function release() {
      if (released) return released;
      released = (async () => {
        // What it started: the whole process group (a detached child leads
        // its own), then the child itself.
        for (const child of children) {
          if (!(child.pid > 0)) continue;
          try { killImpl(-child.pid, "SIGKILL"); } catch (e) {}
          try { killImpl(child.pid, "SIGKILL"); } catch (e) {}
        }
        if (!account) {
          open = Math.max(0, open - 1);
          return;
        }
        // Everything the uid still runs, however it got there: a build
        // script's daemon, a grandchild that left the process group. Only
        // where the uid is not this service's own.
        if (isolated) {
          for (let round = 0; round < 50; round += 1) {
            const left = pidsOf(account.uid);
            if (!left.length) break;
            for (const pid of left) { try { killImpl(pid, "SIGKILL"); } catch (e) {} }
            await new Promise((r) => setTimeout(r, 20));
          }
          if (pidsOf(account.uid).length) {
            log.error(`[sandbox] ${account.name} still runs processes after its job; retired until restart (K-1297)`);
            return;
          }
        }
        try {
          await fs.rm(account.home, { recursive: true, force: true });
        } catch (err) {
          log.error(`[sandbox] could not empty ${account.home}: ${err.message}; ${account.name} retired until restart (K-1297)`);
          return;
        }
        leased.delete(slot);
      })();
      return released;
    }

    return {
      slot,
      user: account ? { ...account } : null,
      home: place.home,
      tmp: place.tmp,
      env: (opts = {}) => childEnv({ ...opts, as: "build", place }),
      spawn,
      scratch,
      handOver,
      release,
    };
  }

  /** One child on its own lease, handed back when it closes: for the short
   * engine runs that are not a job (the judge, the guide). Throws BUSY's
   * words with status 503 when every build user is leased. */
  async function spawnOnce(command, args, options = {}) {
    const l = await lease();
    if (!l) throw Object.assign(new Error(BUSY), { code: "EBUSY", status: 503 });
    let child;
    try {
      child = l.spawn(command, args, options);
    } catch (err) {
      await l.release();
      throw err;
    }
    // Settled once the build user is back in the pool, so a caller that
    // waits for it never races the next lease for the same uid.
    child.handedBack = new Promise((resolve) => {
      child.once("error", () => l.release().then(resolve, resolve));
      child.once("close", () => l.release().then(resolve, resolve));
    });
    return child;
  }

  return {
    isolated,
    switching,
    production,
    users: accounts.filter(Boolean).map((a) => ({ ...a })),
    size: switching ? accounts.length : Infinity,
    free: () => (switching ? accounts.length - leased.size : Infinity),
    ready: sharedReady,
    refusal,
    childEnv: (opts = {}) => childEnv(opts),
    lease,
    spawnOnce,
  };
}
