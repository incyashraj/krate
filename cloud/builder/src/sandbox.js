/* Who runs a stranger's code on this box (K-1293).
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
 * child runs as `kratebuild`, a user with its own home and nothing else.
 * Linux lets one uid read another's /proc/<pid>/environ only with ptrace
 * rights, and a process that switched from root to another uid has none,
 * so root's environment -- and the volume's /work/state, which is root's
 * and 0700 -- is out of reach of anything a build runs.
 *
 * The environment a child gets is the second line, not the boundary: an
 * explicit allow-list of names (toolchain, engine settings), with HOME and
 * TMPDIR pointed at the build user's own, and a model key only when the
 * caller hands one over for a path that talks to a model. Never the hub
 * secret, whatever the operator configures.
 *
 * Every build-time child in this service goes through `spawn` here. On a
 * machine with no build user (a laptop, the tests) it still runs, without
 * the uid switch; in production (FLY_APP_NAME set) it refuses instead, so a
 * box that lost its build user stops building rather than building as root.
 */
import { spawn as nodeSpawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { chown, lchown, lstat, mkdir, mkdtemp, readdir } from "node:fs/promises";
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
 * HOME, TMPDIR, USER and LOGNAME are not here: they are set per child. */
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

export function createSandbox({
  env = process.env,
  passwd = env.KRATE_BUILD_PASSWD || "/etc/passwd",
  user = env.KRATE_BUILD_USER || "kratebuild",
  getuid = () => (typeof process.getuid === "function" ? process.getuid() : -1),
  spawnImpl = nodeSpawn,
  log = console,
} = {}) {
  const account = readPasswd(passwd, user);
  const self = getuid();
  // The boundary holds only when root hands the child to another, non-root
  // user. Already running AS the build user (a test) switches nothing.
  const isolated = Boolean(account && account.uid !== 0 && self === 0);
  const switching = Boolean(account && (isolated || self === account.uid));
  const production = Boolean(env.FLY_APP_NAME) || env.KRATE_REQUIRE_BUILD_USER === "1";

  const home = switching ? account.home : env.HOME || "";
  const tmp = switching ? join(account.home, "tmp") : env.TMPDIR || tmpdir();

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

  const why = !account
    ? `there is no "${user}" user`
    : account.uid === 0
    ? `"${user}" is root`
    : !switching
    ? `this service runs as uid ${self} and cannot switch to "${user}"`
    : !isolated
    ? `this service already runs as "${user}", so builds share its uid`
    : null;
  if (isolated) {
    log.log(`[sandbox] builds run as ${user} (uid ${account.uid}), home ${account.home}`);
  } else if (production) {
    log.error(`[sandbox] ${why}: refusing every build until it exists (K-1293)`);
  } else {
    log.error(`[sandbox] ${why}: builds run without a separate user (fine on a laptop, never on the builder)`);
  }

  /** Why builds are refused here, or null when they may run. */
  function refusal() {
    if (isolated || !production) return null;
    return "Building is switched off on this machine while it is being fixed. Krate Studio is free and makes apps on your own machine.";
  }

  function allowed(name) {
    if (SERVICE_SECRETS.includes(name) || MODEL_KEYS.includes(name)) return false;
    return ALLOWED.has(name) || ALLOWED_PATTERN.test(name) || passed.has(name);
  }

  /** The environment for a child. `as: "build"` (the default) is a child
   * that runs as the build user; `as: "service"` one that stays this
   * service's user and so keeps its HOME. `key` is { name, value } for the
   * model key of an AI path; `extra` is per-run engine settings. */
  function childEnv({ as = "build", key = null, extra = {} } = {}) {
    const out = {};
    for (const [name, value] of Object.entries(env)) {
      if (value !== undefined && allowed(name)) out[name] = value;
    }
    if (as === "build") {
      out.HOME = home;
      out.TMPDIR = tmp;
      if (switching) {
        out.USER = account.name;
        out.LOGNAME = account.name;
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

  /** Start a build-time child: as the build user, with the allow-listed
   * environment. The caller says which key it needs; it cannot hand over
   * an environment of its own. */
  function spawn(command, args, options = {}) {
    const { key = null, extra = {}, env: given, ...rest } = options;
    if (given !== undefined) throw new Error("sandbox.spawn builds the environment itself; pass key/extra, not env");
    const no = refusal();
    if (no) throw Object.assign(new Error(no), { code: "EBUILDUSER", status: 503 });
    const opts = { ...rest, env: childEnv({ as: "build", key, extra }) };
    if (switching) {
      opts.uid = account.uid;
      opts.gid = account.gid;
    }
    return spawnImpl(command, args, opts);
  }

  async function own(path) {
    if (switching) await chown(path, account.uid, account.gid);
  }

  /** A fresh directory for one build, owned by the build user. */
  async function scratch(prefix) {
    await mkdir(tmp, { recursive: true, mode: 0o700 });
    await own(tmp);
    const dir = await mkdtemp(join(tmp, prefix));
    await own(dir);
    return dir;
  }

  /** Give what this service wrote under `path` to the build user, so the
   * build can read and change it. Links are re-owned, never followed. */
  async function handOver(path) {
    if (!switching) return;
    const st = await lstat(path).catch(() => null);
    if (!st) return;
    await lchown(path, account.uid, account.gid).catch(() => {});
    if (st.isSymbolicLink() || !st.isDirectory()) return;
    for (const name of await readdir(path)) await handOver(join(path, name));
  }

  return {
    isolated,
    switching,
    production,
    user: account ? { ...account } : null,
    home,
    tmp,
    refusal,
    childEnv,
    spawn,
    scratch,
    handOver,
  };
}
