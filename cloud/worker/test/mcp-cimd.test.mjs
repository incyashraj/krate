/* Krate in Claude: sign-in by a published identity (OAuth Client ID
 * Metadata Documents, which MCP 2025-11-25 recommends and claude.ai offers
 * as "Use Claude's published identity").
 *
 * The client_id is an https address; the hub fetches the JSON there (fetch
 * to the client's host intercepted) and trusts it only when its client_id
 * is that address byte for byte. Driven through the real worker: metadata,
 * the consent page, the hub's own GitHub sign-in (GitHub stubbed), the yes,
 * the code with PKCE, refresh and revoke -- and every refusal, which must be
 * a page (never a redirect) at /oauth/authorize and OAuth error JSON at
 * /oauth/token. Registration (RFC 7591) keeps working beside it.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/mcp-cimd.test.mjs
 */
import { test, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { orgEnv, call } from "./org-tools.mjs";
import { __forgetClientDocuments } from "../src/mcp-oauth.js";

const BASE = "https://hub.example";
const CLIENT_HOST = "https://client.example";
const CLIENT_ID = `${CLIENT_HOST}/oauth/mcp-client-metadata`;
const REDIRECT = "https://client.example/api/mcp/auth_callback";

/* ---- GitHub's token exchange and the client's own host, stood in for ---- */
let nextLogin = "alice";
/** address -> () => Response, the client host's answers */
const served = new Map();
/** every address the hub fetched on the client's host */
const fetched = [];
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  if (url === "https://github.com/login/oauth/access_token") {
    return new Response(JSON.stringify({ access_token: `gho_${nextLogin}` }), { headers: { "content-type": "application/json" } });
  }
  const u = new URL(url);
  if (u.hostname.endsWith(".example") && u.hostname !== "hub.example") {
    fetched.push({ url, redirect: init.redirect, hasSignal: !!init.signal });
    const answer = served.get(url);
    return answer ? answer() : new Response("not found", { status: 404 });
  }
  return before(input, init);
};

const json = (doc, headers = {}) => () =>
  new Response(typeof doc === "string" ? doc : JSON.stringify(doc), { headers: { "content-type": "application/json", ...headers } });
const goodDoc = (extra = {}) => ({
  client_id: CLIENT_ID,
  client_name: "Claude",
  client_uri: "https://client.example",
  logo_uri: "https://client.example/logo.png",
  redirect_uris: [REDIRECT],
  grant_types: ["authorization_code", "refresh_token"],
  response_types: ["code"],
  token_endpoint_auth_method: "none",
  ...extra,
});

beforeEach(() => {
  __forgetClientDocuments();
  served.clear();
  fetched.length = 0;
});

const hubEnv = () => {
  const env = orgEnv({ PUBLIC_BASE: BASE, KEY_WRAP_SECRET: "test-wrapping-secret", GITHUB_CLIENT_SECRET: "gh-secret" });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

const b64url = (buf) => Buffer.from(buf).toString("base64url");
const pkce = () => {
  const verifier = b64url(randomBytes(48));
  return { verifier, challenge: b64url(createHash("sha256").update(verifier).digest()) };
};
const form = (env, path, params) =>
  call(env, null, "POST", path, {
    bytes: new TextEncoder().encode(new URLSearchParams(params).toString()),
    headers: { "content-type": "application/x-www-form-urlencoded" },
  });
function authorizeUrl(clientId, { challenge, redirect = REDIRECT, state = "st-1" } = {}) {
  const q = new URLSearchParams({
    response_type: "code", client_id: clientId, redirect_uri: redirect, state,
    code_challenge: challenge, code_challenge_method: "S256", scope: "krate.apps", resource: `${BASE}/mcp`,
  });
  return `/oauth/authorize?${q}`;
}

async function hubSignIn(env, login) {
  nextLogin = login;
  const nonce = randomBytes(16).toString("hex");
  const start = await call(env, null, "GET", `/login/start?from=oauth&nonce=${nonce}`);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  const back = await call(env, null, "GET", `/login/callback?state=${state}&code=abc`);
  const where = back.headers.get("location");
  assert.ok(where.startsWith("/oauth/authorize/done#"), `lands on the consent page, got ${where}`);
  return new URLSearchParams(where.split("#")[1]).get("token");
}

/* The consent page, the sign-in and the yes: answers the code and the verifier. */
async function codeFor(env, clientId, login = "alice") {
  const p = pkce();
  const page = await call(env, null, "GET", authorizeUrl(clientId, { challenge: p.challenge }));
  assert.equal(page.status, 200, String(page.body));
  const req = JSON.parse(/const REQ=("[^"]+")/.exec(page.body)[1]);
  const session = await hubSignIn(env, login);
  const yes = await call(env, null, "POST", "/oauth/approve", { json: { req, approve: true }, headers: { authorization: `Bearer ${session}` } });
  assert.equal(yes.status, 200, JSON.stringify(yes.body));
  const back = new URL(yes.body.redirect);
  assert.equal(back.origin + back.pathname, REDIRECT);
  assert.equal(back.searchParams.get("state"), "st-1");
  return { page, code: back.searchParams.get("code"), verifier: p.verifier, session };
}

const buildStatus = (env, token) =>
  call(env, null, "POST", "/mcp", {
    json: { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "build_status", arguments: { build_id: "0".repeat(32) } } },
    headers: { accept: "application/json, text/event-stream", authorization: `Bearer ${token}` },
  });

/** A refusal at /oauth/authorize: a 400 page in plain words, never a redirect anywhere. */
async function refusedPage(env, clientId, words, { redirect } = {}) {
  const r = await call(env, null, "GET", authorizeUrl(clientId, { challenge: pkce().challenge, redirect }));
  assert.equal(r.status, 400, `refused: ${clientId}`);
  assert.equal(r.headers.get("location"), null, "never a redirect");
  assert.match(r.headers.get("content-type"), /text\/html/);
  assert.match(r.body, /This sign-in cannot go on/);
  assert.match(r.body, words);
  assert.ok(!/const REQ=/.test(r.body), "no sign-in is started");
  return r;
}

// --------------------------------------------------------------------------

test("the server metadata says client ID metadata documents are supported, and registration is still offered", async () => {
  const env = hubEnv();
  const as = await call(env, null, "GET", "/.well-known/oauth-authorization-server");
  assert.equal(as.status, 200);
  assert.equal(as.body.client_id_metadata_document_supported, true);
  assert.equal(as.body.registration_endpoint, `${BASE}/oauth/register`);
  assert.deepEqual(as.body.code_challenge_methods_supported, ["S256"]);
  assert.ok(as.body.token_endpoint_auth_methods_supported.includes("none"));
});

test("a good document signs in end to end: consent shows its name and host, PKCE code, tokens, refresh, revoke", async () => {
  const env = hubEnv();
  served.set(CLIENT_ID, json(goodDoc()));
  const { page, code, verifier, session } = await codeFor(env, CLIENT_ID);

  // The person sees who is asking: the name, and the host that vouches for it.
  assert.match(page.body, /<b>Claude<\/b>, from <b>client\.example<\/b>, wants to use your Krate account/);
  assert.match(page.body, /<img class="logo" src="https:\/\/client\.example\/logo\.png"/);
  assert.match(page.body, /build apps you ask for and publish them to your account/);
  assert.match(page.headers.get("content-security-policy"), /img-src https:/);
  const who = JSON.parse(/const WHO=(\{[^;]+\});/.exec(page.body)[1]);
  assert.deepEqual(who, { n: "Claude", h: "client.example", back: "client.example", local: false }, "the Allow page shows the same");

  // The fetch itself: no redirects followed, a deadline.
  assert.equal(fetched[0].url, CLIENT_ID);
  assert.equal(fetched[0].redirect, "manual");
  assert.equal(fetched[0].hasSignal, true);

  // A wrong verifier is refused; the right one gets tokens.
  const bad = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: CLIENT_ID, code_verifier: pkce().verifier });
  assert.equal(bad.status, 400);
  assert.equal(bad.body.error, "invalid_grant");
  const tok = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: CLIENT_ID, code_verifier: verifier });
  assert.equal(tok.status, 200, JSON.stringify(tok.body));
  assert.equal(tok.body.token_type, "Bearer");
  assert.equal((await buildStatus(env, tok.body.access_token)).status, 200, "the access token works at /mcp");
  assert.equal(fetched.length, 1, "the document was kept between the consent page and the token");

  // Another client's id cannot spend this code.
  const otherId = `${CLIENT_HOST}/other-client`;
  served.set(otherId, json(goodDoc({ client_id: otherId })));

  // Refresh rotates, with the published identity as the client.
  const fresh = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: tok.body.refresh_token, client_id: CLIENT_ID });
  assert.equal(fresh.status, 200, JSON.stringify(fresh.body));
  const stranger = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: fresh.body.refresh_token, client_id: otherId });
  assert.equal(stranger.body.error, "invalid_grant", "a different published identity cannot use these tokens");

  // The account's list names the host it came from.
  const listed = await call(env, null, "GET", "/me/connectors", { headers: { authorization: `Bearer ${session}` } });
  assert.equal(listed.status, 200, JSON.stringify(listed.body));
  assert.equal(listed.body.connectors[0].client, "Claude");
  assert.equal(listed.body.connectors[0].from, "client.example");

  // Revoke ends it.
  const revoked = await form(env, "/oauth/revoke", { token: fresh.body.access_token, client_id: CLIENT_ID });
  assert.equal(revoked.status, 200);
  assert.equal((await buildStatus(env, fresh.body.access_token)).status, 401);
});

test("a document whose client_id is not its address is refused, at the consent page and at the token endpoint", async () => {
  const env = hubEnv();
  // A copy of someone else's document, served from another address.
  served.set(CLIENT_ID, json(goodDoc({ client_id: "https://claude.ai/oauth/mcp-oauth-client-metadata" })));
  await refusedPage(env, CLIENT_ID, /client_id is not the address it was fetched from/);
  // Not even a trailing slash of difference.
  served.set(CLIENT_ID, json(goodDoc({ client_id: `${CLIENT_ID}/` })));
  await refusedPage(env, CLIENT_ID, /client_id is not the address it was fetched from/);

  const tok = await form(env, "/oauth/token", { grant_type: "authorization_code", code: "kmcpk_x", redirect_uri: REDIRECT, client_id: CLIENT_ID, code_verifier: pkce().verifier });
  assert.equal(tok.status, 401);
  assert.equal(tok.body.error, "invalid_client");
  assert.match(tok.body.error_description, /client_id is not the address it was fetched from/);
  assert.equal(tok.headers.get("content-type"), "application/json");
});

test("a redirect_uri the document does not list is refused with a page, never followed", async () => {
  const env = hubEnv();
  served.set(CLIENT_ID, json(goodDoc()));
  await refusedPage(env, CLIENT_ID, /does not list/, { redirect: "https://evil.example/cb" });
  // Exact match only: a longer path, another port, or a query is a different address.
  await refusedPage(env, CLIENT_ID, /does not list/, { redirect: `${REDIRECT}/x` });
  await refusedPage(env, CLIENT_ID, /does not list/, { redirect: "https://client.example:8443/api/mcp/auth_callback" });
  await refusedPage(env, CLIENT_ID, /does not list/, { redirect: `${REDIRECT}?next=evil` });
});

test("a client_id that is not a plain https address with a path on a public host is refused before anything is fetched", async () => {
  const env = hubEnv();
  for (const [id, words] of [
    ["http://client.example/oauth/meta", /must start with https:\/\//],
    ["https://client.example", /needs a path/],
    ["https://client.example/", /needs a path/],
    ["https://user:pw@client.example/meta", /user name or password/],
    ["https://client.example/meta#frag", /#fragment/],
    ["https://client.example/a/../meta", /plain form/],
    ["https://CLIENT.example/meta", /plain form/],
    ["https://client.example:443/meta", /plain form/],
    ["https://127.0.0.1/meta", /public host name/],
    ["https://[::1]/meta", /public host name/],
    ["https://localhost/meta", /public host name/],
    ["https://metadata.internal/meta", /public host name/],
  ]) {
    await refusedPage(env, id, words);
  }
  assert.equal(fetched.length, 0, "the hub fetched nothing");
  const tok = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: "x", client_id: "http://client.example/oauth/meta" });
  assert.equal(tok.status, 401);
  assert.equal(tok.body.error, "invalid_client");
});

test("an oversized document is refused, whether it says its size or not", async () => {
  const env = hubEnv();
  const big = JSON.stringify(goodDoc({ padding: "x".repeat(6000) }));
  served.set(CLIENT_ID, () => new Response(big, { headers: { "content-type": "application/json", "content-length": String(big.length) } }));
  await refusedPage(env, CLIENT_ID, /larger than 5 KB/);
  // No length given: streamed, and cut off past the limit.
  served.set(CLIENT_ID, () => {
    const chunk = new TextEncoder().encode("x".repeat(1024));
    let sent = 0;
    const body = new ReadableStream({
      pull(c) {
        if (sent++ < 64) c.enqueue(chunk);
        else c.close();
      },
    });
    return new Response(body, { headers: { "content-type": "application/json" } });
  });
  await refusedPage(env, CLIENT_ID, /larger than 5 KB/);
  // Just under the limit is fine.
  const fits = goodDoc({ padding: "" });
  fits.padding = "x".repeat(5 * 1024 - JSON.stringify(fits).length);
  assert.equal(JSON.stringify(fits).length, 5 * 1024);
  served.set(CLIENT_ID, json(fits));
  const ok = await call(env, null, "GET", authorizeUrl(CLIENT_ID, { challenge: pkce().challenge }));
  assert.equal(ok.status, 200);
});

test("a redirect, a non-JSON answer, a secret, another auth method or a missing name is refused, and never kept", async () => {
  const env = hubEnv();
  served.set(CLIENT_ID, () => new Response(null, { status: 302, headers: { location: "https://elsewhere.example/doc" } }));
  await refusedPage(env, CLIENT_ID, /redirects somewhere else/);
  assert.ok(!fetched.some((f) => f.url.startsWith("https://elsewhere.example")), "the redirect was not followed");

  served.set(CLIENT_ID, () => new Response(JSON.stringify(goodDoc()), { headers: { "content-type": "text/html" } }));
  await refusedPage(env, CLIENT_ID, /did not answer with JSON/);
  served.set(CLIENT_ID, json("{not json"));
  await refusedPage(env, CLIENT_ID, /not valid JSON/);
  served.set(CLIENT_ID, json([goodDoc()]));
  await refusedPage(env, CLIENT_ID, /not a JSON object/);
  served.set(CLIENT_ID, json(goodDoc({ client_secret: "s3cret" })));
  await refusedPage(env, CLIENT_ID, /carries a client secret/);
  served.set(CLIENT_ID, json(goodDoc({ token_endpoint_auth_method: "client_secret_basic" })));
  await refusedPage(env, CLIENT_ID, /Krate takes only public clients/);
  served.set(CLIENT_ID, json(goodDoc({ token_endpoint_auth_method: "private_key_jwt" })));
  await refusedPage(env, CLIENT_ID, /Krate takes only public clients/);
  served.set(CLIENT_ID, json(goodDoc({ client_name: undefined })));
  await refusedPage(env, CLIENT_ID, /no client_name/);
  served.set(CLIENT_ID, json(goodDoc({ redirect_uris: ["http://evil.example/cb"] })));
  await refusedPage(env, CLIENT_ID, /redirect_uris must be/);
  served.set(CLIENT_ID, json(goodDoc({ grant_types: ["client_credentials"] })));
  await refusedPage(env, CLIENT_ID, /authorization_code/);
  served.set(CLIENT_ID, () => new Response("no", { status: 404 }));
  await refusedPage(env, CLIENT_ID, /answered 404/);

  // None of those was kept: the good document is read fresh the next time.
  const n = fetched.length;
  served.set(CLIENT_ID, json(goodDoc()));
  assert.equal((await call(env, null, "GET", authorizeUrl(CLIENT_ID, { challenge: pkce().challenge }))).status, 200);
  assert.equal(fetched.length, n + 1);
});

test("a host that is down is a try-again page at the consent page and a 503 at the token endpoint", async () => {
  const env = hubEnv();
  served.set(CLIENT_ID, () => new Response("down", { status: 503 }));
  await refusedPage(env, CLIENT_ID, /could not be read[\s\S]*Try again/);
  const tok = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: "x", client_id: CLIENT_ID });
  assert.equal(tok.status, 503);
  assert.equal(tok.body.error, "temporarily_unavailable");
  served.set(CLIENT_ID, () => {
    throw new TypeError("network down");
  });
  const net = await form(env, "/oauth/token", { grant_type: "refresh_token", refresh_token: "x", client_id: CLIENT_ID });
  assert.equal(net.status, 503);
});

test("a document is kept as its Cache-Control says, a day at most, and not at all for no-store", async () => {
  const env = hubEnv();
  const open = () => call(env, null, "GET", authorizeUrl(CLIENT_ID, { challenge: pkce().challenge }));
  served.set(CLIENT_ID, json(goodDoc(), { "cache-control": "no-store" }));
  await open();
  await open();
  assert.equal(fetched.length, 2, "no-store: fetched every time");

  __forgetClientDocuments();
  fetched.length = 0;
  served.set(CLIENT_ID, json(goodDoc(), { "cache-control": "public, max-age=999999999" }));
  const realNow = Date.now;
  try {
    await open();
    await open();
    assert.equal(fetched.length, 1, "kept");
    Date.now = () => realNow() + 23 * 3600 * 1000;
    await open();
    assert.equal(fetched.length, 1, "still kept within a day");
    Date.now = () => realNow() + 25 * 3600 * 1000;
    await open();
    assert.equal(fetched.length, 2, "a year's max-age is held to a day");
  } finally {
    Date.now = realNow;
  }
});

test("a lookalike name shows the real host, and a loopback redirect carries a warning", async () => {
  const env = hubEnv();
  const id = "https://evil.example/claude-metadata.json";
  served.set(id, json({ client_id: id, client_name: "Claude <script>alert(1)</script>", redirect_uris: ["http://127.0.0.1:33418/callback"] }));
  const r = await call(env, null, "GET", authorizeUrl(id, { challenge: pkce().challenge, redirect: "http://127.0.0.1:33418/callback" }));
  assert.equal(r.status, 200, String(r.body));
  assert.match(r.body, /<b>Claude &lt;script&gt;alert\(1\)&lt;\/script&gt;<\/b>, from <b>evil\.example<\/b>/);
  assert.ok(!r.body.includes("<script>alert"), "the name is escaped on the page and in the script");
  assert.match(r.body, /It will send you back to <b>127\.0\.0\.1:33418<\/b>/);
  assert.match(r.body, /class="warn">It sends you back to a program on this computer/);
});

test("registration (RFC 7591) still signs in exactly as before", async () => {
  const env = hubEnv();
  const reg = await call(env, null, "POST", "/oauth/register", { json: { client_name: "Claude", redirect_uris: [REDIRECT] } });
  assert.equal(reg.status, 201, JSON.stringify(reg.body));
  assert.match(reg.body.client_id, /^kmcpc_/);
  const { page, code, verifier } = await codeFor(env, reg.body.client_id);
  assert.match(page.body, /<b>Claude<\/b> \(client\.example\) wants to use your Krate account/);
  const tok = await form(env, "/oauth/token", { grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: reg.body.client_id, code_verifier: verifier });
  assert.equal(tok.status, 200, JSON.stringify(tok.body));
  assert.equal((await buildStatus(env, tok.body.access_token)).status, 200);
  assert.equal(fetched.length, 0, "a registered client is never fetched");
  // A published identity cannot spend a registered client's code, nor the reverse.
  served.set(CLIENT_ID, json(goodDoc()));
  const second = await codeFor(env, reg.body.client_id);
  const cross = await form(env, "/oauth/token", { grant_type: "authorization_code", code: second.code, redirect_uri: REDIRECT, client_id: CLIENT_ID, code_verifier: second.verifier });
  assert.equal(cross.body.error, "invalid_grant");
});
