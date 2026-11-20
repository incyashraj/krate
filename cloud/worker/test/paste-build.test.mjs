/* Make an app in any AI chat (paste-build.js): the parser that reads an AI's
 * answer into files, and the /build page's door, driven through the real
 * worker with the build service stood in for, as mcp-build.test.mjs does.
 *
 * What must hold: the documented format and the forms an AI drifts into
 * read back exactly; a missing file, a broken code block, a path outside
 * the app and too much text are each refused in words the AI can act on;
 * signed out, nothing reaches the builder; a build here is the connector's
 * build -- same builder contract, same per-account limits counted together,
 * same check words, same unlisted publish -- and a failure comes back as a
 * box to paste into the chat.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/paste-build.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";
import { parsePastedApp, buildPage } from "../src/paste-build.js";

const here = dirname(fileURLToPath(import.meta.url));
const FINISHED = new Uint8Array(readFileSync(join(here, "fixtures", "notes-0.1.0.krate")));
const BASE = "https://hub.example";
const BUILDER = "https://builder.example";
const SECRET = "test-builder-secret";

/* ---- GitHub's token exchange and the build service, stood in for ------- */
let nextLogin = "alice";
const builder = { jobs: new Map(), calls: [] };
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  const headers = new Headers((input && input.headers) || init.headers || {});
  const reply = (v, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });
  if (url === "https://github.com/login/oauth/access_token") return reply({ access_token: `gho_${nextLogin}` });
  if (url.startsWith(BUILDER)) {
    const path = url.slice(BUILDER.length);
    const account = headers.get("x-krate-account");
    builder.calls.push({ path, account });
    if (headers.get("x-krate-builder") !== SECRET) return new Response("not found", { status: 404 });
    if (path === "/compile") {
      const id = randomBytes(16).toString("hex");
      builder.jobs.set(id, { account, state: "building", body: JSON.parse(init.body), check: null });
      return reply({ id }, 202);
    }
    const m = /^\/build\/([0-9a-f]{32})(\/file)?$/.exec(path);
    if (m) {
      const job = builder.jobs.get(m[1]);
      if (!job || job.account !== account) return new Response("no such build", { status: 404 });
      if (m[2]) return job.state === "done" ? new Response(FINISHED) : new Response("not ready", { status: 404 });
      return reply({ state: job.state, stage: "build", line: "compiling", check: job.check, error: null });
    }
  }
  return before(input, init);
};

const hubEnv = (extra = {}) => {
  const env = orgEnv({
    PUBLIC_BASE: BASE,
    KEY_WRAP_SECRET: "test-wrapping-secret",
    GITHUB_CLIENT_SECRET: "gh-secret",
    KRATE_BUILDER_URL: BUILDER,
    KRATE_BUILDER_SECRET: SECRET,
    ...extra,
  });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

/* The person signs in from the page: the hub's own GitHub sign-in, back to /build. */
async function signIn(env, login) {
  nextLogin = login;
  const nonce = randomBytes(16).toString("hex");
  const start = await call(env, null, "GET", `/login/start?from=build&nonce=${nonce}`);
  assert.equal(start.status, 302);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  const back = await call(env, null, "GET", `/login/callback?state=${state}&code=abc`);
  const where = back.headers.get("location");
  assert.ok(where.startsWith("/build#"), `lands back on the paste page, got ${where}`);
  const fields = new URLSearchParams(where.split("#")[1]);
  assert.equal(fields.get("nonce"), nonce, "with the page's own nonce");
  return fields.get("token");
}

const CARGO = '[package]\nname = "tip-split"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\ncrate-type = ["cdylib"]\n';
const MANIFEST = '[app]\nid = "dev.krate.tipsplit"\nname = "Tip Split"\nversion = "0.1.0"\n';
const LIB = 'use krate::prelude::*;\n\n/// Splits a bill.\n/// ```\n/// let x = 1;\n/// ```\npub fn split(total: f32, people: u32) -> f32 {\n    (total / people as f32).round()\n}\n';

/* What an AI answers when it follows the guide: words, then one block. */
const ANSWER = `Here is your Krate app. Copy the whole block below.

\`\`\`text
--- FILE: Cargo.toml ---
${CARGO}
--- FILE: manifest.toml ---
${MANIFEST}--- FILE: src/lib.rs ---
${LIB}--- END ---
\`\`\`

Paste it at hub.krate.tech/build and press Build.`;

const start = (env, token, text) =>
  call(env, null, "POST", "/build/start", { json: { text }, headers: token ? { authorization: `Bearer ${token}` } : {} });
const status = (env, token, id) => call(env, null, "GET", `/build/status/${id}`, { headers: { authorization: `Bearer ${token}` } });
const compiles = () => builder.calls.filter((c) => c.path === "/compile").length;

// ------------------------------------------------------------------ parser

test("the documented format reads back exactly, the AI's words around it ignored", () => {
  const got = parsePastedApp(ANSWER);
  assert.equal(got.problem, undefined, got.problem);
  assert.equal(got.name, "Tip Split", "the name from manifest.toml");
  assert.deepEqual(got.files, [
    { path: "Cargo.toml", content: CARGO },
    { path: "manifest.toml", content: MANIFEST },
    { path: "src/lib.rs", content: LIB },
  ]);
  // Copied as rendered text, the block's fences are gone: the same files.
  const rendered = ANSWER.replace(/^```.*$/gm, "");
  assert.deepEqual(parsePastedApp(rendered).files, got.files);
  // Windows line ends read the same.
  assert.deepEqual(parsePastedApp(ANSWER.replace(/\n/g, "\r\n")).files, got.files);
});

test("the forms an AI drifts into: a code block per file, bold or heading markers, a path in backticks", () => {
  const drift = `Sure! Here are the files.

**File: \`Cargo.toml\`**
\`\`\`toml
${CARGO}\`\`\`

### FILE: manifest.toml
\`\`\`toml
${MANIFEST}\`\`\`

=== FILE: src/lib.rs ===
\`\`\`rust
${LIB}\`\`\`

Let me know if you want changes.`;
  const got = parsePastedApp(drift);
  assert.equal(got.problem, undefined, got.problem);
  assert.deepEqual(got.files.map((f) => f.path), ["Cargo.toml", "manifest.toml", "src/lib.rs"]);
  assert.equal(got.files[2].content, LIB, "a rustdoc example's ``` stays inside the file");
  // Prose after the last block is cut at --- END --- only; without it the
  // last file's block ends at its own fence.
  assert.ok(!got.files[2].content.includes("Let me know"), got.files[2].content);
});

test("a missing manifest, no markers, a broken code block, a bad path and a file twice are refused in words", () => {
  const noManifest = parsePastedApp(`--- FILE: Cargo.toml ---\n${CARGO}--- FILE: src/lib.rs ---\n${LIB}--- END ---`);
  assert.match(noManifest.problem, /no manifest\.toml/);
  assert.match(parsePastedApp("```rust\nfn main() {}\n```").problem, /No files were found/);
  assert.match(parsePastedApp("").problem, /Nothing was pasted/);

  // Two files run together inside one block: a fence line is left in the middle.
  const wrongFence = `--- FILE: Cargo.toml ---\n${CARGO}--- FILE: manifest.toml ---\n${MANIFEST}--- FILE: src/lib.rs ---\n\`\`\`rust\n${LIB}\`\`\`\nmore words\n\`\`\`rust\nfn other() {}\n\`\`\`\n--- END ---`;
  const wf = parsePastedApp(wrongFence);
  assert.match(wf.problem, /src\/lib\.rs is followed by another code block/);
  // A block closed in the middle of a file: the rest of it would be lost.
  const cut = `--- FILE: Cargo.toml ---\n${CARGO}\`\`\`\n[dependencies]\n--- FILE: manifest.toml ---\n${MANIFEST}--- FILE: src/lib.rs ---\n${LIB}--- END ---`;
  assert.match(parsePastedApp(cut).problem, /Cargo\.toml has a code-block line \(```\) inside it, at its line 8/);

  const escape = parsePastedApp(`--- FILE: ../../etc/passwd ---\nx\n--- FILE: Cargo.toml ---\n${CARGO}--- END ---`);
  assert.match(escape.problem, /not a path inside the app folder/);
  assert.match(parsePastedApp(`--- FILE: /etc/passwd ---\nx\n`).problem, /not a path inside/);
  assert.match(parsePastedApp(`--- FILE: Cargo.toml ---\n${CARGO}--- FILE: Cargo.toml ---\n${CARGO}`).problem, /twice/);
});

test("too much is refused before anything is built: over 2 MB of source, over 3 MB of text, over 64 files", () => {
  const big = parsePastedApp(`--- FILE: Cargo.toml ---\n${CARGO}--- FILE: manifest.toml ---\n${MANIFEST}--- FILE: src/lib.rs ---\n${"// x\n".repeat(450_000)}--- END ---`);
  assert.match(big.problem, /more than 2 MB/);
  assert.match(parsePastedApp("x".repeat(3 * 1024 * 1024 + 1)).problem, /more than 3 MB/);
  let many = `--- FILE: Cargo.toml ---\n${CARGO}--- FILE: manifest.toml ---\n${MANIFEST}--- FILE: src/lib.rs ---\n${LIB}`;
  for (let i = 0; i < 64; i += 1) many += `--- FILE: assets/n${i}.txt ---\n${i}\n`;
  assert.match(parsePastedApp(many).problem, /64 at most/);
});

test("the page carries the same parser, its script compiles, and it says what it is", async () => {
  const res = buildPage();
  const html = await res.text();
  assert.match(res.headers.get("content-security-policy"), /connect-src 'self'/);
  assert.match(res.headers.get("content-security-policy"), /frame-ancestors 'none'/);
  const script = /<script>([\s\S]*)<\/script>/.exec(html)[1];
  assert.ok(script.includes(parsePastedApp.toString()), "the page parses with the server's own function");
  assert.doesNotThrow(() => new Function(script), "the page's script is valid JavaScript");
  assert.match(html, /krate\.tech\/krate-mode\.md/, "the prompt sends the AI to the guide");
  assert.match(html, /--- FILE: &lt;path&gt; ---/);
  assert.match(html, /Copy this back to your AI/);
  assert.match(html, /<meta name="viewport" content="width=device-width, initial-scale=1">/);
  assert.match(html, /prefers-color-scheme: dark/);
});

// ------------------------------------------------------------------- door

test("signed out: the page opens, a build is refused and nothing reaches the builder", async () => {
  const env = hubEnv();
  const page = await call(env, null, "GET", "/build");
  assert.equal(page.status, 200);
  assert.match(page.headers.get("content-type"), /text\/html/);
  const before = compiles();
  const r = await start(env, null, ANSWER);
  assert.equal(r.status, 401);
  assert.equal(r.body.signed_out, true);
  const forged = await start(env, "not-a-session", ANSWER);
  assert.equal(forged.status, 401);
  assert.equal((await call(env, null, "GET", `/build/status/${"0".repeat(32)}`)).status, 401);
  assert.equal(compiles(), before, "the builder was not asked");
});

test("paste, build, read the check's words as a box for the chat, paste the fix, get a link", async () => {
  const env = hubEnv();
  const token = await signIn(env, "alice");

  // A paste Krate cannot read: words and a box to copy back, no build.
  const before = compiles();
  const bad = await start(env, token, "Here you go:\n```rust\nfn main() {}\n```");
  assert.equal(bad.status, 422);
  assert.match(bad.body.problem, /No files were found/);
  assert.match(bad.body.copy_back, /^Krate could not read the files: No files were found[\s\S]*--- FILE: <path> ---[\s\S]*--- END ---/);
  assert.equal(compiles(), before);

  // The answer: sent to the builder as the connector sends it, for alice.
  const first = await start(env, token, ANSWER);
  assert.equal(first.status, 202, JSON.stringify(first.body));
  assert.equal(first.body.name, "Tip Split");
  assert.deepEqual(first.body.files, ["Cargo.toml", "manifest.toml", "src/lib.rs"]);
  const job = builder.jobs.get(first.body.build_id);
  assert.equal(job.account, "hub:github:alice", "the builder is told whose job it is");
  assert.equal(job.body.name, "Tip Split");
  assert.equal(Buffer.from(job.body.files.find((f) => f.path === "src/lib.rs").data, "base64").toString(), LIB);

  assert.equal((await status(env, token, first.body.build_id)).body.status, "building");
  job.state = "failed";
  job.check = { stage: "build", kind: "compile", detail: "error[E0425]: cannot find value `people_count`", fix: "Declare people_count before using it." };
  const failed = (await status(env, token, first.body.build_id)).body;
  assert.equal(failed.status, "failed");
  assert.match(failed.copy_back, /^Krate built the app and its check did not pass\.[\s\S]*the build stage[\s\S]*E0425[\s\S]*Declare people_count[\s\S]*--- FILE: <path> ---/);
  assert.doesNotMatch(JSON.stringify(failed), /build_app/, "the page's words name no connector tool");

  // The fixed answer passes and is published unlisted to alice's account.
  const second = await start(env, token, ANSWER);
  builder.jobs.get(second.body.build_id).state = "done";
  const done = (await status(env, token, second.body.build_id)).body;
  assert.equal(done.status, "published", JSON.stringify(done));
  assert.match(done.link, /^https:\/\/hub\.example\/a\/v[0-9a-f]{11}$/);
  assert.equal(done.browser_link, `${BASE}/run/${done.link.split("/a/")[1]}`);
  const row = JSON.parse(env._kv.get(`app:${done.app_id}`));
  assert.equal(row.unlisted, true);
  assert.equal(row.owner, "github:alice");
  assert.equal(row.name, "Tip Split");

  // Bob cannot see alice's build.
  const bob = await signIn(env, "bob");
  assert.equal((await status(env, bob, second.body.build_id)).body.status, "unknown");
});

test("the page and the connector share one account's limit: 10 an hour by default, here 2", async () => {
  const env = hubEnv({ MCP_BUILDS_PER_HOUR: "2" });
  const token = await signIn(env, "carol");
  assert.equal((await start(env, token, ANSWER)).status, 202);
  assert.equal((await start(env, token, ANSWER)).status, 202);
  const before = compiles();
  const third = await start(env, token, ANSWER);
  assert.equal(third.status, 429);
  assert.match(third.body.problem, /2 builds an hour/);
  assert.equal(compiles(), before, "the builder was not asked");
  // The account's record is the connector's: one key, counted together.
  assert.equal(JSON.parse(env._kv.get("mcpbuilds:github:carol")).length, 2);
});
