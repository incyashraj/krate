/* A finished sign-in carries back the nonce its page started with (K-909).
 *
 * The page that starts a sign-in keeps a random nonce and /login/done only
 * accepts a session that comes back with it. That check lives in the page;
 * this proves the hub's half: every door -- GitHub, Google, email, and the
 * desktop's `app_nonce` -- hands the nonce back in the fragment, unchanged,
 * and a start with no nonce still signs a person in (an old page
 * mid-deploy must not lock anyone out).
 *
 * Also pinned: the nonce rides in the state record the start already
 * writes. A sign-in costs no more KV writes than before (K-912).
 *
 * Drives the REAL worker with a Map for KV and a stubbed fetch for the
 * providers.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/sign-in-nonce.test.mjs
 */
import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.js";

const KV = new Map();
let puts = 0;
const env = {
  PUBLIC_BASE: "https://hub.test",
  GITHUB_CLIENT_SECRET: "gh-secret",
  GOOGLE_CLIENT_ID: "google-id",
  GOOGLE_CLIENT_SECRET: "google-secret",
  RESEND_API_KEY: "resend-key",
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { puts++; KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix }) => ({ keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })) }),
  },
};

let mailed = null;
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String(input.url || input);
  const reply = (v) => new Response(JSON.stringify(v), { headers: { "content-type": "application/json" } });
  if (url === "https://github.com/login/oauth/access_token") return reply({ access_token: "gho_mallory" });
  if (url === "https://api.github.com/user") return reply({ id: 7, login: "mallory", name: "M" });
  if (url === "https://oauth2.googleapis.com/token") return reply({ id_token: "idt" });
  if (url.startsWith("https://oauth2.googleapis.com/tokeninfo")) {
    return reply({ aud: "google-id", sub: "g-1", email: "m@example.com", name: "M" });
  }
  if (url === "https://api.resend.com/emails") {
    mailed = JSON.parse(init.body);
    return reply({ id: "sent" });
  }
  return realFetch(input, init);
};

async function go(method, path, body) {
  const res = await worker.fetch(
    new Request(`https://hub.test${path}`, {
      method,
      headers: body ? { "content-type": "application/json" } : {},
      body: body ? JSON.stringify(body) : undefined,
    }),
    env,
    { waitUntil() {} },
  );
  return res;
}

/// Where the sign-in finally lands, as { app, fields } from /login/done.
function landing(res) {
  assert.strictEqual(res.status, 302, "a finished sign-in redirects");
  const where = new URL(res.headers.get("location"));
  assert.strictEqual(where.origin + where.pathname, "https://krate.tech/login/done/");
  return {
    app: where.searchParams.get("app") === "1",
    fields: new URLSearchParams(where.hash.slice(1)),
  };
}

async function viaGitHub(query) {
  const start = await go("GET", `/login/start${query}`);
  assert.strictEqual(start.status, 302);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  return landing(await go("GET", `/login/callback?state=${state}&code=abc`));
}

async function viaGoogle(query) {
  const start = await go("GET", `/login/google/start${query}`);
  assert.strictEqual(start.status, 302);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  return landing(await go("GET", `/login/google/callback?state=${state}&code=abc`));
}

async function viaEmail(body) {
  const sent = await go("POST", "/login/email", { email: "m@example.com", ...body });
  assert.strictEqual(sent.status, 200, await sent.text());
  const link = new URL(mailed.text.match(/https:\/\/\S+/)[0]);
  return landing(await go("GET", link.pathname + link.search));
}

const NONCE = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
const APP_NONCE = "0f9e8d7c6b5a49382716f5e4d3c2b1a0";

test("every door hands back the nonce its page started with", async () => {
  for (const [door, run] of [
    ["github", () => viaGitHub(`?nonce=${NONCE}`)],
    ["google", () => viaGoogle(`?nonce=${NONCE}`)],
    ["email", () => viaEmail({ nonce: NONCE })],
  ]) {
    const { app, fields } = await run();
    assert.ok(fields.get("token"), `${door}: a session is delivered`);
    assert.strictEqual(fields.get("nonce"), NONCE, `${door}: the page's nonce comes back unchanged`);
    assert.strictEqual(app, false, `${door}: a web sign-in lands on the web`);
  }
});

test("a desktop sign-in carries the desktop's own nonce through as well", async () => {
  for (const [door, run] of [
    ["github", () => viaGitHub(`?from=app&nonce=${NONCE}&app_nonce=${APP_NONCE}`)],
    ["google", () => viaGoogle(`?from=app&nonce=${NONCE}&app_nonce=${APP_NONCE}`)],
    ["email", () => viaEmail({ from: "app", nonce: NONCE, app_nonce: APP_NONCE })],
  ]) {
    const { app, fields } = await run();
    assert.strictEqual(app, true, `${door}: hands back to the desktop`);
    assert.strictEqual(fields.get("nonce"), NONCE, `${door}: the browser's nonce`);
    assert.strictEqual(fields.get("app_nonce"), APP_NONCE, `${door}: and the desktop's`);
  }
});

test("no nonce still signs in, and nothing hostile rides through", async () => {
  // An old page, or a released desktop, starts with no nonce: the person
  // still gets a session (the page decides whether to take it).
  const bare = await viaGitHub("");
  assert.ok(bare.fields.get("token"));
  assert.strictEqual(bare.fields.get("nonce"), null, "no nonce is invented");
  // A nonce is a token of plain characters. Anything else is dropped, not
  // echoed into a URL the page then parses.
  const odd = await viaGitHub(`?nonce=${encodeURIComponent("x&token=evil#y")}`);
  assert.strictEqual(odd.fields.get("nonce"), null);
  assert.strictEqual(odd.fields.getAll("token").length, 1, "one token, the real one");
});

test("a state written by the old hub still finishes", async () => {
  // Before K-909 the record was the bare word "app" or "web".
  KV.set("login:old-state", "app");
  const { app, fields } = landing(await go("GET", "/login/callback?state=old-state&code=abc"));
  assert.strictEqual(app, true);
  assert.ok(fields.get("token"));
});

test("the nonce costs no KV write of its own", async () => {
  // Same number of writes with and without a nonce: it rides in the state
  // record the start writes anyway.
  const count = async (query) => {
    const before = puts;
    await viaGitHub(query);
    return puts - before;
  };
  assert.strictEqual(await count(`?nonce=${NONCE}&app_nonce=${APP_NONCE}`), await count(""));
});
