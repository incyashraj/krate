/* Ways to sign in (IC-996): one account, GitHub, Google, Apple and email.
 *
 * Settings lists an account's ways in, adds one on purpose, and removes
 * one. Adding is where an attack lives: a provider page finished in
 * somebody else's browser would attach THEIR GitHub to the account that
 * started the link. So the provider's answer is only parked, and only the
 * starting account's own session confirms it.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/ways-to-sign-in.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync, createSign } from "node:crypto";
import worker from "../src/index.js";
import { orgEnv } from "./org-tools.mjs";
import "./github-stub.mjs";

// GitHub's and Google's token exchanges, as the stub answers them.
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String((input && input.url) || input);
  if (url === "https://github.com/login/oauth/access_token") {
    const code = new URLSearchParams(String(init.body || "")).get("code") || "";
    return new Response(JSON.stringify({ access_token: `gho_${code}` }), { headers: { "content-type": "application/json" } });
  }
  if (url === "https://oauth2.googleapis.com/token") {
    const code = new URLSearchParams(String(init.body || "")).get("code") || "";
    return new Response(JSON.stringify({ id_token: `google-${code}` }), { headers: { "content-type": "application/json" } });
  }
  if (url.startsWith("https://oauth2.googleapis.com/tokeninfo")) {
    const who = new URL(url).searchParams.get("id_token").slice("google-".length);
    return new Response(JSON.stringify({ aud: "gid", sub: `g-${who}`, email: `${who}@gmail.example`, email_verified: "true", name: who }), {
      headers: { "content-type": "application/json" },
    });
  }
  return before(input, init);
};

const hubEnv = () => {
  const env = orgEnv({
    PUBLIC_BASE: "https://hub.example",
    GITHUB_CLIENT_SECRET: "s",
    GOOGLE_CLIENT_ID: "gid",
    GOOGLE_CLIENT_SECRET: "gs",
    RESEND_API_KEY: "r",
  });
  env.USAGE = { writeDataPoint() {} };
  env.__mailSink = [];
  return env;
};
const hit = async (env, path, { token, method = "GET", json, form } = {}) => {
  const headers = {};
  if (token) headers.authorization = `Bearer ${token}`;
  let body;
  if (json !== undefined) {
    headers["content-type"] = "application/json";
    body = JSON.stringify(json);
  }
  if (form) body = form;
  const r = await worker.fetch(new Request(`https://hub.example${path}`, { method, headers, body, redirect: "manual" }), env, { waitUntil() {} });
  const text = await r.text();
  let parsed = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = text;
  }
  return { status: r.status, body: parsed, location: r.headers.get("location") || "" };
};
const NONCE = "n".repeat(24);

/// A GitHub sign-in through the web flow, as a person does it: the account
/// it makes, and a Krate session on that account to drive Settings with.
async function githubPerson(env, login) {
  await hit(env, "/my/apps", { token: `gho_${login}` });
  const userId = env._kv.get(`ghlogin:${login}`);
  env._kv.set(`session:krs_${login}-s`, userId);
  return { userId, session: `krs_${login}-s` };
}
const fragment = (location) => new URLSearchParams(location.split("#")[1] || "");

/// Start a link from Settings, finish the provider's page as `who`, and
/// hand back what the browser lands on.
async function linkThrough(env, session, kind, who) {
  const start = await hit(env, "/me/sign-ins/link", { token: session, method: "POST", json: { kind, nonce: NONCE } });
  assert.equal(start.status, 200, JSON.stringify(start.body));
  const state = new URL(start.body.go).searchParams.get("state");
  const back = kind === "github" ? `/login/callback?state=${state}&code=${who}` : `/login/google/callback?state=${state}&code=${who}`;
  const landed = await hit(env, back);
  assert.equal(landed.status, 302);
  assert.match(landed.location, /^https:\/\/krate\.tech\/account\/#/, "back to the account page, never signed in somewhere");
  return fragment(landed.location);
}

test("a GitHub account adds Google, and both sign in to the same account", async () => {
  const env = hubEnv();
  const { userId, session } = await githubPerson(env, "alice");
  const listed = await hit(env, "/me/sign-ins", { token: session });
  assert.deepEqual(listed.body.ways.map((w) => w.kind), ["github"]);
  assert.equal(listed.body.ways[0].label, "alice");
  assert.equal(listed.body.ways[0].removable, false, "the only way in cannot be removed");

  const back = await linkThrough(env, session, "google", "alice");
  assert.equal(back.get("nonce"), NONCE, "the page's own nonce comes back, so it can tell this browser started it");
  assert.equal(env._kv.get("ident:google:g-alice"), undefined, "nothing is added before the page confirms");
  const done = await hit(env, "/me/sign-ins/confirm", { token: session, method: "POST", json: { code: back.get("link") } });
  assert.equal(done.status, 200, JSON.stringify(done.body));
  assert.deepEqual(done.body.ways.map((w) => w.kind).sort(), ["email", "github", "google"]);
  assert.equal(env._kv.get("ident:google:g-alice"), userId);

  // Google sign-in now lands on the same account.
  const st = "s".repeat(36);
  env._kv.set(`login:${st}`, JSON.stringify({ from: "web", nonce: NONCE, app_nonce: "" }));
  const signin = await hit(env, `/login/google/callback?state=${st}&code=alice`);
  const token = fragment(signin.location).get("token");
  assert.equal(env._kv.get(`session:${token}`), userId, "the Google sign-in opened the GitHub account");
});

test("a link finished in someone else's browser adds nothing to anyone", async () => {
  const env = hubEnv();
  const attacker = await githubPerson(env, "mallory");
  const victim = await githubPerson(env, "victim");
  // The attacker starts a link and sends the victim the provider's page;
  // the victim's browser finishes it as the victim's Google.
  const back = await linkThrough(env, attacker.session, "google", "victim");
  const tried = await hit(env, "/me/sign-ins/confirm", { token: victim.session, method: "POST", json: { code: back.get("link") } });
  assert.equal(tried.status, 403, "the victim's session cannot finish a link the attacker started");
  assert.equal(env._kv.get("ident:google:g-victim"), undefined);
});

test("a way that already opens another account is not moved", async () => {
  const env = hubEnv();
  const alice = await githubPerson(env, "alice");
  const bob = await githubPerson(env, "bob");
  const back = await linkThrough(env, alice.session, "github", "bob");
  const done = await hit(env, "/me/sign-ins/confirm", { token: alice.session, method: "POST", json: { code: back.get("link") } });
  assert.equal(done.status, 409, JSON.stringify(done.body));
  assert.match(done.body.message, /already signs in to a different Krate account/);
  assert.equal(env._kv.get("ghlogin:bob"), bob.userId, "bob's GitHub still opens bob's account");
});

test("an address is added by a mail to it, confirmed by the account's own session", async () => {
  const env = hubEnv();
  const { userId, session } = await githubPerson(env, "alice");
  const asked = await hit(env, "/me/sign-ins/email", { token: session, method: "POST", json: { email: "Alice@Work.Example" } });
  assert.equal(asked.status, 200, JSON.stringify(asked.body));
  const mail = env.__mailSink.pop();
  assert.equal(mail.to, "alice@work.example");
  const landed = await hit(env, new URL(mail.link).pathname + new URL(mail.link).search);
  const code = fragment(landed.location).get("link");
  assert.equal(env._kv.get("ident:email:alice@work.example"), undefined, "not before the page confirms");
  const done = await hit(env, "/me/sign-ins/confirm", { token: session, method: "POST", json: { code } });
  assert.equal(done.status, 200, JSON.stringify(done.body));
  assert.equal(env._kv.get("ident:email:alice@work.example"), userId);
});

test("removing a way ends its sessions and keeps the account", async () => {
  const env = hubEnv();
  const { userId, session } = await githubPerson(env, "alice");
  const back = await linkThrough(env, session, "google", "alice");
  await hit(env, "/me/sign-ins/confirm", { token: session, method: "POST", json: { code: back.get("link") } });
  // A Google session on the account.
  env._kv.set("session:krs_google-one", userId);
  env._kv.set(`usess:${userId}:krs_google-one`, JSON.stringify({ kind: "google", label: "A browser", created_at: 1 }));
  const gone = await hit(env, "/me/sign-ins/google/g-alice", { token: session, method: "DELETE" });
  assert.equal(gone.status, 200, JSON.stringify(gone.body));
  assert.equal(env._kv.get("ident:google:g-alice"), undefined);
  assert.equal(env._kv.get("session:krs_google-one"), undefined, "the Google sign-in's session ended");
  assert.equal(env._kv.get(`session:${session}`), userId, "this session stays");
  assert.ok(gone.body.ways.some((w) => w.kind === "github"));
});

test("the last way in stays", async () => {
  const env = hubEnv();
  const { session } = await githubPerson(env, "alice");
  const listed = await hit(env, "/me/sign-ins", { token: session });
  const only = listed.body.ways[0];
  const r = await hit(env, `/me/sign-ins/${only.kind}/${encodeURIComponent(only.id || "-")}`, { token: session, method: "DELETE" });
  assert.equal(r.status, 409);
  assert.match(r.body.message, /only way into your account/);
});

test("Sign in with Apple: a checked token signs in; a forged one does not", async () => {
  const env = hubEnv();
  env.APPLE_CLIENT_ID = "tech.krate.signin";
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const jwk = { ...publicKey.export({ format: "jwk" }), kid: "k1", alg: "RS256", use: "sig" };
  env.__appleKeys = async () => [jwk];
  const b64 = (o) => Buffer.from(JSON.stringify(o)).toString("base64url");
  const tokenFor = (claims, key = privateKey) => {
    const head = b64({ alg: "RS256", kid: "k1" });
    const body = b64(claims);
    const sig = createSign("RSA-SHA256").update(`${head}.${body}`).sign(key).toString("base64url");
    return `${head}.${body}.${sig}`;
  };
  const begin = await hit(env, `/login/apple/start?from=web&nonce=${NONCE}`);
  assert.equal(begin.status, 302);
  const state = new URL(begin.location).searchParams.get("state");
  const claims = { iss: "https://appleid.apple.com", aud: "tech.krate.signin", exp: Math.floor(Date.now() / 1000) + 600, sub: "apple-1", nonce: state, email: "x@privaterelay.appleid.com", email_verified: "true" };

  // Forged: signed by a key Apple never published.
  const other = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey;
  const forged = new FormData();
  forged.set("state", state);
  forged.set("id_token", tokenFor(claims, other));
  const refused = await hit(env, "/login/apple/callback", { method: "POST", form: forged });
  assert.doesNotMatch(refused.location || "", /token=/, "a forged token signs nobody in");

  const begin2 = await hit(env, `/login/apple/start?from=web&nonce=${NONCE}`);
  const state2 = new URL(begin2.location).searchParams.get("state");
  const real = new FormData();
  real.set("state", state2);
  real.set("id_token", tokenFor({ ...claims, nonce: state2 }));
  real.set("user", JSON.stringify({ name: { firstName: "Ada", lastName: "L" } }));
  const ok = await hit(env, "/login/apple/callback", { method: "POST", form: real });
  const token = fragment(ok.location).get("token");
  assert.ok(token && token.startsWith("krs_"), ok.location);
  const userId = env._kv.get(`session:${token}`);
  const user = JSON.parse(env._kv.get(`user:${userId}`));
  assert.equal(user.name, "Ada L");
  assert.deepEqual(user.ways.map((w) => w.kind), ["apple"]);
});

test("the sign-in page learns which ways this hub offers", async () => {
  const env = hubEnv();
  const r = await hit(env, "/login/ways");
  assert.deepEqual(r.body, { github: true, google: true, apple: false, email: true }, "Apple hidden until configured");
});
