/* Porting an uploaded project through the build service (2026-10-03).
 *
 * The page uploads a folder as relative paths and bytes. The service writes
 * them back to disk, asks the engine for its read-only plan (`/port/plan`,
 * no model, nothing counted), and ports on request (`/port`) through the
 * same wall, slot and ledger as a build. The engine's exit 7 -- built,
 * permission-tested, not yet compared with the original -- is a finished
 * job carrying that note, never a failure.
 *
 * Drives the REAL server with a fake hub and a fake `krate` script, like
 * jobs.test.mjs. Pinned here:
 *   - the plan comes back as the engine's JSON, and the upload is deleted;
 *   - a path that steps outside the project is refused before anything runs;
 *   - a port ends "done" with verdict "ported" and the permissions the
 *     engine's artifact record names;
 *   - the project reaches the engine with its directory structure intact.
 *
 *   node --test cloud/builder/test/port.test.mjs
 */
import assert from "node:assert";
import test from "node:test";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { chmod, mkdtemp, readFile, writeFile, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

const HUB_PORT = 8935;
const BUILDER_PORT = 8936;
const BUILDER = `http://127.0.0.1:${BUILDER_PORT}`;
const BUILDER_SECRET = "builder-secret-for-tests";

const hub = createServer((req, res) => {
  const token = (req.headers.authorization || "").replace(/^Bearer\s+/i, "");
  if (req.url === "/me") {
    if (token !== "alice-token") { res.statusCode = 401; return res.end("no"); }
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify({ user: { login: "alice" }, plan: { active: true } }));
  }
  if (req.url === "/plan/count") return res.end("{}");
  if (req.url === "/keys/use") { res.statusCode = 404; return res.end(""); }
  if (req.url === "/case/open") { res.setHeader("content-type", "application/json"); return res.end(JSON.stringify({ id: "case-1", n: 0 })); }
  if (req.url === "/case/attempt") { res.setHeader("content-type", "application/json"); return res.end(JSON.stringify({ id: "case-1", made: true })); }
  if (req.url === "/spend") return res.end("{}");
  res.statusCode = 404; res.end("");
});
await new Promise((r) => hub.listen(HUB_PORT, r));

// The fake engine records what `port` was handed, writes a bundle and an
// artifact record, and exits 7 the way the real one does.
const fakeDir = await mkdtemp(join(tmpdir(), "krate-fake-"));
const seen = join(fakeDir, "seen.txt");
const krateBin = join(fakeDir, "krate");
await writeFile(krateBin, `#!/bin/sh
if [ "$1" = "port" ]; then
  src=""; prepare=""; out=""; transcript=""; prev=""; after=""
  for a in "$@"; do
    if [ "$prev" = "--prepare" ]; then prepare="$a"; fi
    if [ "$prev" = "--to" ]; then out="$a"; fi
    if [ "$prev" = "--transcript" ]; then transcript="$a"; fi
    if [ "$after" = "1" ] && [ -z "$src" ]; then src="$a"; fi
    if [ "$a" = "--" ]; then after=1; fi
    prev="$a"
  done
  if [ -z "$out" ]; then
    # the plan: say what was uploaded, so the test can prove the tree arrived
    files=$(cd "$src" && find . -type f | sort | tr '\\n' ',')
    echo "{\\"schema\\":\\"krate.port.plan.v1\\",\\"verdict\\":\\"needs-changes\\",\\"languages\\":[\\"python\\"],\\"frameworks\\":[\\"tkinter\\"],\\"findings\\":[],\\"suggested_capabilities\\":[\\"ui.window:create\\"],\\"scan\\":{\\"files_scanned\\":2},\\"files\\":\\"$files\\"}"
    exit 0
  fi
  echo "$src" > "${seen}"
  echo "$prepare" >> "${seen}"
  (cd "$src" && find . -type f | sort) >> "${seen}"
  echo "==> transforming the candidate"
  sleep 0.2
  echo "==> validating the port candidate"
  echo "==> packing $out"
  echo "==> verifying the permission wall"
  mkdir -p "$prepare"
  printf '{"schema":"krate.port.artifact.v1","requested_permissions":["ui.window:create","fs.write:todos.json"]}' > "$prepare/artifact.json"
  printf 'not-a-real-bundle' > "$out"
  [ -n "$transcript" ] && printf '{"schema":"krate.port.result.v1","output":"%s"}' "$out" > "$transcript"
  echo "This is not a finished port yet."
  exit 7
fi
exit 1
`.replaceAll("${seen}", seen));
await chmod(krateBin, 0o755);

const stateDir = await mkdtemp(join(tmpdir(), "krate-builder-"));
const server = spawn("node", ["cloud/builder/src/server.js"], {
  env: {
    ...process.env,
    PORT: String(BUILDER_PORT),
    KRATE_HUB: `http://127.0.0.1:${HUB_PORT}`,
    KRATE_BUILDER_SECRET: BUILDER_SECRET,
    KRATE_BIN: krateBin,
    KRATE_STATE_DIR: stateDir,
    KRATE_AGENT: "claude",
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
const project = {
  name: "tkinter-todo",
  files: [
    { path: "todo.py", bytes: b64("import tkinter as tk\nroot = tk.Tk()\n") },
    { path: "lib/util.py", bytes: b64("def f(): pass\n") },
    { path: ".git/HEAD", bytes: b64("ref: refs/heads/main\n") },
    { path: "node_modules/x/index.js", bytes: b64("x") },
  ],
};
const post = (path, body, token = "alice-token") => fetch(`${BUILDER}${path}`, {
  method: "POST",
  headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
  body: JSON.stringify(body),
});

test("the plan comes back as the engine's JSON, with the tree the person uploaded and nothing they did not mean", async () => {
  const res = await post("/port/plan", { project });
  const text = await res.text();
  assert.equal(res.status, 200, text);
  const plan = JSON.parse(text);
  assert.equal(plan.schema, "krate.port.plan.v1");
  assert.equal(plan.verdict, "needs-changes");
  assert.match(plan.files, /\.\/todo\.py/, "the file at the root arrived");
  assert.match(plan.files, /\.\/lib\/util\.py/, "and the one in a subfolder, with its folder");
  assert.doesNotMatch(plan.files, /\.git|node_modules/, ".git and node_modules were not written");
});

test("a path that steps outside the project is refused before the engine runs", async () => {
  for (const path of ["../etc/passwd", "/etc/passwd", "a/../../b.py", "src//x.py"]) {
    const res = await post("/port/plan", { project: { name: "p", files: [{ path, bytes: b64("x") }] } });
    assert.equal(res.status, 400, `${path}: ${res.status}`);
  }
  const none = await post("/port/plan", { project: { name: "p", files: [] } });
  assert.equal(none.status, 400);
});

test("without an account there is no plan", async () => {
  const res = await post("/port/plan", { project }, "nobody");
  assert.equal(res.status, 401);
});

test("a port ends done, with verdict ported and the artifact's permissions", async () => {
  const started = await post("/port", { device: "d1", project });
  const startedText = await started.text();
  assert.equal(started.status, 200, startedText);
  const { id } = JSON.parse(startedText);
  let job;
  for (let i = 0; i < 100; i += 1) {
    const res = await fetch(`${BUILDER}/build/${id}`, { headers: { authorization: "Bearer alice-token" } });
    job = await res.json();
    if (job.state !== "working") break;
    await new Promise((r) => setTimeout(r, 100));
  }
  assert.equal(job.state, "done", JSON.stringify(job) + "\n" + serverLog.slice(-1500));
  assert.equal(job.result.verdict, "ported");
  assert.match(job.result.verdict_detail, /Not yet compared with the original/);
  assert.deepEqual(job.result.asks, ["ui.window:create", "fs.write:todos.json"]);
  assert.equal(job.result.name, "Tkinter todo", "named after the project, not \"Port x\"");
  const handed = await readFile(seen, "utf8");
  assert.match(handed, /\.\/todo\.py/, "the engine was pointed at the written project");
  assert.match(handed, /\.\/lib\/util\.py/);
  const [src, work] = handed.split("\n");
  assert.equal(dirname(work), dirname(src), "the workspace sat beside the project, not inside it");
  assert.equal(basename(work), "work");
  assert.equal(basename(src), "tkinter-todo", "the engine sees the folder's own name, not a generic one");
  assert.equal(await stat(src).catch(() => null), null, "the upload was deleted once the job was done");
});

test.after(() => { server.kill("SIGKILL"); hub.close(); });
