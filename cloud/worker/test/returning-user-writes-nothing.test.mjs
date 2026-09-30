/* A returning, unchanged account costs zero KV writes (K-952).
 *
 * Every request that carries a GitHub token goes through ensureUser. It used
 * to rewrite the user, the ident and the email ident on each call, so one
 * person working in Studio (whose session sync sends that token on every
 * save and list) spent the free plan's 1,000 writes a day, and every new
 * sign-up after that failed before an account could exist.
 *
 * Drives the REAL worker with a counting Map for KV and a stubbed GitHub.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/returning-user-writes-nothing.test.mjs
 */
import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.js";

const KV = new Map();
const writes = [];
const env = {
  PUBLIC_BASE: "https://hub.test",
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { writes.push(k); KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix }) => ({ keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })) }),
  },
};

let ghUser = { id: 41, login: "ada", name: "Ada", email: "ada@example.com", avatar_url: "https://a/1.png" };
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String(input.url || input);
  if (url === "https://api.github.com/user") {
    return new Response(JSON.stringify(ghUser), { headers: { "content-type": "application/json" } });
  }
  return realFetch(input, init);
};

async function asGitHub(path) {
  const res = await worker.fetch(
    new Request(`https://hub.test${path}`, { headers: { authorization: "Bearer gho_ada_token" } }),
    env,
    { waitUntil() {} },
  );
  return res.status;
}

test("the first request with a GitHub token creates the account", async () => {
  const status = await asGitHub("/sessions");
  assert.equal(status, 200, "an authenticated session list answers 200");
  const users = [...KV.keys()].filter((k) => k.startsWith("user:"));
  assert.equal(users.length, 1, "one account exists after the first request");
  assert.ok(writes.some((k) => k.startsWith("user:")), "creating the account wrote user:");
  assert.ok(writes.includes("ident:github:41"), "and the GitHub ident");
  assert.ok(writes.includes("ident:email:ada@example.com"), "and the email ident");
});

test("the same person, unchanged, costs no writes however often they call", async () => {
  writes.length = 0;
  for (let i = 0; i < 25; i++) {
    assert.equal(await asGitHub("/sessions"), 200);
  }
  assert.deepEqual(writes, [], `25 unchanged requests wrote: ${writes.join(", ")}`);
});

test("something that really changed is still written", async () => {
  // A blank filled in (the account had no name), then nothing again.
  const userKey = [...KV.keys()].find((k) => k.startsWith("user:"));
  const rec = JSON.parse(KV.get(userKey));
  rec.name = "";
  KV.set(userKey, JSON.stringify(rec));
  writes.length = 0;
  await asGitHub("/sessions");
  assert.deepEqual(writes, [userKey], "only the user record is rewritten to fill the blank");
  assert.equal(JSON.parse(KV.get(userKey)).name, "Ada");
  writes.length = 0;
  await asGitHub("/sessions");
  assert.deepEqual(writes, [], "and the next request writes nothing");
});

test("a new person still gets an account", async () => {
  ghUser = { id: 42, login: "grace", name: "Grace", email: "grace@example.com" };
  writes.length = 0;
  await asGitHub("/sessions");
  const users = [...KV.keys()].filter((k) => k.startsWith("user:"));
  assert.equal(users.length, 2, "a second account exists");
  assert.ok(writes.includes("ident:github:42"));
});
