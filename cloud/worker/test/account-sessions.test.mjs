/*
 * Signed-in sessions, and signing out everywhere (src/account-sessions.js),
 * through the hub's real sign-in routes and the real session routes: each
 * sign-in is listed with its kind, the browser it came from and when; the
 * id is not the token; one session or every other one is signed out, and
 * the hub refuses a signed-out token from then on -- for an email sign-in
 * (a Krate session) and for a GitHub sign-in (GitHub's own token, revoked
 * at GitHub). A Krate sign-in costs no more KV writes than before; a GitHub
 * sign-in costs one. GitHub is a stub: nothing here reaches it.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/account-sessions.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, worker } from "./org-tools.mjs";
import { sessionLabel } from "../src/account-sessions.js";

const CHROME_MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";
const SAFARI_IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1";
const FIREFOX_WIN = "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0";
const EDGE_WIN = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0";

/* GitHub, for the sign-in round trip and the revocation: each code is
 * exchanged for the next token in `issue`; /user answers zed for those
 * tokens and 401 once one is revoked; the OAuth app's revocation endpoint
 * records what it was asked to revoke. Anything else goes on. */
const gh = { issue: [], tokens: new Set(), revoked: new Set(), revokeCalls: [] };
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  const headers = new Headers((input && input.headers) || init.headers || {});
  if (url === "https://github.com/login/oauth/access_token") {
    const token = gh.issue.shift();
    gh.tokens.add(token);
    return new Response(JSON.stringify({ access_token: token }), { headers: { "content-type": "application/json" } });
  }
  if (url === "https://api.github.com/user") {
    const token = (headers.get("authorization") || "").replace(/^Bearer\s+/i, "");
    if (gh.revoked.has(token)) return new Response("Bad credentials", { status: 401 });
    if (gh.tokens.has(token)) {
      return new Response(JSON.stringify({ id: 7007, login: "zed", name: "Zed", avatar_url: "" }), { headers: { "content-type": "application/json" } });
    }
  }
  const revoke = url.match(/^https:\/\/api\.github\.com\/applications\/([^/]+)\/token$/);
  if (revoke && (init.method || "GET") === "DELETE") {
    const { access_token } = JSON.parse(init.body);
    gh.revokeCalls.push({ client: revoke[1], auth: headers.get("authorization"), token: access_token });
    if (!gh.tokens.has(access_token) || gh.revoked.has(access_token)) return new Response(null, { status: 404 });
    gh.revoked.add(access_token);
    return new Response(null, { status: 204 });
  }
  return before(input, init);
};

async function hit(env, method, path, { token, ua } = {}) {
  const h = {};
  if (token) h.authorization = `Bearer ${token}`;
  if (ua) h["user-agent"] = ua;
  const res = await worker.fetch(new Request(`https://hub.example${path}`, { method, headers: h }), env);
  const text = await res.text();
  let body = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = text;
  }
  return { status: res.status, body, headers: res.headers };
}

/* Count every KV write, so a test can say what a sign-in cost. */
function counting(env) {
  const writes = [];
  const put = env.APPS.put;
  env.APPS.put = async (k, v, o) => {
    writes.push(k);
    return put(k, v, o);
  };
  return writes;
}

/* The real email-link sign-in: the link's record, then the click. */
let links = 0;
async function emailSignIn(env, email, ua) {
  const t = `link${++links}`;
  env._kv.set(`email:${t}`, JSON.stringify({ email, from: "web", nonce: "", app_nonce: "" }));
  const res = await hit(env, "GET", `/login/email/verify?t=${t}`, { ua });
  assert.strictEqual(res.status, 302, JSON.stringify(res.body));
  const token = new URLSearchParams(res.headers.get("location").split("#")[1]).get("token");
  assert.match(token, /^krs_/);
  return token;
}

/* The real GitHub web sign-in: start's state, then GitHub's callback. */
let states = 0;
async function githubSignIn(env, token, ua) {
  const state = `state-${++states}`;
  env._kv.set(`login:${state}`, JSON.stringify({ from: "web", nonce: "", app_nonce: "" }));
  gh.issue.push(token);
  const res = await hit(env, "GET", `/login/callback?state=${state}&code=c${states}`, { ua });
  assert.strictEqual(res.status, 302, JSON.stringify(res.body));
  assert.strictEqual(new URLSearchParams(res.headers.get("location").split("#")[1]).get("token"), token);
  return token;
}

const list = (env, token) => hit(env, "GET", "/account/sessions", { token });

test("each sign-in is listed with its kind, its browser and when; the id is not the token; this one is current", async () => {
  const env = orgEnv();
  const writes = counting(env);
  const a = await emailSignIn(env, "ana@corp.example", CHROME_MAC);
  // A Krate sign-in writes what it wrote before: the session and its index entry.
  assert.deepStrictEqual(writes.filter((k) => k.startsWith("session:") || k.startsWith("usess:")).map((k) => k.split(":")[0]), ["session", "usess"]);
  const b = await emailSignIn(env, "ana@corp.example", SAFARI_IPHONE);
  const r = await list(env, a);
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  assert.deepStrictEqual(Object.keys(r.body), ["sessions"]);
  assert.strictEqual(r.body.sessions.length, 2);
  for (const s of r.body.sessions) {
    assert.deepStrictEqual(Object.keys(s).sort(), ["created_at", "current", "id", "kind", "label"]);
    assert.match(s.id, /^[0-9a-f]{16}$/);
    assert.strictEqual(s.kind, "email");
    assert.ok(Math.abs(s.created_at - Date.now() / 1000) < 60);
  }
  const [first, second] = r.body.sessions;
  assert.deepStrictEqual([first.current, first.label], [true, "Chrome on Mac"], "the one asking, first");
  assert.deepStrictEqual([second.current, second.label], [false, "Safari on iPhone"]);
  const raw = JSON.stringify(r.body);
  assert.ok(!raw.includes(a.slice(4)) && !raw.includes(b.slice(4)), "no token, or any part of one, in the answer");
  assert.strictEqual(r.headers.get("access-control-allow-origin"), "*");
  assert.deepStrictEqual((await list(env, b)).body.sessions.map((s) => [s.label, s.current]), [["Safari on iPhone", true], ["Chrome on Mac", false]]);
  assert.deepStrictEqual((await list(env, null)).status, 401);
  assert.deepStrictEqual((await list(env, "krs_madeup")).status, 401);
});

test("signing one session out ends it: the hub refuses its token from then on", async () => {
  const env = orgEnv();
  const a = await emailSignIn(env, "ana@corp.example", CHROME_MAC);
  const b = await emailSignIn(env, "ana@corp.example", FIREFOX_WIN);
  const other = await emailSignIn(env, "bo@corp.example", EDGE_WIN);
  const bId = (await list(env, a)).body.sessions.find((s) => !s.current).id;
  // Someone else's session is not theirs to end.
  const othersId = (await list(env, other)).body.sessions[0].id;
  assert.deepStrictEqual([(await hit(env, "DELETE", `/account/sessions/${othersId}`, { token: a })).status], [404]);
  assert.strictEqual((await list(env, other)).status, 200, "still signed in");

  const del = await hit(env, "DELETE", `/account/sessions/${bId}`, { token: a });
  assert.deepStrictEqual([del.status, del.body], [200, { ok: true }]);
  assert.strictEqual((await list(env, b)).status, 401, "the signed-out token is refused");
  assert.strictEqual((await hit(env, "GET", "/orgs", { token: b })).status, 401, "everywhere the hub checks a sign-in");
  assert.deepStrictEqual((await list(env, a)).body.sessions.map((s) => s.label), ["Chrome on Mac"]);
  assert.strictEqual((await hit(env, "DELETE", `/account/sessions/${bId}`, { token: a })).status, 404, "already ended");
  // Signing out the session making the call.
  const aId = (await list(env, a)).body.sessions[0].id;
  assert.strictEqual((await hit(env, "DELETE", `/account/sessions/${aId}`, { token: a })).status, 200);
  assert.strictEqual((await list(env, a)).status, 401);
});

test("signing out everywhere else keeps this session and ends the rest", async () => {
  const env = orgEnv();
  const a = await emailSignIn(env, "ana@corp.example", CHROME_MAC);
  const rest = [await emailSignIn(env, "ana@corp.example", SAFARI_IPHONE), await emailSignIn(env, "ana@corp.example", FIREFOX_WIN), await emailSignIn(env, "ana@corp.example", EDGE_WIN)];
  // An index entry from before sessions carried their details still lists.
  const userId = env._kv.get(`session:${a}`);
  env._kv.set("session:krs_fromlastmonth", userId);
  env._kv.set(`usess:${userId}:krs_fromlastmonth`, "1");
  const old = (await list(env, a)).body.sessions.find((s) => s.created_at === null);
  assert.deepStrictEqual([old.label, old.kind, old.current], ["A browser", "email", false]);
  const r = await hit(env, "POST", "/account/sessions/revoke-others", { token: a });
  assert.deepStrictEqual([r.status, r.body], [200, { revoked: 4 }]);
  for (const t of [...rest, "krs_fromlastmonth"]) assert.strictEqual((await list(env, t)).status, 401, "ended");
  assert.deepStrictEqual((await list(env, a)).body.sessions.map((s) => [s.label, s.current]), [["Chrome on Mac", true]]);
  assert.deepStrictEqual((await hit(env, "POST", "/account/sessions/revoke-others", { token: a })).body, { revoked: 0 });
});

test("a GitHub sign-in is listed (one write) and signed out at GitHub, so the hub refuses it after", async () => {
  const env = orgEnv({ GITHUB_CLIENT_SECRET: "gh-secret" });
  const writes = counting(env);
  const one = await githubSignIn(env, "gho_zed_one", CHROME_MAC);
  assert.deepStrictEqual(writes.filter((k) => k.startsWith("usess:")).length, 1, "the one index entry");
  writes.length = 0;
  const two = await githubSignIn(env, "gho_zed_two", FIREFOX_WIN);
  // A returning GitHub account: the index entry is the only write.
  assert.deepStrictEqual(writes.filter((k) => !k.startsWith("login:")).map((k) => k.split(":")[0]), ["usess"]);

  const r = await list(env, one);
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  assert.deepStrictEqual(r.body.sessions.map((s) => [s.kind, s.label, s.current]), [["github", "Chrome on Mac", true], ["github", "Firefox on Windows", false]]);
  assert.ok(!JSON.stringify(r.body).includes("zed_two"));
  // The other one has been used, so the hub holds its six-hour identity cache.
  assert.strictEqual((await list(env, two)).status, 200);
  assert.ok([...env._kv.keys()].some((k) => k.startsWith("token:")), "the cache exists");

  const out = await hit(env, "POST", "/account/sessions/revoke-others", { token: one });
  assert.deepStrictEqual([out.status, out.body], [200, { revoked: 1 }]);
  assert.deepStrictEqual(gh.revokeCalls.at(-1), { client: "Ov23liV2n8Dxi0okyv0F", auth: `Basic ${btoa("Ov23liV2n8Dxi0okyv0F:gh-secret")}`, token: two });
  assert.strictEqual((await list(env, two)).status, 401, "GitHub's token is refused once revoked");
  assert.strictEqual((await hit(env, "GET", "/orgs", { token: two })).status, 401);
  assert.strictEqual((await list(env, one)).status, 200, "this one stays");
  assert.deepStrictEqual((await list(env, one)).body.sessions.length, 1);
});

test("without the GitHub secret a GitHub session cannot be signed out, and says so; a test sign-in is one session", async () => {
  const env = orgEnv();
  const one = await githubSignIn(env, "gho_zed_three", CHROME_MAC);
  await githubSignIn(env, "gho_zed_four", SAFARI_IPHONE);
  const otherId = (await list(env, one)).body.sessions.find((s) => !s.current).id;
  const r = await hit(env, "DELETE", `/account/sessions/${otherId}`, { token: one });
  assert.deepStrictEqual([r.status, r.body.error], [502, "not-revocable"]);
  assert.strictEqual((await list(env, one)).body.sessions.length, 2, "nothing was dropped");
  assert.deepStrictEqual((await hit(env, "POST", "/account/sessions/revoke-others", { token: one })).body, { revoked: 0 });

  const local = orgEnv({ PUBLIC_BASE: "http://127.0.0.1:8787", KRATE_TEST_IDENTITIES: "tok-alice=alice" });
  const t = await hit(local, "GET", "/account/sessions", { token: "tok-alice", ua: CHROME_MAC });
  assert.strictEqual(t.status, 200);
  assert.strictEqual(t.body.sessions.length, 1);
  assert.deepStrictEqual([t.body.sessions[0].current, t.body.sessions[0].kind, t.body.sessions[0].label, t.body.sessions[0].created_at], [true, "github", "Chrome on Mac", null]);
  assert.deepStrictEqual((await hit(local, "POST", "/account/sessions/revoke-others", { token: "tok-alice" })).body, { revoked: 0 });
  assert.strictEqual((await hit(local, "DELETE", `/account/sessions/${t.body.sessions[0].id}`, { token: "tok-alice" })).status, 409);
});

test("labels read as a person says them", () => {
  assert.strictEqual(sessionLabel(CHROME_MAC), "Chrome on Mac");
  assert.strictEqual(sessionLabel(SAFARI_IPHONE), "Safari on iPhone");
  assert.strictEqual(sessionLabel(FIREFOX_WIN), "Firefox on Windows");
  assert.strictEqual(sessionLabel(EDGE_WIN), "Edge on Windows");
  assert.strictEqual(sessionLabel(CHROME_MAC, "app"), "Krate on Mac", "handed to the desktop app");
  assert.strictEqual(sessionLabel("Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/131.0"), "Firefox on Linux");
  assert.strictEqual(sessionLabel(""), "A browser");
});
