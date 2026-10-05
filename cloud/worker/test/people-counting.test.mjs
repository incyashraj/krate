/* The desk's People numbers count people, not machines (2026-09-30).
 *
 * The founder watched two people sign up and the desk showed neither. The
 * sign-ups had failed (K-952) and nothing recorded a failed attempt, and the
 * visit and install counts could not tell a person from a crawler or a CI
 * runner. Pinned here, against the REAL worker:
 *   - a page view carries a daily visitor code, a bare source host and a
 *     human/bot flag, and never the full referrer URL;
 *   - a download click is its own event, with the system;
 *   - a sign-in step records how it ended, including a failure on our side;
 *   - usage from a cloud network is marked "cloud", from anywhere else "home";
 *   - none of it touches KV;
 *   - the People route lists accounts newest first.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/people-counting.test.mjs
 */
import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.js";

const KV = new Map();
let kvWrites = 0;
const points = [];
const env = {
  PUBLIC_BASE: "https://hub.test",
  VISITOR_SALT: "test-salt",
  KRATE_ADMINS: "boss",
  CF_ACCOUNT_ID: "acct",
  CF_ANALYTICS_TOKEN: "t",
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { kvWrites++; KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix }) => ({ keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })) }),
  },
  USAGE: { writeDataPoint: (p) => points.push(p) },
};

const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String(input.url || input);
  const reply = (v) => new Response(JSON.stringify(v), { headers: { "content-type": "application/json" } });
  if (url === "https://api.github.com/user") return reply({ id: 1, login: "boss", name: "Boss" });
  if (url.includes("/analytics_engine/sql")) return reply({ data: [] });
  if (url === "https://github.com/login/oauth/access_token") {
    if (globalThis.__ghThrows) return new Response("not json", { status: 500 });
    return reply({ error: "bad_verification_code" });
  }
  return realFetch(input, init);
};

function req(method, path, { body, headers = {}, cf } = {}) {
  const r = new Request(`https://hub.test${path}`, {
    method,
    headers: { "content-type": "application/json", ...headers },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (cf) Object.defineProperty(r, "cf", { value: cf });
  return worker.fetch(r, env, { waitUntil() {} });
}

const browser = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/537.36 Chrome/128 Safari/537.36";

test("a person's page view: visitor code, bare source host, country, human", async () => {
  points.length = 0;
  const res = await req("POST", "/view", {
    body: { page: "/", ref: "https://www.google.com/search?q=krate+app&secret=1" },
    headers: { "user-agent": browser, "cf-connecting-ip": "203.0.113.9" },
    cf: { country: "SG" },
  });
  assert.equal(res.status, 204);
  assert.equal(points.length, 1);
  const [p] = points;
  assert.deepEqual(p.blobs, ["view", "/", "google.com", "SG", "human", "view", "-"]);
  assert.match(p.indexes[0], /^[0-9a-f]{24}$/, "a visitor code, not an address");
  assert.ok(!JSON.stringify(p).includes("203.0.113.9"), "the address is never stored");
  assert.ok(!JSON.stringify(p).includes("secret=1"), "the full referrer is never stored");
});

test("the same browser on the same day is one visitor; another browser is another", async () => {
  points.length = 0;
  const h = { "user-agent": browser, "cf-connecting-ip": "203.0.113.9" };
  await req("POST", "/view", { body: { page: "/" }, headers: h });
  await req("POST", "/view", { body: { page: "/download/" }, headers: h });
  await req("POST", "/view", { body: { page: "/" }, headers: { ...h, "cf-connecting-ip": "198.51.100.4" } });
  assert.equal(points[0].indexes[0], points[1].indexes[0]);
  assert.notEqual(points[0].indexes[0], points[2].indexes[0]);
});

test("a crawler is counted as a bot, and our own pages are not a source", async () => {
  points.length = 0;
  await req("POST", "/view", {
    body: { page: "/", ref: "https://krate.tech/download/" },
    headers: { "user-agent": "Mozilla/5.0 (compatible; Googlebot/2.1)" },
  });
  assert.equal(points[0].blobs[4], "bot");
  assert.equal(points[0].blobs[2], "-");
});

test("a download click is its own event with the system", async () => {
  points.length = 0;
  await req("POST", "/view", { body: { page: "/download/", event: "download", os: "windows" }, headers: { "user-agent": browser } });
  assert.equal(points[0].blobs[5], "download");
  assert.equal(points[0].blobs[6], "windows");
});

test("a sign-in step that fails on our side is recorded as failed", async () => {
  // Plant a start so the callback reaches the token exchange, which fails.
  KV.set("login:abc", JSON.stringify({ from: "web" }));
  points.length = 0;
  const res = await req("GET", "/login/callback?code=x&state=abc");
  // K-972: a failed step goes back to the sign-in page with a reason, not a bare error page.
  assert.equal(res.status, 302, `the stubbed GitHub failure answers ${res.status}`);
  assert.equal(res.headers.get("location"), "https://krate.tech/login/?error=failed");
  const p = points.find((x) => x.blobs[0] === "signin");
  assert.ok(p, "the attempt left a trace");
  assert.deepEqual(p.blobs.slice(0, 3), ["signin", "github-done", "error"]);
});

test("cancelling at GitHub goes back to sign in, recorded as refused", async () => {
  KV.set("login:can", JSON.stringify({ from: "web" }));
  points.length = 0;
  const res = await req("GET", "/login/callback?error=access_denied&state=can");
  assert.equal(res.status, 302);
  assert.equal(res.headers.get("location"), "https://krate.tech/login/?error=cancelled");
  const p = points.find((x) => x.blobs[0] === "signin");
  assert.deepEqual(p.blobs.slice(0, 3), ["signin", "github-done", "refused"]);
});

test("an expired sign-in goes back to sign in with a reason", async () => {
  const res = await req("GET", "/login/callback?code=x&state=never-started");
  assert.equal(res.status, 302);
  assert.equal(res.headers.get("location"), "https://krate.tech/login/?error=expired");
});

test("a sign-in step that throws is still recorded before the error goes on", async () => {
  KV.set("login:def", JSON.stringify({ from: "web" }));
  points.length = 0;
  globalThis.__ghThrows = true;
  await assert.rejects(req("GET", "/login/callback?code=x&state=def"));
  globalThis.__ghThrows = false;
  const p = points.find((x) => x.blobs[0] === "signin");
  assert.ok(p, "a thrown step left a trace");
  assert.equal(p.blobs[2], "error");
});

test("usage from a cloud network is marked cloud; from a home ISP, home", async () => {
  points.length = 0;
  const ping = { id: "a1b2c3d4e5f60718", action: "install", version: "0.5.4", os: "linux" };
  await req("POST", "/usage", { body: ping, cf: { asOrganization: "Microsoft Corporation" } });
  await req("POST", "/usage", { body: ping, cf: { asOrganization: "Singtel Mobile" } });
  assert.equal(points[0].blobs[8], "cloud");
  assert.equal(points[1].blobs[8], "home");
});

test("none of this counting writes KV", () => {
  assert.equal(kvWrites, 0, `counting wrote ${kvWrites} KV keys`);
});

test("the People route lists accounts newest first", async () => {
  KV.set("user:old", JSON.stringify({ id: "old", name: "Old", created: Date.parse("2026-08-01") }));
  KV.set("user:new", JSON.stringify({ id: "new", name: "New", created: Date.now() }));
  KV.set("ident:github:1", "boss-id");
  KV.set("user:boss-id", JSON.stringify({ id: "boss-id", login: "boss", name: "Boss", providers: ["github"], created: Date.parse("2026-07-01") }));
  const res = await req("GET", "/admin/api/people?days=7", { headers: { authorization: "Bearer gho_boss" } });
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.deepEqual(body.newest_accounts.map((a) => a.name), ["New", "Old", "Boss"]);
  assert.equal(body.days.length, 7);
  assert.equal(body.days[0].accounts_new, 1, "today's account is counted today");
});
