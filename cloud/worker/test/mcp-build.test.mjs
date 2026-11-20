/* Krate in Claude, phase 2 (IC-989): CLAUDE WRITES, KRATE BUILDS.
 *
 * The OAuth sign-in a connector does, end to end through the real worker --
 * metadata, registration, the consent page, the hub's own GitHub sign-in
 * (GitHub stubbed), the yes, the code with PKCE, tokens and refresh -- and
 * the three building tools against a stand-in for the build service
 * (fetch to its address intercepted). The finished file is a real bundle,
 * so publishing goes through the real validator, signing and admission.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/mcp-build.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash, randomBytes } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";
import { __forgetGuide } from "../src/mcp-build.js";

const here = dirname(fileURLToPath(import.meta.url));
const FINISHED = new Uint8Array(readFileSync(join(here, "fixtures", "notes-0.1.0.krate")));
const BASE = "https://hub.example";
const BUILDER = "https://builder.example";
const SECRET = "test-builder-secret";

/* ---- GitHub's token exchange and the build service, stood in for ------- */
let nextLogin = "alice";
const builder = { jobs: new Map(), calls: [], guide: "# Krate Mode\nWrite Cargo.toml, src/lib.rs and manifest.toml." };
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  const headers = new Headers((input && input.headers) || init.headers || {});
  const reply = (v, status = 200) => new Response(typeof v === "string" ? v : JSON.stringify(v), { status, headers: { "content-type": "application/json" } });
  if (url === "https://github.com/login/oauth/access_token") return reply({ access_token: `gho_${nextLogin}` });
  if (url.startsWith(BUILDER)) {
    const path = url.slice(BUILDER.length);
    const account = headers.get("x-krate-account");
    builder.calls.push({ path, account, method: init.method || "GET" });
    if (headers.get("x-krate-builder") !== SECRET) return new Response("not found", { status: 404 });
    if (path === "/guide") return new Response(builder.guide, { headers: { "content-type": "text/markdown" } });
    if (path === "/compile") {
      const body = JSON.parse(init.body);
      const id = randomBytes(16).toString("hex");
      builder.jobs.set(id, { account, state: "working", body, check: null });
      return reply({ id }, 202);
    }
    const m = /^\/build\/([0-9a-f]{32})(\/file)?$/.exec(path);
    if (m) {
      const job = builder.jobs.get(m[1]);
      if (!job || job.account !== account) return new Response("no such build", { status: 404 });
      if (m[2]) return job.state === "done" ? new Response(FINISHED) : new Response("not ready", { status: 404 });
      return reply({ state: job.state, stage: "test", line: "checking", kind: "compile", check: job.check, error: job.state === "failed" ? "check-app stopped" : null });
    }
    return new Response("no", { status: 404 });
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

const b64url = (buf) => Buffer.from(buf).toString("base64url");
const pkce = () => {
  const verifier = b64url(randomBytes(48));
  return { verifier, challenge: b64url(createHash("sha256").update(verifier).digest()) };
};
const REDIRECT = "https://claude.ai/api/mcp/auth_callback";

/* ---- the client's side of a sign-in ------------------------------------- */
async function register(env, extra = {}) {
  const r = await call(env, null, "POST", "/oauth/register", { json: { client_name: "Claude", redirect_uris: [REDIRECT], ...extra } });
  assert.equal(r.status, 201, JSON.stringify(r.body));
  return r.body;
}
const form = (env, path, params, headers = {}) =>
  call(env, null, "POST", path, {
    bytes: new TextEncoder().encode(new URLSearchParams(params).toString()),
    headers: { "content-type": "application/x-www-form-urlencoded", ...headers },
  });

function authorizeUrl(client, { challenge, redirect = REDIRECT, state = "st-1", method = "S256", resource = `${BASE}/mcp` } = {}) {
  const q = new URLSearchParams({ response_type: "code", client_id: client.client_id, redirect_uri: redirect, state, code_challenge: challenge, code_challenge_method: method, scope: "krate.apps", resource });
  return `/oauth/authorize?${q}`;
}

/* The person: the hub's own GitHub sign-in, started from the consent page,
 * lands on /oauth/authorize/done with the session and the page's nonce. */
async function hubSignIn(env, login) {
  nextLogin = login;
  const nonce = randomBytes(16).toString("hex");
  const start = await call(env, null, "GET", `/login/start?from=oauth&nonce=${nonce}`);
  assert.equal(start.status, 302);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  const back = await call(env, null, "GET", `/login/callback?state=${state}&code=abc`);
  assert.equal(back.status, 302);
  const where = back.headers.get("location");
  assert.ok(where.startsWith("/oauth/authorize/done#"), `lands on the consent page, got ${where}`);
  const fields = new URLSearchParams(where.split("#")[1]);
  assert.equal(fields.get("nonce"), nonce, "with the page's own nonce");
  return fields.get("token");
}

/* The whole dance; answers the client, the tokens and the PKCE pair. */
async function connect(env, login, client = null) {
  client = client || (await register(env));
  const p = pkce();
  const page = await call(env, null, "GET", authorizeUrl(client, { challenge: p.challenge }));
  assert.equal(page.status, 200);
  assert.match(page.body, /build apps you ask for and publish them to your account/);
  const req = JSON.parse(/const REQ=("[^"]+")/.exec(page.body)[1]);
  const session = await hubSignIn(env, login);
  const yes = await call(env, null, "POST", "/oauth/approve", { json: { req, approve: true }, headers: { authorization: `Bearer ${session}` } });
  assert.equal(yes.status, 200, JSON.stringify(yes.body));
  const back = new URL(yes.body.redirect);
  assert.equal(back.origin + back.pathname, REDIRECT);
  assert.equal(back.searchParams.get("state"), "st-1");
  assert.equal(back.searchParams.get("iss"), BASE);
  const code = back.searchParams.get("code");
  const tok = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: client.client_id, code_verifier: p.verifier });
  assert.equal(tok.status, 200, JSON.stringify(tok.body));
  return { client, code, verifier: p.verifier, ...tok.body };
}

let nextId = 1;
async function rpc(env, method, params, token) {
  return call(env, null, "POST", "/mcp", {
    json: { jsonrpc: "2.0", id: nextId++, method, params },
    headers: { accept: "application/json, text/event-stream", ...(token ? { authorization: `Bearer ${token}` } : {}) },
  });
}
const tool = (env, name, args, token) => rpc(env, "tools/call", { name, arguments: args }, token);

const FILES = [
  { path: "Cargo.toml", content: '[package]\nname = "notes"\nversion = "0.1.0"\n' },
  { path: "src/lib.rs", content: "// a notes app -- written by the model\n" },
  { path: "manifest.toml", content: '[app]\nid = "notes"\n' },
];

// --------------------------------------------------------------------------

test("the metadata says where to sign in and what the resource is", async () => {
  const env = hubEnv();
  const pr = await call(env, null, "GET", "/.well-known/oauth-protected-resource/mcp");
  assert.equal(pr.status, 200);
  assert.equal(pr.body.resource, `${BASE}/mcp`);
  assert.deepEqual(pr.body.authorization_servers, [BASE]);
  assert.deepEqual((await call(env, null, "GET", "/.well-known/oauth-protected-resource")).body, pr.body);
  const as = await call(env, null, "GET", "/.well-known/oauth-authorization-server");
  assert.equal(as.body.issuer, BASE);
  assert.deepEqual(as.body.code_challenge_methods_supported, ["S256"]);
  for (const k of ["authorization_endpoint", "token_endpoint", "registration_endpoint", "revocation_endpoint"]) assert.ok(as.body[k].startsWith(BASE), k);
});

test("a write tool without a sign-in is a 401 that points at the metadata; reading needs none", async () => {
  const env = hubEnv();
  const r = await tool(env, "build_app", { name: "Notes", files: FILES });
  assert.equal(r.status, 401);
  assert.match(r.headers.get("www-authenticate"), /^Bearer resource_metadata="https:\/\/hub\.example\/\.well-known\/oauth-protected-resource\/mcp"/);
  assert.match(r.headers.get("access-control-expose-headers"), /www-authenticate/);
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) })).status, 401);
  assert.equal(builder.calls.filter((c) => c.path === "/compile").length, 0, "nothing reached the builder");

  __forgetGuide();
  const guide = await tool(env, "get_krate_guide", {});
  assert.equal(guide.status, 200);
  assert.match(guide.body.result.content[0].text, /call build_app[\s\S]*# Krate Mode/);
  assert.equal(guide.body.result.structuredContent.next, "build_app");
  assert.equal((await tool(env, "search_apps", { query: "x" })).status, 200);

  // A token that is not this server's live access token is refused outright.
  const bad = await tool(env, "search_apps", { query: "x" }, "kmcpa_nonsense");
  assert.equal(bad.status, 401);
  assert.match(bad.headers.get("www-authenticate"), /error="invalid_token"/);
});

test("sign in, build, read check-app's words, build again, and get a link that opens anywhere", async () => {
  const env = hubEnv();
  const { access_token: token } = await connect(env, "alice");

  // A failing build: check-app's own stage, words and fix come back.
  const first = await tool(env, "build_app", { name: "Notes", files: FILES }, token);
  assert.equal(first.status, 200, JSON.stringify(first.body));
  const s1 = first.body.result.structuredContent;
  assert.equal(s1.status, "building");
  const job1 = builder.jobs.get(s1.build_id);
  assert.match(job1.account, /^hub:(github|account):/, "the builder is told whose job it is");
  assert.equal(Buffer.from(job1.body.files.find((f) => f.path === "src/lib.rs").data, "base64").toString(), FILES[1].content);
  assert.equal(job1.body.name, "Notes");

  let st = await tool(env, "build_status", { build_id: s1.build_id }, token);
  assert.equal(st.body.result.structuredContent.status, "building");
  job1.state = "failed";
  job1.check = { stage: "build", kind: "compile", detail: "error[E0425]: cannot find value `x`", fix: "Declare x before using it." };
  st = await tool(env, "build_status", { build_id: s1.build_id }, token);
  const f = st.body.result.structuredContent;
  assert.deepEqual([f.status, f.stage, f.fix], ["failed", "build", "Declare x before using it."]);
  assert.match(st.body.result.content[0].text, /E0425[\s\S]*Declare x[\s\S]*call build_app again/);

  // The fixed build passes and is published, unlisted, to alice's account.
  const second = await tool(env, "build_app", { name: "Notes", files: FILES }, token);
  const id2 = second.body.result.structuredContent.build_id;
  builder.jobs.get(id2).state = "done";
  const done = await tool(env, "build_status", { build_id: id2 }, token);
  const p = done.body.result.structuredContent;
  assert.equal(p.status, "published", JSON.stringify(done.body));
  assert.match(p.link, /^https:\/\/hub\.example\/a\/v[0-9a-f]{11}$/);
  assert.equal(p.browser_link, `${BASE}/run/${p.link.split("/a/")[1]}`, "the browser link opens the app's own link");
  const row = JSON.parse(env._kv.get(`app:${p.app_id}`));
  assert.equal(row.unlisted, true, "published unlisted");
  assert.equal(row.signature.state, "valid", "signed as the account");
  assert.match(row.owner, /^(github:alice|account:)/);
  assert.equal(row.name, "Notes");
  assert.deepEqual((await tool(env, "search_apps", { query: "notes" })).body.result.structuredContent.results, [], "not in the gallery");

  // Asking again answers the same app; nothing is published twice.
  const again = await tool(env, "build_status", { build_id: id2 }, token);
  assert.equal(again.body.result.structuredContent.app_id, p.app_id);
  assert.equal([...env._kv.keys()].filter((k) => k.startsWith("app:")).length, 1);
});

test("one account's token cannot see another account's build", async () => {
  const env = hubEnv();
  const alice = await connect(env, "alice");
  const bob = await connect(env, "bob");
  const started = await tool(env, "build_app", { name: "Notes", files: FILES }, alice.access_token);
  const id = started.body.result.structuredContent.build_id;
  builder.jobs.get(id).state = "done";
  const asked = builder.calls.length;
  const asBob = await tool(env, "build_status", { build_id: id }, bob.access_token);
  assert.equal(asBob.body.result.isError, true);
  assert.match(asBob.body.result.content[0].text, /No build of this Krate account has that id/);
  assert.equal(builder.calls.length, asked, "the hub refuses it without asking the builder");
  assert.equal(asBob.body.result.structuredContent.status, "unknown");
  assert.ok(!JSON.stringify(asBob.body).includes("Notes"), "nothing about it");
  assert.equal([...env._kv.keys()].filter((k) => k.startsWith("app:")).length, 0, "bob published nothing of alice's");
});

test("the sign-in refuses a bad verifier, a wrong redirect, a stranger's client, and a code used twice", async () => {
  const env = hubEnv();
  const client = await register(env);

  // A redirect the client did not register: a page, never a redirect.
  const p = pkce();
  const wrong = await call(env, null, "GET", authorizeUrl(client, { challenge: p.challenge, redirect: "https://evil.example/cb" }));
  assert.equal(wrong.status, 400);
  assert.equal(wrong.headers.get("location"), null);
  const forged = await call(env, null, "GET", authorizeUrl({ client_id: "kmcpc_forged" }, { challenge: p.challenge }));
  assert.equal(forged.status, 400);
  // No PKCE, or plain PKCE: back to the client with the error.
  const plain = await call(env, null, "GET", authorizeUrl(client, { challenge: p.challenge, method: "plain" }));
  assert.equal(plain.status, 302);
  assert.equal(new URL(plain.headers.get("location")).searchParams.get("error"), "invalid_request");
  assert.equal(
    (await register(env, { redirect_uris: ["http://evil.example/cb"] }).catch(() => null)),
    null,
    "a plain-http redirect that is not this machine is refused",
  );

  // A wrong verifier.
  const page = await call(env, null, "GET", authorizeUrl(client, { challenge: p.challenge }));
  const req = JSON.parse(/const REQ=("[^"]+")/.exec(page.body)[1]);
  const session = await hubSignIn(env, "alice");
  const yes = await call(env, null, "POST", "/oauth/approve", { json: { req, approve: true }, headers: { authorization: `Bearer ${session}` } });
  const code = new URL(yes.body.redirect).searchParams.get("code");
  const badPkce = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: client.client_id, code_verifier: pkce().verifier });
  assert.equal(badPkce.status, 400);
  assert.equal(badPkce.body.error, "invalid_grant");
  // Another client presenting the code.
  const other = await register(env);
  const stolen = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: other.client_id, code_verifier: p.verifier });
  assert.equal(stolen.body.error, "invalid_grant");
  // The right one works once; the second use ends the grant.
  const ok = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: client.client_id, code_verifier: p.verifier });
  assert.equal(ok.status, 200);
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) }, ok.body.access_token)).status, 200);
  const twice = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: client.client_id, code_verifier: p.verifier });
  assert.equal(twice.body.error, "invalid_grant");
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) }, ok.body.access_token)).status, 401, "a replayed code ends the grant");

  // No.
  const no = await call(env, null, "POST", "/oauth/approve", { json: { req, approve: false }, headers: { authorization: `Bearer ${session}` } });
  assert.equal(new URL(no.body.redirect).searchParams.get("error"), "access_denied");
  // Yes without a session.
  assert.equal((await call(env, null, "POST", "/oauth/approve", { json: { req, approve: true } })).status, 401);

  // A client that registered a secret must present it.
  const confidential = await register(env, { token_endpoint_auth_method: "client_secret_post" });
  assert.ok(confidential.client_secret);
  const noSecret = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: "x", client_id: confidential.client_id });
  assert.equal(noSecret.status, 401);
  assert.equal(noSecret.body.error, "invalid_client");
});

test("refresh tokens rotate; an old one used again ends the grant; revoking ends it too", async () => {
  const env = hubEnv();
  const first = await connect(env, "alice");
  const fresh = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: first.refresh_token, client_id: first.client.client_id });
  assert.equal(fresh.status, 200, JSON.stringify(fresh.body));
  assert.notEqual(fresh.body.access_token, first.access_token);
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) }, fresh.body.access_token)).status, 200);
  const replay = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: first.refresh_token, client_id: first.client.client_id });
  assert.equal(replay.body.error, "invalid_grant");
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) }, fresh.body.access_token)).status, 401, "both holders lose it");

  const second = await connect(env, "alice");
  const revoked = await form(env, "/oauth/revoke", { token: second.access_token, client_id: second.client.client_id });
  assert.equal(revoked.status, 200);
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) }, second.access_token)).status, 401);
});

test("the account sees the connectors it allowed and can take one back", async () => {
  const env = hubEnv();
  const c = await connect(env, "alice");
  const session = await hubSignIn(env, "alice");
  const listed = await call(env, null, "GET", "/me/connectors", { headers: { authorization: `Bearer ${session}` } });
  assert.equal(listed.status, 200, JSON.stringify(listed.body));
  assert.equal(listed.body.connectors.length, 1);
  assert.equal(listed.body.connectors[0].client, "Claude");
  const id = listed.body.connectors[0].id;
  const bobSession = await hubSignIn(env, "bob");
  assert.equal((await call(env, null, "DELETE", `/me/connectors/${id}`, { headers: { authorization: `Bearer ${bobSession}` } })).status, 404, "not bob's to take back");
  assert.equal((await call(env, null, "DELETE", `/me/connectors/${id}`, { headers: { authorization: `Bearer ${session}` } })).status, 200);
  assert.equal((await tool(env, "build_status", { build_id: "0".repeat(32) }, c.access_token)).status, 401);
});

test("an account's builds are limited per hour, and a refused build reaches no builder", async () => {
  const env = hubEnv({ MCP_BUILDS_PER_HOUR: "2" });
  const { access_token: token } = await connect(env, "alice");
  const compiles = () => builder.calls.filter((c) => c.path === "/compile").length;
  for (let i = 0; i < 2; i++) {
    const r = await tool(env, "build_app", { name: "Notes", files: FILES }, token);
    assert.equal(r.body.result.structuredContent.status, "building");
  }
  const before = compiles();
  const third = await tool(env, "build_app", { name: "Notes", files: FILES }, token);
  assert.equal(third.body.result.isError, true);
  assert.match(third.body.result.content[0].text, /2 builds an hour/);
  assert.equal(compiles(), before, "the builder was not asked");
});

test("build_app refuses files that are not an app's source", async () => {
  const env = hubEnv();
  const { access_token: token } = await connect(env, "alice");
  const bad = async (files) => (await tool(env, "build_app", { name: "x", files }, token)).body.error?.code;
  assert.equal(await bad([{ path: "../etc/passwd", content: "" }, ...FILES]), -32602);
  assert.equal(await bad(FILES.slice(1)), -32602, "Cargo.toml is needed");
  assert.equal(await bad([...FILES, FILES[0]]), -32602, "a path twice");
  assert.equal(await bad([...FILES, { path: "a.png", content: "not base64!", encoding: "base64" }]), -32602);
  assert.equal(await bad([...FILES, { path: "big.txt", content: "x".repeat(2 * 1024 * 1024 + 1) }]), -32602);
});
