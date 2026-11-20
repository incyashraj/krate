/* Compiling an app whose source somebody else wrote (IC-989 phase 2).
 *
 * "Claude writes, Krate builds": a chat model on claude.ai writes the app's
 * source, the hub's connector sends it here, and this box builds it with the
 * same engine checks every made app passes -- `krate check-app`, then
 * `krate pack` -- and NO model of ours. Nothing on this path runs an agent,
 * opens a funded case or spends an allowance; it costs compute only, and
 * the server counts that per account.
 *
 * This file holds the parts that need no server: what an upload may
 * contain, what its Cargo.toml may ask cargo to do, where the SDK is, and
 * the words the connector hands the model before it writes. server.js owns
 * the door, the job and the engine runs.
 */

import { spawn } from "node:child_process";
import { mkdtemp, readdir, rm, stat, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

/* What one upload may be. The same bounds as an uploaded project (/port):
 * an app's source is a few files, and a request near these is not one. */
export const MAX_COMPILE_FILES = 600;
export const MAX_COMPILE_FILE_BYTES = 2 * 1024 * 1024;
export const MAX_COMPILE_BYTES = 16 * 1024 * 1024;

/* One path segment: starts with a letter or digit, so no dotfile
 * (`.cargo/config.toml` can name a rustc wrapper -- a command of the
 * uploader's choosing run on this box) and no `..`. */
const SEGMENT = /^[A-Za-z0-9][A-Za-z0-9._-]{0,120}$/;

/* Where a file may sit. An allow-list, not a deny-list: everything a Krate
 * app is made of fits here, and anything else -- build.rs, .cargo/,
 * rust-toolchain.toml, target/ -- is a way to change what the build does
 * rather than what the app is. */
const TOP_FILES = new Set(["Cargo.toml", "Cargo.lock", "manifest.toml", "krate-check.toml"]);
const TOP_DIRS = new Set(["src", "assets"]);

/* The words for the files people most often send by mistake, so the answer
 * says what to do rather than only "no". */
const NAMED_REFUSALS = {
  "build.rs": "build.rs is not accepted: a build script runs on the build machine. Do the work in the app itself.",
  target: "target/ is build output. Send the source only.",
  ".cargo": ".cargo/ is not accepted: it changes how the build runs. Send Cargo.toml, manifest.toml, src/ and assets/.",
  "rust-toolchain.toml": "rust-toolchain.toml is not accepted: Krate builds with its own toolchain.",
  "rust-toolchain": "rust-toolchain is not accepted: Krate builds with its own toolchain.",
};

function refusal(status, error) {
  return { ok: false, status, error };
}

/* The problem with one path, or null. Refused, never cleaned: a cleaned
 * path is a different file from the one the writer meant. */
export function compilePathProblem(path) {
  if (typeof path !== "string" || !path || path.length > 400) return "a file has no usable path";
  if (path.startsWith("/") || /^[A-Za-z]:/.test(path) || path.includes("\\")) return `${path} is not a relative path`;
  const parts = path.split("/");
  for (const part of parts) {
    if (part === "." || part === ".." || part === "") return `${path} steps outside the app`;
  }
  const named = NAMED_REFUSALS[parts[0]];
  if (named) return named;
  for (const part of parts) {
    if (!SEGMENT.test(part)) return `${path} has a name the build cannot use (letters, digits, '.', '_' and '-' only, not starting with '.')`;
  }
  if (parts.length === 1) {
    if (!TOP_FILES.has(path)) return `${path} is not part of a Krate app. Send Cargo.toml, manifest.toml, src/ and assets/.`;
    return null;
  }
  if (!TOP_DIRS.has(parts[0])) return `${path} is not part of a Krate app. Send Cargo.toml, manifest.toml, src/ and assets/.`;
  return null;
}

/* Check a whole upload before anything is written to disk.
 *
 * `files` is [{ path, data }] with data as base64. Answers { ok: true,
 * files: [{ path, bytes }] } or { ok: false, status, error }. Every check
 * runs here, so a refused upload leaves nothing behind and spawns nothing. */
export function checkCompileSource(files, { crates = DEFAULT_COMPILE_CRATES } = {}) {
  if (!Array.isArray(files) || !files.length) return refusal(400, "Send the app's files: Cargo.toml, manifest.toml and src/lib.rs at least.");
  if (files.length > MAX_COMPILE_FILES) return refusal(413, `That is more than ${MAX_COMPILE_FILES} files. Send the app's source, not its build output.`);
  const out = [];
  const seen = new Set();
  let total = 0;
  for (const item of files) {
    if (!item || typeof item !== "object") return refusal(400, "Every file is { path, data }.");
    // A link is not a file. JSON cannot make one by accident, so an entry
    // that says it is one is refused rather than written as its target.
    if ((item.type !== undefined && item.type !== "file") || item.link !== undefined || item.symlink !== undefined || item.target !== undefined) {
      return refusal(400, `${item.path || "a file"} is a link. Send files only.`);
    }
    const problem = compilePathProblem(item.path);
    if (problem) return refusal(400, problem);
    if (typeof item.data !== "string") return refusal(400, `${item.path} has no data (base64).`);
    if (!/^[A-Za-z0-9+/]*={0,2}$/.test(item.data.replace(/\s+/g, ""))) return refusal(400, `${item.path} is not base64.`);
    const bytes = Buffer.from(item.data, "base64");
    if (bytes.length > MAX_COMPILE_FILE_BYTES) return refusal(413, `${item.path} is more than ${MAX_COMPILE_FILE_BYTES / 1024 / 1024} MB.`);
    total += bytes.length;
    if (total > MAX_COMPILE_BYTES) return refusal(413, `That is more than ${MAX_COMPILE_BYTES / 1024 / 1024} MB of files.`);
    if (seen.has(item.path)) return refusal(400, `${item.path} is sent twice.`);
    seen.add(item.path);
    out.push({ path: item.path, bytes });
  }
  // A file and a folder of the same name cannot both exist.
  for (const path of seen) {
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i += 1) {
      const dir = parts.slice(0, i).join("/");
      if (seen.has(dir)) return refusal(400, `${dir} is sent as a file and as a folder.`);
    }
  }
  for (const required of ["Cargo.toml", "manifest.toml", "src/lib.rs"]) {
    if (!seen.has(required)) {
      return refusal(400, required === "manifest.toml"
        ? "There is no manifest.toml, so this is not a Krate app. Add one: [app] with id, name, version, entry and world, and its [[capabilities]]."
        : `${required} is missing. A Krate app has Cargo.toml, manifest.toml and src/lib.rs.`);
    }
  }
  const manifest = out.find((f) => f.path === "manifest.toml").bytes.toString("utf8");
  if (!/^\s*\[app\]\s*$/m.test(manifest)) {
    return refusal(400, "manifest.toml has no [app] section, so this is not a Krate app.");
  }
  const cargo = out.find((f) => f.path === "Cargo.toml").bytes.toString("utf8");
  const cargoProblem = cargoTomlProblem(cargo, { crates });
  if (cargoProblem) return refusal(400, cargoProblem);
  const readProblem = sourceReadProblem(out);
  if (readProblem) return refusal(400, readProblem);
  return { ok: true, files: out };
}

/* The SDK placeholders a Cargo.toml may use. `{KRATE_SDK}` is what a
 * bundle carries in place of a machine's SDK path; `<sdk>` is how the
 * authoring pack writes it and `PREFIX` how Krate Mode's templates do (the
 * guide this door hands out). All mean "Krate's SDK on the build machine",
 * and only as the start of a path: `PREFIX/crates/bindings-rust`. */
export const SDK_PLACEHOLDERS = ["{KRATE_SDK}", "<sdk>", "PREFIX"];

/* A line with its comment removed (a `#` outside a string). */
function stripComment(line) {
  let quote = null;
  for (let i = 0; i < line.length; i += 1) {
    const c = line[i];
    if (quote) {
      if (c === "\\" && quote === '"') { i += 1; continue; }
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === "#") {
      return line.slice(0, i);
    }
  }
  return line;
}

function keyParts(key) {
  return key.split(".").map((p) => p.trim().replace(/^["']|["']$/g, "")).filter(Boolean);
}

/* The crates.io crates an uploaded app may depend on, besides the SDK.
 *
 * An allow-list, because a crate's build script and a proc-macro run on
 * this box while it compiles, with the builder's own user -- and anybody
 * can publish a crate. The shipped apps under apps/ depend on exactly these
 * two (112 on wit-bindgen-rt, 7 on libm); widening it is a decision, made
 * with KRATE_COMPILE_CRATES, not a default. */
export const DEFAULT_COMPILE_CRATES = ["wit-bindgen-rt", "libm"];

/* What a Cargo.toml may not ask cargo to do on this box, or null.
 *
 * Cargo runs code or reads files at build time in a few named ways, and
 * each is a door from an upload into this machine: a build script
 * (`build.rs`, `package.build`, `[build-dependencies]`), a `links` native
 * library, a `[patch]`/`[replace]` that swaps a crate, a git or path
 * dependency, a workspace member or target path outside the app, and any
 * crate not on the allow-list above. The SDK's own path is the one path
 * allowed, written with a placeholder this box resolves.
 *
 * A line scan, not a TOML parser: it refuses more than it must rather than
 * less. Cargo is the real parser, and it runs only on what passes here. */
export function cargoTomlProblem(text, { crates = DEFAULT_COMPILE_CRATES } = {}) {
  const allowed = new Set(crates);
  let table = [];
  for (const raw of String(text).split(/\r?\n/)) {
    const line = stripComment(raw).trim();
    if (!line) continue;
    const header = /^\[\[?\s*([^\]]+?)\s*\]\]?$/.exec(line);
    let parts;
    if (header) {
      table = keyParts(header[1]);
      parts = table;
    } else {
      const eq = line.indexOf("=");
      if (eq <= 0) continue;
      parts = [...table, ...keyParts(line.slice(0, eq))];
    }
    const dotted = parts.join(".");
    if (parts.some((p) => p === "build-dependencies" || p === "build_dependencies")) {
      return "Cargo.toml has build-dependencies: those run a build script on the build machine, which is not accepted.";
    }
    if (parts[0] === "patch" || parts[0] === "replace") {
      return `Cargo.toml has [${parts[0]}], which swaps crates for others. Not accepted.`;
    }
    if (dotted === "cargo-features") return "Cargo.toml asks for unstable cargo features. Not accepted.";
    if (dotted === "package.build") return "Cargo.toml names a build script (package.build). Not accepted: a build script runs on the build machine.";
    if (dotted === "package.links") return "Cargo.toml links a native library (package.links). Not accepted.";
    if (dotted === "package.workspace") return "Cargo.toml points at another workspace (package.workspace). Not accepted.";
    if (parts[0] === "workspace" && !header) return "Cargo.toml's [workspace] must stay empty: the app is its own workspace.";
    // A dependency table is [dependencies], [dev-dependencies] or
    // [target.<cfg>.dependencies] -- not package.metadata.component's
    // WIT dependencies, which are paths and checked as paths below.
    const isDeps = (p) => p === "dependencies" || p === "dev-dependencies" || p === "dev_dependencies";
    const depAt = isDeps(parts[0]) ? 0 : parts[0] === "target" && isDeps(parts[2]) ? 2 : -1;
    if (depAt >= 0) {
      const name = parts[depAt + 1];
      if (header && name) return `Write dependencies inline under [dependencies] (Cargo.toml has [${dotted}]).`;
      if (!header && name) {
        if (name === "krate") {
          if (!/(^|[\s{,])path\s*=/.test(line)) return "Cargo.toml's krate dependency must be the SDK: krate = { path = \"{KRATE_SDK}/crates/bindings-rust\", features = [\"gui\"] }.";
        } else if (!allowed.has(name)) {
          return `Cargo.toml depends on ${name}. This build path takes only the Krate SDK and ${[...allowed].join(", ")} -- write the code in the app instead.`;
        }
        if (/(^|[\s{,])package\s*=/.test(line)) return `Cargo.toml renames a dependency (${name} = { package = ... }). Not accepted.`;
      }
    }
    if (!header) {
      // Inline tables carry keys inside one value: { git = "...", path = "..." }.
      if (/(^|[\s{,.])(git|registry|registry-index)\s*=/.test(line)) {
        return "Cargo.toml has a git or registry dependency. Only crates.io versions and the Krate SDK are accepted.";
      }
      if (/(^|[\s{,.])build\s*=/.test(line) && parts.includes("package")) {
        return "Cargo.toml names a build script. Not accepted: a build script runs on the build machine.";
      }
      const paths = [...line.matchAll(/(?:^|[\s{,.])path\s*=\s*(["'])(.*?)\1/g)].map((m) => m[2]);
      if (/(^|[\s{,.])path\s*=/.test(line) && !paths.length) {
        return "Cargo.toml has a path that is not a plain string.";
      }
      for (const path of paths) {
        if (dotted === "lib.path") {
          if (!path.startsWith("src/") || path.includes("..")) return `Cargo.toml's [lib] path must be inside src/ (it says ${path}).`;
          continue;
        }
        if (!SDK_PLACEHOLDERS.some((p) => path === p || path.startsWith(`${p}/`)) || path.includes("..")) {
          return `Cargo.toml points at ${path}. The only path a Krate app may name is Krate's SDK, written as {KRATE_SDK}/... (for example {KRATE_SDK}/crates/bindings-rust).`;
        }
      }
    }
  }
  return null;
}

/* What the Rust source may not read at compile time, or null.
 *
 * `include_str!`, `include_bytes!`, `include!` and `#[path = ...]` read a
 * file while the app compiles. Pointed at `/proc/1/environ` they read the
 * service's own secrets into the app -- or into a compiler error, which goes
 * straight back to the caller. So each must name one plain string that
 * stays inside the app's folder; anything computed (`concat!`, a macro
 * argument, an escape) is refused, because it cannot be checked here. */
const READS = /\b(include_str|include_bytes|include)\s*!\s*[(\[{]\s*([^)\]}]*)[)\]}]|#\s*!?\s*\[\s*path\s*=\s*([^\]]*)\]/g;

export function sourceReadProblem(files) {
  for (const file of files) {
    if (!file.path.endsWith(".rs")) continue;
    const text = file.bytes.toString("utf8");
    for (const m of text.matchAll(READS)) {
      const what = m[1] ? `${m[1]}!` : "#[path]";
      const arg = String(m[1] ? m[2] : m[3]).trim();
      const lit = /^"([^"\\]*)"$/.exec(arg);
      if (!lit) return `${file.path}: ${what} must name one plain "relative/path" (it says ${arg.slice(0, 80) || "nothing"}).`;
      const target = lit[1];
      if (target.startsWith("/") || /^[A-Za-z]:/.test(target)) return `${file.path}: ${what} reads ${target}, outside the app. Read files from the app's own folder.`;
      // Relative to the file that names it, as rustc resolves it.
      const parts = file.path.split("/").slice(0, -1);
      for (const seg of target.split("/")) {
        if (seg === "" || seg === ".") continue;
        if (seg === "..") {
          if (!parts.length) return `${file.path}: ${what} reads ${target}, outside the app. Read files from the app's own folder.`;
          parts.pop();
        } else parts.push(seg);
      }
    }
  }
  return null;
}

/* Point every SDK placeholder at this machine's SDK. A bare word
 * (`PREFIX`) only where it starts a path, so the word elsewhere stays. */
export function resolveSdkPlaceholders(text, root) {
  const at = String(root).replace(/\\/g, "/").replace(/\/+$/, "");
  let out = String(text);
  for (const p of SDK_PLACEHOLDERS) {
    if (/^[A-Z]+$/.test(p)) out = out.replace(new RegExp(`(["'])${p}(?=/|\\1)`, "g"), `$1${at}`);
    else out = out.split(p).join(at);
  }
  return out;
}

/* The crate's name from Cargo.toml's [package] table, as cargo names the
 * built file (`-` becomes `_`). Null when it cannot be read. */
export function crateFileName(cargoText) {
  let inPackage = false;
  for (const raw of String(cargoText).split(/\r?\n/)) {
    const line = stripComment(raw).trim();
    const header = /^\[\s*([^\]]+?)\s*\]$/.exec(line);
    if (header) { inPackage = header[1] === "package"; continue; }
    const m = inPackage && /^name\s*=\s*["']([^"']+)["']/.exec(line);
    if (m) return m[1].replace(/-/g, "_");
  }
  return null;
}

/* The app's own name and the permissions it asks for, read from the
 * manifest for the finished card. check-app has already parsed it properly;
 * this only reads two kinds of line back. */
export function manifestFacts(text) {
  let table = "";
  let name = "";
  const asks = [];
  for (const raw of String(text).split(/\r?\n/)) {
    const line = stripComment(raw).trim();
    const header = /^\[\[?\s*([^\]]+?)\s*\]\]?$/.exec(line);
    if (header) { table = header[1]; continue; }
    const m = /^(\w+)\s*=\s*"([^"]*)"/.exec(line);
    if (!m) continue;
    if (table === "app" && m[1] === "name") name = m[2];
    if (table === "capabilities" && m[1] === "cap" && !asks.includes(m[2])) asks.push(m[2]);
  }
  return { name, asks };
}

/* The last line of a run's output that is a JSON object: check-app --json
 * prints exactly one, after whatever the compiler said. */
export function lastJsonObject(text) {
  const lines = String(text).split("\n").map((l) => l.trim()).filter(Boolean);
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    if (!lines[i].startsWith("{")) continue;
    try {
      const value = JSON.parse(lines[i]);
      if (value && typeof value === "object" && typeof value.ok === "boolean") return value;
    } catch (e) { /* not this one */ }
  }
  return null;
}

/* Where the engine keeps its SDK: `<cache>/krate/sdk/<version>`, complete
 * when it holds `.complete` (crates/cli/src/sdk.rs). */
export function sdkCacheBase(env) {
  const cache = env.XDG_CACHE_HOME || join(env.HOME || homedir(), ".cache");
  return join(cache, "krate", "sdk");
}

/* The newest complete SDK under the cache, or null. Newest, because the
 * volume keeps the SDK of every engine this box has run, and the one the
 * current engine just wrote (or touched first) is the one it builds with. */
export async function newestSdk(env) {
  const base = sdkCacheBase(env);
  let names = [];
  try { names = await readdir(base); } catch (e) { return null; }
  let best = null;
  for (const name of names) {
    if (name.startsWith(".")) continue;
    const marker = await stat(join(base, name, ".complete")).catch(() => null);
    if (!marker) continue;
    if (!best || marker.mtimeMs > best.at) best = { root: join(base, name), at: marker.mtimeMs };
  }
  return best ? best.root : null;
}

/* Make sure this engine's SDK is on disk, and say where.
 *
 * The engine writes its embedded SDK the first time something needs it --
 * `krate create`, `krate pack` of a crate -- and prints its path nowhere.
 * So a fresh volume has none until a build has run. `krate pack` of a tiny
 * crate writes it before it gets to the (deliberately invalid) component,
 * so that is the nudge: model-free, a second long, and a no-op once the
 * SDK exists. KRATE_SDK_ROOT overrides all of it, as it does for the
 * engine's own port path. */
//
// The builder passes its sandbox (K-1293): `spawnImpl` runs the engine as
// the build user with its own environment, `scratch` makes a directory that
// user owns, `handOver` gives it what was written. `env` is that same
// environment, read here only to find the cache.
export async function ensureSdk({
  krate,
  env,
  timeoutMs = 60_000,
  spawnImpl = (cmd, args, opts) => spawn(cmd, args, { ...opts, env }),
  scratch: makeScratch = () => mkdtemp(join(tmpdir(), "krate-sdk-nudge-")),
  handOver = async () => {},
}) {
  if (env.KRATE_SDK_ROOT) return env.KRATE_SDK_ROOT;
  const scratch = await makeScratch();
  try {
    await writeFile(join(scratch, "Cargo.toml"), '[package]\nname = "sdk-nudge"\nversion = "0.0.0"\n');
    await writeFile(join(scratch, "manifest.toml"), '[app]\nid = "dev.krate.sdk_nudge"\nname = "x"\nversion = "0.0.0"\nentry = "code.wasm"\nworld = "krate:app/cli@0.1.0"\n');
    await writeFile(join(scratch, "code.wasm"), Buffer.alloc(0));
    await handOver(scratch);
    await new Promise((resolve) => {
      let proc;
      try {
        proc = spawnImpl(krate, ["pack", join(scratch, "code.wasm"), "--manifest", join(scratch, "manifest.toml"), "-o", join(scratch, "nudge.krate")], { stdio: "ignore" });
      } catch (e) {
        return resolve();
      }
      const killer = setTimeout(() => { try { proc.kill("SIGKILL"); } catch (e) {} }, timeoutMs);
      proc.on("error", () => { clearTimeout(killer); resolve(); });
      proc.on("close", () => { clearTimeout(killer); resolve(); });
    });
  } finally {
    await rm(scratch, { recursive: true, force: true }).catch(() => {});
  }
  return newestSdk(env);
}

/* A Cargo.toml for a windowed app, exactly as `krate create` writes one
 * (crates/author/src/lib.rs, checklist_cargo_toml) with the SDK written as
 * the placeholder. test/compile.test.mjs holds the two together. */
export const CARGO_TEMPLATE = `# An empty [workspace] table makes the generated app its own workspace
# root, so it builds standalone inside another workspace's directory tree.
[workspace]

[package]
name = "your-app"
version = "0.1.0-dev"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/incyashraj/krate"
rust-version = "1.91"

[dependencies]
# The Krate SDK with its \`gui\` feature: that is what makes this a windowed
# app (the feature selects the gui world) and gives it \`krate::ui\`,
# \`krate::gfx\` and the rest. Your app is \`#![no_std]\`, so the SDK also
# supplies the allocator, the panic handler, and the memory intrinsics.
# Keep it, because without it the app will not link.
krate = { path = "{KRATE_SDK}/crates/bindings-rust", features = ["gui"] }
wit-bindgen-rt = { version = "0.44.0", features = ["bitflags"] }

[lib]
crate-type = ["cdylib"]

# Keeps the generated bindings free of the standard library, so your app can
# stay \`#![no_std]\`. Turning this off links std, which pulls in operating
# system calls that Krate refuses, and the build fails at the import check.
[package.metadata.component.bindings]
std_feature = true

[package.metadata.component]
package = "krate:your-app"

[package.metadata.component.target]
path = "{KRATE_SDK}/wit/krate/phase4"
world = "gui"

[package.metadata.component.target.dependencies]
"krate:io" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/io" }
"krate:fs" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/fs" }
"krate:net" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/net" }
"krate:time" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/time" }
"krate:locale" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/locale" }
"krate:resources" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/resources" }
"krate:store" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/store" }
"krate:random" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/random" }
"krate:process" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/process" }
"krate:serial" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/serial" }
"krate:print" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/print" }
"krate:ui" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/ui" }
"krate:gfx" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/gfx" }
"krate:gpu" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/gpu" }
"krate:audio" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/audio" }
"krate:camera" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/camera" }
"krate:speech" = { path = "{KRATE_SDK}/wit/krate/phase4/deps/speech" }

[profile.release]
panic = "abort"
lto = true
codegen-units = 1
opt-level = "s"
`;

/* The template with only the WIT packages this machine's SDK has. The
 * guide's template follows the newest starter (test/compile.test.mjs holds
 * them together), and the engine on the builder can be a release behind:
 * a dependency on a WIT folder its SDK lacks fails every build. `present`
 * is the SDK's wit/krate/phase4/deps listing, or null when it could not be
 * read -- then nothing is dropped. */
export function cargoTemplateFor(present) {
  if (!present) return CARGO_TEMPLATE;
  const have = new Set(present);
  return CARGO_TEMPLATE.split("\n").filter((line) => {
    const m = /^"krate:([a-z0-9-]+)" = \{ path = "\{KRATE_SDK\}\/wit\/krate\/phase4\/deps\/([a-z0-9-]+)" \}$/.exec(line);
    return !m || have.has(m[2]);
  }).join("\n");
}

/* The guide a chat model reads before it writes an app for this door.
 *
 * The engine's Krate Mode text (`krate krate-mode`): the whole API, the
 * permission catalog, the no_std rules and two worked examples, written for
 * a chat model with no compiler -- about 110 KB, against 195 KB for the
 * authoring pack (`krate authoring-context`), which is written for an agent
 * working in a starter folder and repeats what a tool result does not need.
 * Krate Mode ends with "The handoff: you cannot build this"; this door CAN
 * build, so that part is cut and replaced by how to send the files. */
export function connectorGuide(mode, { crates = DEFAULT_COMPILE_CRATES, present = null } = {}) {
  let text = String(mode || "");
  const handoff = /^# The handoff\b.*$/m.exec(text);
  if (handoff) text = text.slice(0, handoff.index);
  text = text.trimEnd().replace(/\n-{3,}\s*$/, "").trimEnd();
  return text + guideAppendix(crates, cargoTemplateFor(present));
}

/* What the guide adds after Krate Mode: how to hand the source over. A
 * chat model starts from nothing, so it gets the starter's Cargo.toml and
 * the rules of this door. */
export const guideAppendix = (crates = DEFAULT_COMPILE_CRATES, template = CARGO_TEMPLATE) => `

---

# Sending the app to Krate to build

This part replaces any advice above to hand the files to the person to
build: you can build. Send the files to Krate's build tool and it compiles,
checks and packs them with no AI of its own; the answer is either the app
or check-app's own words about what to fix. Send these files:

- \`Cargo.toml\` -- start from the one below. Change only \`name\` (and the
  \`krate:<name>\` package to match). The only other crates this build
  takes are ${crates.join(", ")}, by crates.io version. Where a template
  above says \`PREFIX\`, write \`{KRATE_SDK}\`: the build machine puts
  its own SDK there.
- \`manifest.toml\` -- \`[app]\` with \`id\`, \`name\`, \`version\`,
  \`entry = "target/wasm32-wasip1/release/<name with _ for ->.wasm"\` and
  \`world = "krate:app/gui@0.2.0"\` (a CLI app: \`krate:app/cli@0.1.0\`),
  then one \`[[capabilities]]\` per permission the app uses (section 2
  lists them). A window needs \`ui.window:create\`.
- \`src/lib.rs\` and any other files under \`src/\`.
- \`assets/...\` if the app ships images, fonts or sounds.

Not accepted, and refused before anything is built: \`build.rs\` or any
build script, \`[build-dependencies]\`, \`[patch]\`, git or path dependencies
(other than the SDK), \`.cargo/\`, \`rust-toolchain.toml\`, \`target/\`, and
\`include_str!\`/\`include_bytes!\`/\`#[path]\` naming anything but one plain
relative path inside the app.

The build runs \`krate check-app\` -- build, import check, a headless run
with the bare argument \`quick\`, usability -- and packs the app only when
every stage passes. When it fails, the answer names the stage, what went
wrong and the fix: correct the code and send every file again.

\`\`\`toml
${template}\`\`\`
`;
