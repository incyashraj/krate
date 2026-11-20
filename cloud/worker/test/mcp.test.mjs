/* Krate as a remote MCP server (IC-989, phase 1): POST /mcp, driven
 * through the real worker with real published bundles.
 *
 * What it must be true of: the handshake negotiates a protocol version;
 * every tool is read-only, titled, named within 64 characters and declares
 * both schemas; search is the gallery's own ranking; one app answers by its
 * link or its hash; a taken-down app answers with its notice and nothing
 * else; an unlisted app is never returned; bad calls are JSON-RPC errors;
 * notifications get 202; and the endpoint has its own rate-limit ceiling.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/mcp.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const NOTES = new Uint8Array(readFileSync(join(here, "fixtures", "notes-0.1.0-signed-a.krate")));
const NOTES_2 = new Uint8Array(readFileSync(join(here, "fixtures", "notes-0.2.0-signed-a.krate")));
const BOUNCE = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));

const hubEnv = (extra = {}) => {
  const env = orgEnv({ PUBLIC_BASE: "https://hub.example", KRATE_ADMINS: "root", ...extra });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

const publish = async (env, who, bytes, headers) => {
  const r = await call(env, who, "POST", "/publish", { bytes, headers });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  return r.body;
};

let nextId = 1;
/* One JSON-RPC message to /mcp. */
async function rpc(env, method, params, { headers = {}, id = nextId++, notify = false } = {}) {
  const message = { jsonrpc: "2.0", method, ...(params === undefined ? {} : { params }), ...(notify ? {} : { id }) };
  return call(env, null, "POST", "/mcp", {
    json: message,
    headers: { accept: "application/json, text/event-stream", "cf-connecting-ip": "192.0.2.1", ...headers },
  });
}
const tool = (env, name, args) => rpc(env, "tools/call", { name, arguments: args });

/* A listing written straight to KV, as the gallery tests do, for ranking. */
const hex = (n) => n.toString(16).repeat(64).slice(0, 64);
const row = (env, n, fields) =>
  env.APPS.put(`app:${hex(n)}`, JSON.stringify({ published: 1_790_000_000 + n, category: "apps", author: "someone", size: 1000, ...fields }));

test("initialize negotiates the protocol version and names the server", async () => {
  const env = hubEnv();
  const asked = await rpc(env, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "t", version: "1" } });
  assert.equal(asked.status, 200);
  assert.equal(asked.headers.get("content-type"), "application/json");
  assert.equal(asked.headers.get("access-control-allow-origin"), "*");
  const r = asked.body.result;
  assert.equal(r.protocolVersion, "2025-06-18", "a version the server speaks is answered with itself");
  assert.equal(r.serverInfo.name, "krate");
  assert.ok(r.serverInfo.version);
  assert.deepEqual(r.capabilities, { tools: { listChanged: false } });
  assert.equal(asked.headers.get("mcp-session-id"), null, "stateless: no session is issued");

  const unknown = await rpc(env, "initialize", { protocolVersion: "1999-01-01", capabilities: {} });
  assert.equal(unknown.body.result.protocolVersion, "2025-11-25", "an unknown version gets the newest this server speaks");
  const older = await rpc(env, "initialize", { protocolVersion: "2025-03-26", capabilities: {} });
  assert.equal(older.body.result.protocolVersion, "2025-03-26");

  // A request under an unsupported negotiated version is refused, as the
  // streamable HTTP transport says; a session id is accepted and ignored.
  const bad = await rpc(env, "ping", undefined, { headers: { "mcp-protocol-version": "1999-01-01" } });
  assert.equal(bad.status, 400);
  const ping = await rpc(env, "ping", undefined, { headers: { "mcp-protocol-version": "2025-06-18", "mcp-session-id": "anything" } });
  assert.equal(ping.status, 200);
  assert.deepEqual(ping.body.result, {});
});

test("notifications get 202 and no body; GET is 405; a batch is refused", async () => {
  const env = hubEnv();
  const n = await rpc(env, "notifications/initialized", undefined, { notify: true });
  assert.equal(n.status, 202);
  assert.equal(n.bytes.length, 0);
  const get = await call(env, null, "GET", "/mcp");
  assert.equal(get.status, 405);
  assert.match(get.headers.get("allow"), /POST/);
  const pre = await call(env, null, "OPTIONS", "/mcp");
  assert.equal(pre.status, 204);
  assert.match(pre.headers.get("access-control-allow-headers"), /mcp-protocol-version/);
  const batch = await call(env, null, "POST", "/mcp", { json: [{ jsonrpc: "2.0", id: 1, method: "ping" }] });
  assert.equal(batch.status, 400);
  assert.equal(batch.body.error.code, -32600);
  const junk = await call(env, null, "POST", "/mcp", { bytes: new TextEncoder().encode("{not json"), headers: { "content-type": "application/json" } });
  assert.equal(junk.status, 400);
  assert.equal(junk.body.error.code, -32700);
});

test("tools/list: every tool is titled, named within 64 characters, with both schemas; only the build tools write", async () => {
  const env = hubEnv();
  const r = await rpc(env, "tools/list", {});
  const tools = r.body.result.tools;
  assert.deepEqual(tools.map((t) => t.name).sort(), ["build_app", "build_status", "get_app", "get_krate_guide", "list_featured", "search_apps"]);
  const WRITE = new Set(["build_app", "build_status"]);
  for (const t of tools) {
    assert.ok(t.name.length <= 64 && /^[a-z_]+$/.test(t.name), t.name);
    assert.ok(typeof t.title === "string" && t.title.length > 0, `${t.name} has a title`);
    // The directory's submission check reads the listing name from here.
    assert.equal(t.annotations.title, t.title, `${t.name} carries its title in annotations`);
    assert.ok(t.description.length > 40, `${t.name} says what it does`);
    assert.equal(t.annotations.openWorldHint, false);
    assert.equal(t.inputSchema.type, "object");
    assert.equal(t.outputSchema.type, "object");
    if (WRITE.has(t.name)) {
      assert.equal(t.annotations.readOnlyHint, false, `${t.name} writes`);
      assert.equal(t.annotations.destructiveHint, false, `${t.name} removes nothing`);
      assert.equal(t.annotations.idempotentHint, false);
      assert.match(t.description, /sign in/, `${t.name} says it needs a sign-in`);
    } else {
      assert.equal(t.annotations.readOnlyHint, true, `${t.name} is read-only`);
    }
    assert.ok(!/publish_|make_|delete|create_ai|generate/.test(t.name), "no tool starts an AI build, deletes or publishes directly");
  }
});

test("search_apps ranks a name match above a description match, and keeps the gallery's rules", async () => {
  const env = hubEnv();
  const notes = await publish(env, "alice", NOTES, { "x-krate-name": "Notes" });
  await row(env, 1, { name: "Scratchpad", description: "quick notes for the day" });
  await row(env, 2, { name: "Hidden Notes", description: "", unlisted: true });
  await row(env, 3, { name: "Old Notes", description: "", superseded: true });

  const r = await tool(env, "search_apps", { query: "notes" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const res = r.body.result;
  assert.equal(res.isError, undefined);
  const names = res.structuredContent.results.map((a) => a.name);
  assert.deepEqual(names, ["Notes", "Scratchpad"], "name match first; unlisted and superseded rows never");
  const top = res.structuredContent.results[0];
  assert.equal(top.id, notes.id);
  assert.equal(top.link, notes.url, "the app's own link");
  assert.equal(top.browser_link, `https://hub.example/run/${notes.line}`);
  assert.equal(top.verified_publisher, true);
  assert.equal(top.signed, true);
  assert.equal(top.size_bytes, NOTES.length);
  assert.ok(top.permissions.some((p) => p.id === "store.sql" && p.words === "keep its own database" && p.why), "the validator's plain words, and the maker's reason");
  assert.ok(!JSON.stringify(res).includes("github:alice"), "the owner principal never leaves");
  assert.match(res.content[0].text, /^2 Krate apps for "notes"/);
  assert.match(res.content[0].text, /keep its own database/);

  const limited = await tool(env, "search_apps", { query: "notes", limit: 1 });
  assert.equal(limited.body.result.structuredContent.results.length, 1);
  const none = await tool(env, "search_apps", { query: "notes", category: "games" });
  assert.deepEqual(none.body.result.structuredContent.results, []);
  assert.match(none.body.result.content[0].text, /No public Krate app/);
});

test("get_app answers by the app's link, by its hash, and by a /run/ link; never an unlisted app", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", NOTES, { "x-krate-name": "Notes" });
  const v2 = await publish(env, "alice", NOTES_2, { "x-krate-name": "Notes" });

  const byLink = await tool(env, "get_app", { id_or_link: v1.url });
  const app = byLink.body.result.structuredContent.app;
  assert.equal(byLink.body.result.structuredContent.found, true);
  assert.equal(app.id, v2.id, "the app's link is its current version");
  assert.equal(app.name, "Notes");
  assert.deepEqual(app.versions, { count: 2, current: 2, this_version: 2, newer_exists: false });
  assert.match(app.published, /^\d{4}-\d\d-\d\dT/);
  assert.equal(app.link, v1.url);

  const byHash = await tool(env, "get_app", { id_or_link: v1.id });
  const old = byHash.body.result.structuredContent.app;
  assert.equal(old.id, v1.id);
  assert.equal(old.signed, true);
  assert.deepEqual(old.versions, { count: 2, current: 2, this_version: 1, newer_exists: true });
  assert.match(byHash.body.result.content[0].text, /version 1 of 2; the app's link opens version 2/);

  const byRun = await tool(env, "get_app", { id_or_link: `https://hub.example/run/${v1.line}` });
  assert.equal(byRun.body.result.structuredContent.app.id, v2.id);

  const unlisted = await publish(env, "bob", BOUNCE, { "x-krate-name": "Bounce", "x-krate-unlisted": "1" });
  const hidden = await tool(env, "get_app", { id_or_link: unlisted.id });
  assert.equal(hidden.body.result.isError, true);
  assert.equal(hidden.body.result.structuredContent.found, false);
  assert.ok(!JSON.stringify(hidden.body).includes("Bounce"), "nothing about an unlisted app");

  const nonsense = await tool(env, "get_app", { id_or_link: "not a link" });
  assert.equal(nonsense.body.result.isError, true);
  assert.match(nonsense.body.result.content[0].text, /not a Krate app link/);
});

test("a taken-down app answers removed with its notice, and leaves search and the shelves", async () => {
  const env = hubEnv();
  const bounce = await publish(env, "bob", BOUNCE, { "x-krate-name": "Bounce", "x-krate-description": "a ball that bounces" });
  assert.equal((await tool(env, "search_apps", { query: "bounce" })).body.result.structuredContent.results.length, 1);

  const td = await call(env, "root", "POST", `/admin/takedown/${bounce.id}`, { json: { reason: "impersonates a bank" } });
  assert.equal(td.status, 200, JSON.stringify(td.body));

  const got = await tool(env, "get_app", { id_or_link: bounce.url });
  const s = got.body.result.structuredContent;
  assert.deepEqual(s, { found: false, removed: true, notice: `https://hub.example/takedown/${bounce.id}` });
  assert.ok(!JSON.stringify(got.body).includes("Bounce"), "a removed app's details are never given");
  assert.deepEqual((await tool(env, "search_apps", { query: "bounce" })).body.result.structuredContent.results, []);
  const featured = (await tool(env, "list_featured", {})).body.result.structuredContent;
  assert.ok(![...featured.start_here, ...featured.newest].some((a) => a.id === bounce.id));
});

test("list_featured is the Start here and newest shelves", async () => {
  const env = hubEnv({ GALLERY_PICKS: hex(2).slice(0, 12) });
  await row(env, 1, { name: "Pad", description: "" });
  await row(env, 2, { name: "Markdown Editor", description: "write markdown" });
  const r = await tool(env, "list_featured", {});
  const s = r.body.result.structuredContent;
  assert.deepEqual(s.start_here.map((a) => a.name), ["Markdown Editor"]);
  assert.deepEqual(s.newest.map((a) => a.name), ["Markdown Editor", "Pad"]);
  assert.equal(s.start_here[0].permissions, null, "no recorded permissions reads as not recorded, not as none");
  assert.match(r.body.result.content[0].text, /Start here:/);
});

test("unknown methods, unknown tools and bad arguments are JSON-RPC errors", async () => {
  const env = hubEnv();
  assert.equal((await rpc(env, "resources/list", {})).body.error.code, -32601);
  assert.equal((await tool(env, "publish_app", {})).body.error.code, -32602);
  assert.equal((await tool(env, "search_apps", {})).body.error.code, -32602, "query is required");
  assert.equal((await tool(env, "search_apps", { query: "x", limit: 21 })).body.error.code, -32602, "limit is at most 20");
  assert.equal((await tool(env, "search_apps", { query: "x", category: "casino" })).body.error.code, -32602);
  assert.equal((await tool(env, "search_apps", { query: "x", extra: 1 })).body.error.code, -32602);
  assert.equal((await tool(env, "get_app", { id_or_link: 7 })).body.error.code, -32602);
});

test("/mcp has its own rate-limit ceiling, per caller", async () => {
  const env = hubEnv();
  let refused = null;
  for (let i = 1; i <= 200 && !refused; i++) {
    const r = await rpc(env, "ping", undefined, { headers: { "cf-connecting-ip": "203.0.113.50" } });
    if (r.status === 429) refused = i;
    else assert.equal(r.status, 200);
  }
  assert.equal(refused, 121, "120 a minute, then 429");
  const other = await rpc(env, "ping", undefined, { headers: { "cf-connecting-ip": "203.0.113.51" } });
  assert.equal(other.status, 200, "another caller is not refused");
  const over = await rpc(env, "ping", undefined, { headers: { "cf-connecting-ip": "203.0.113.50" } });
  assert.equal(over.headers.get("access-control-allow-origin"), "*");
  assert.ok(over.headers.get("retry-after"));
});

