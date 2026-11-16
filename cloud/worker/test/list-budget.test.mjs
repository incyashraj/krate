/* The ten-minute run lists KV only when something may be waiting.
 *
 * The free plan allows about 1,000 KV list operations a day for the whole
 * account. On 2026-11-16 they were gone by late morning -- welcomes stopped
 * and the admin desk went blank -- and two ten-minute jobs had been listing
 * 288 times a day, nearly always finding nothing.
 *
 * Drives the REAL worker's scheduled run with a Map for KV that counts lists:
 *   - a first run lists (it has never swept: work queued before the stamps
 *     existed still gets done), finds nothing, and records the sweep;
 *   - a second run with nothing queued lists nothing;
 *   - a sign-up's waiting stamp makes the next run list again, and a welcome
 *     not yet due keeps it listing until the welcome goes;
 *   - a stamp younger than the KV list lag is never recorded as swept.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/list-budget.test.mjs
 */
import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.js";

const KV = new Map();
let lists = 0;
const env = {
  PUBLIC_BASE: "https://hub.test",
  WELCOME_AUTO: "1",
  RESEND_API_KEY: "resend-key",
  KEY_WRAP_SECRET: "wrap-secret",
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix }) => {
      lists++;
      return { keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true };
    },
  },
};

const mailed = [];
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  if (String(input.url || input) === "https://api.resend.com/emails") {
    mailed.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
  }
  return realFetch(input, init);
};

const tick = async (now) => {
  const real = Date.now;
  Date.now = () => now;
  const before = lists;
  try {
    await worker.scheduled({ cron: "*/10 * * * *" }, env);
  } finally {
    Date.now = real;
  }
  return lists - before;
};

const MIN = 60_000;

test("an idle ten-minute run lists nothing once it has swept", async () => {
  const t0 = Date.now() + 10 * MIN;
  assert.equal(await tick(t0), 1, "never swept: the first run lists, so old queued work is found");
  assert.ok(KV.has("list-swept:welcome"), "and records that nothing was left");
  assert.equal(await tick(t0 + 10 * MIN), 0, "nothing queued since: no list");
  assert.equal(await tick(t0 + 20 * MIN), 0, "and still none");
});

test("a sign-up's stamp brings the list back until its welcome goes", async () => {
  const signedUp = Date.now();
  KV.set("user:u1", JSON.stringify({ id: "u1", login: "ada", name: "Ada Lovelace", email: "ada@example.com", created: signedUp }));
  KV.set("welcome-due:u1", String(signedUp));
  // What the sign-up writes beside the due mark.
  KV.set("list-waiting:welcome", `${signedUp}-aaaaaaaa`);

  assert.equal(await tick(signedUp + 5 * MIN), 1, "something waits: the run lists");
  assert.equal(mailed.length, 0, "five minutes in, not yet due");
  assert.equal(await tick(signedUp + 15 * MIN), 1, "still waiting, so it lists again");
  assert.equal(await tick(signedUp + 25 * MIN), 1, "due now: listed and sent");
  assert.equal(mailed.length, 1, "the welcome went");
  assert.equal(await tick(signedUp + 35 * MIN), 0, "cleared: back to no lists");
});

test("the public gallery, /stats and developer names list once per cache window, not per visit", async () => {
  // Cloudflare's per-data-centre cache, as the worker sees it.
  const kept = new Map();
  globalThis.caches = {
    default: {
      match: async (req) => {
        const hit = kept.get(req.url);
        return hit ? hit.clone() : undefined;
      },
      put: async (req, res) => { kept.set(req.url, res.clone()); },
    },
  };
  try {
    const ctx = { waitUntil() {} };
    const visit = async (url, headers = {}) => {
      const before = lists;
      const res = await worker.fetch(new Request(url, { headers }), env, ctx);
      return { status: res.status, lists: lists - before, cors: res.headers.get("access-control-allow-origin") };
    };
    const first = await visit("https://hub.test/apps");
    assert.equal(first.status, 200);
    assert.ok(first.lists > 0, "the first gallery answer lists");
    const second = await visit("https://hub.test/apps");
    assert.equal(second.lists, 0, "the next visit is answered from the cache");
    assert.equal(second.cors, "*", "and a kept answer still carries CORS");
    assert.ok((await visit("https://hub.test/apps?q=notes")).lists > 0, "a different search is its own answer");

    assert.ok((await visit("https://hub.test/stats")).lists > 0);
    assert.equal((await visit("https://hub.test/stats")).lists, 0, "/stats is kept too");
  } finally {
    delete globalThis.caches;
  }
});

test("a stamp younger than the list lag is not taken as swept", async () => {
  const now = Date.now() + 2 * 60 * MIN;
  // Stamped thirty seconds before the run: its due mark may not be listable yet.
  KV.set("list-waiting:welcome", `${now - 30_000}-bbbbbbbb`);
  assert.equal(await tick(now), 1, "listed");
  assert.equal(await tick(now + 10 * MIN), 1, "listed again: the young stamp was not recorded as swept");
  assert.equal(await tick(now + 20 * MIN), 0, "the next sweep counted it, and the run is idle again");
});
