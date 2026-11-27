/* Krate for Claude Desktop through the hub (IC-1005): the connector's
 * get_krate_for_claude_desktop tool, the stable /claude/krate-<os>.mcpb
 * addresses, and the one line a finished app offers to Claude.
 *
 * What it must be true of: before any release carries the files, the
 * addresses answer a page that says the file is coming (never a 404 and
 * never a redirect to nothing) and the tool says the same; once a release
 * carries them, each address redirects to that release's own file on
 * GitHub and the tool answers the link for the person's computer with the
 * two steps and the real one-time builder size; and only Claude is
 * offered "open it on this computer", only once the file exists.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/claude-desktop.test.mjs
 */
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const NOTES = new Uint8Array(readFileSync(join(here, "fixtures", "notes-0.1.0-signed-a.krate")));

const MAC = "https://github.com/incyashraj/krate/releases/download/v0.6.2/krate-0.6.2-mac.mcpb";
const WIN = "https://github.com/incyashraj/krate/releases/download/v0.6.2/krate-0.6.2-windows.mcpb";
const WITHOUT = {
  tag_name: "v0.6.1",
  assets: [{ name: "krate-0.6.1-aarch64-apple-darwin.tar.gz", browser_download_url: "https://github.com/x", size: 1 }],
};
const WITH = {
  tag_name: "v0.6.2",
  assets: [
    ...WITHOUT.assets,
    { name: "krate-0.6.2-mac.mcpb", browser_download_url: MAC, size: 46_000_000 },
    { name: "krate-0.6.2-windows.mcpb", browser_download_url: WIN, size: 25_000_000 },
  ],
};

// GitHub's latest-release answer, as the hub's mirror asks for it, and
// Cloudflare's cache as an empty one each test.
let latest = WITHOUT;
let githubDown = false;
const realFetch = globalThis.fetch;
beforeEach(() => {
  globalThis.caches = { default: { match: async () => undefined, put: async () => {} } };
  globalThis.fetch = async (input, init) => {
    const url = String((input && input.url) || input);
    if (url === "https://api.github.com/repos/incyashraj/krate/releases/latest") {
      if (githubDown) return new Response("down", { status: 500 });
      return new Response(JSON.stringify(latest), { headers: { "content-type": "application/json" } });
    }
    return realFetch(input, init);
  };
});
afterEach(() => {
  delete globalThis.caches;
  globalThis.fetch = realFetch;
  latest = WITHOUT;
  githubDown = false;
});

const hubEnv = () => {
  const env = orgEnv({ PUBLIC_BASE: "https://hub.example" });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

let nextId = 1;
const tool = (env, name, args, headers = {}) =>
  call(env, null, "POST", "/mcp", {
    json: { jsonrpc: "2.0", id: nextId++, method: "tools/call", params: { name, arguments: args } },
    headers: { accept: "application/json, text/event-stream", "cf-connecting-ip": "192.0.2.9", ...headers },
  });

test("the tool is listed read-only, needs no sign-in, and names no other tool", async () => {
  const env = hubEnv();
  const r = await call(env, null, "POST", "/mcp", {
    json: { jsonrpc: "2.0", id: 1, method: "tools/list" },
    headers: { "cf-connecting-ip": "192.0.2.9" },
  });
  const t = r.body.result.tools.find((x) => x.name === "get_krate_for_claude_desktop");
  assert.ok(t, "listed");
  assert.equal(t.annotations.readOnlyHint, true);
  assert.equal(t.annotations.title, "Get Krate for Claude Desktop");
  assert.deepEqual(t._meta.securitySchemes, [{ type: "noauth" }]);
  assert.deepEqual(t.inputSchema.properties.os.enum, ["mac", "windows"]);
});

test("before a release carries the file: the address says it is coming and the tool says so too", async () => {
  const env = hubEnv();
  for (const file of ["krate-mac.mcpb", "krate-windows.mcpb"]) {
    const r = await call(env, null, "GET", `/claude/${file}`);
    assert.equal(r.status, 200, "never a dead link");
    assert.match(r.headers.get("content-type"), /text\/html/);
    assert.match(String(r.body), /comes with the next Krate release/);
    assert.match(String(r.body), /https:\/\/krate\.tech\/claude\//);
  }
  const answer = (await tool(env, "get_krate_for_claude_desktop", { os: "mac" })).body.result;
  assert.equal(answer.isError, undefined);
  assert.equal(answer.structuredContent.downloads.length, 1);
  assert.equal(answer.structuredContent.downloads[0].available, false);
  assert.match(answer.content[0].text, /next Krate release/);
  assert.doesNotMatch(answer.content[0].text, /krate-mac\.mcpb/, "no link to a file that is not there");
});

test("once a release carries it: each address redirects to that release's own file", async () => {
  latest = WITH;
  const env = hubEnv();
  const mac = await call(env, null, "GET", "/claude/krate-mac.mcpb");
  assert.equal(mac.status, 302);
  assert.equal(mac.headers.get("location"), MAC);
  const win = await call(env, null, "GET", "/claude/krate-windows.mcpb");
  assert.equal(win.headers.get("location"), WIN);
  const head = await call(env, null, "HEAD", "/claude/krate-mac.mcpb");
  assert.equal(head.status, 302);
  assert.equal((await call(env, null, "GET", "/claude/krate-linux.mcpb")).status === 302, false, "only the two systems Claude Desktop runs on");
});

test("GitHub unreachable: the address says try again, not that the file is gone", async () => {
  githubDown = true;
  const r = await call(hubEnv(), null, "GET", "/claude/krate-mac.mcpb");
  assert.equal(r.status, 503);
  assert.match(String(r.body), /Try again in a minute/);
});

test("the tool answers the link for the person's computer, the two steps and the real builder size", async () => {
  latest = WITH;
  const env = hubEnv();
  const mac = (await tool(env, "get_krate_for_claude_desktop", { os: "mac" })).body.result;
  const s = mac.structuredContent;
  assert.deepEqual(s.downloads.map((d) => [d.os, d.url, d.available, d.size_bytes]), [
    ["mac", "https://krate.tech/claude/krate-mac.mcpb", true, 46_000_000],
  ]);
  assert.equal(s.downloads[0].builder_download, "about 127-141 MB");
  assert.equal(s.steps.length, 2);
  assert.match(s.steps[1], /Click Install/);
  assert.match(mac.content[0].text, /https:\/\/krate\.tech\/claude\/krate-mac\.mcpb/);
  assert.doesNotMatch(mac.content[0].text, /windows\.mcpb/, "one computer named, one link");

  const both = (await tool(env, "get_krate_for_claude_desktop", {})).body.result;
  assert.deepEqual(both.structuredContent.downloads.map((d) => d.os), ["mac", "windows"]);
  assert.equal(both.structuredContent.downloads[1].builder_download, "about 226 MB");
  assert.match(both.content[0].text, /about 127-141 MB on a Mac, about 226 MB on Windows/);

  const bad = await tool(env, "get_krate_for_claude_desktop", { os: "linux" });
  assert.equal(bad.body.error.code, -32602, "Claude Desktop runs on a Mac and Windows only");
});

test("a finished app offers 'open it on this computer' to Claude only, and only once the file exists", async () => {
  const env = hubEnv();
  const published = await call(env, "alice", "POST", "/publish", { bytes: NOTES, headers: { "x-krate-name": "Notes" } });
  assert.equal(published.status, 200, JSON.stringify(published.body));
  const ask = (ua) => tool(env, "get_app", { id_or_link: published.body.id }, ua ? { "user-agent": ua } : {});
  const LINE = /add Krate to Claude Desktop once: https:\/\/krate\.tech\/claude\//;

  assert.doesNotMatch((await ask("Claude-User")).body.result.content[0].text, LINE, "no release has the file yet");
  latest = WITH;
  assert.match((await ask("Claude-User")).body.result.content[0].text, LINE);
  assert.doesNotMatch((await ask("openai-mcp/1.0")).body.result.content[0].text, LINE, "not offered in ChatGPT");
  assert.doesNotMatch((await ask("")).body.result.content[0].text, LINE, "nor to a client that does not say");
});

test("the builder sizes said are the archives krate setup rust really fetches", async () => {
  const { DESKTOP_SYSTEMS } = await import("../src/claude-desktop.js");
  const src = readFileSync(join(here, "..", "..", "..", "crates", "cli", "src", "rust_toolchain.rs"), "utf8");
  const version = /pub const VERSION: &str = "([^"]+)";/.exec(src)[1];
  const sizes = new Map([...src.matchAll(/\(\s*"([^"]+)",\s*"[0-9a-f]{64}",\s*([0-9_]+),\s*\)/g)].map((m) => [m[1], Number(m[2].replaceAll("_", ""))]));
  const mb = (host, ext) => Math.ceil(
    [["rustc", host], ["cargo", host], ["rust-std", host], ["rust-std", "wasm32-wasip1"]]
      .map(([part, target]) => sizes.get(`${part}-${version}-${target}.${ext}`))
      .reduce((a, b) => a + b, 0) / 1_000_000,
  );
  assert.equal(DESKTOP_SYSTEMS.mac.builder, `about ${mb("aarch64-apple-darwin", "tar.xz")}-${mb("x86_64-apple-darwin", "tar.xz")} MB`);
  assert.equal(DESKTOP_SYSTEMS.windows.builder, `about ${mb("x86_64-pc-windows-gnullvm", "tar.gz")} MB`);
});

// K-1386: anonymous asks from a Worker share Cloudflare's addresses and
// GitHub refused them, so every download link said "could not be looked
// up". The hub signs the ask with its OAuth app, asks as anyone if that is
// refused, and serves the last good answer when GitHub does not answer.
test("the release lookup is signed, falls back, and keeps the last good answer", async () => {
  const store = new Map();
  globalThis.caches = {
    default: {
      match: async (req) => {
        const hit = store.get(String(req.url));
        return hit ? hit.clone() : undefined;
      },
      put: async (req, res) => {
        store.set(String(req.url), res.clone());
      },
    },
  };
  const asks = [];
  globalThis.fetch = async (input, init) => {
    const url = String((input && input.url) || input);
    if (url === "https://api.github.com/repos/incyashraj/krate/releases/latest") {
      const auth = (init && init.headers && init.headers.authorization) || "";
      asks.push(auth);
      if (githubDown) return new Response("down", { status: 403 });
      if (auth && auth.includes(btoa("Ov23liV2n8Dxi0okyv0F:wrong"))) {
        return new Response("bad credentials", { status: 401 });
      }
      return new Response(JSON.stringify(WITH), { headers: { "content-type": "application/json" } });
    }
    return realFetch(input, init);
  };

  const env = hubEnv();
  env.GITHUB_CLIENT_SECRET = "right";
  latest = WITH;
  let r = await call(env, null, "GET", "/claude/krate-mac.mcpb");
  assert.equal(r.status, 302);
  assert.equal(r.headers.get("location"), MAC);
  assert.ok(asks[0].startsWith("Basic "), "the ask is signed with the hub's app");

  // GitHub refuses now; the ten-minute copy is gone, the month-long one is not.
  for (const k of [...store.keys()]) if (!k.includes("/__last-good")) store.delete(k);
  githubDown = true;
  r = await call(env, null, "GET", "/claude/krate-windows.mcpb");
  assert.equal(r.status, 302, "the last good answer serves while GitHub is down");
  assert.equal(r.headers.get("location"), WIN);

  // A secret GitHub no longer takes: the hub asks again as anyone.
  store.clear();
  githubDown = false;
  asks.length = 0;
  env.GITHUB_CLIENT_SECRET = "wrong";
  r = await call(env, null, "GET", "/claude/krate-mac.mcpb");
  assert.equal(r.status, 302);
  assert.deepEqual(asks.map((a) => (a ? "signed" : "anyone")), ["signed", "anyone"]);
});
