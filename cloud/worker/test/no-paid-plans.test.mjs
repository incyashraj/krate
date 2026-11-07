/* Krate has no paid plans (F-322, 2026-11-07).
 *
 * Every door the Stripe integration had -- /billing/config, checkout,
 * status, portal, the webhook, and the founding-200 list -- answers 410 in
 * words, even with the old Stripe secrets still set, and nothing reaches
 * Stripe. A referral is a count and no longer hands out a plan (which also
 * lifted the free limits on Krate's own AI key). /me names no plan.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/no-paid-plans.test.mjs
 */
import test from "node:test";
import assert from "node:assert";
import worker from "../src/index.js";

function env() {
  const kv = new Map([
    ["session:krs_alice", "u1"],
    ["user:u1", JSON.stringify({ id: "u1", login: "alice", name: "Alice", email: "alice@example.com", created: 1 })],
    ["session:krs_bob", "u2"],
    ["user:u2", JSON.stringify({ id: "u2", login: "bob", name: "Bob", email: "bob@example.com", created: Date.now() })],
    ["rcode:abcdef12", "u1"],
    // Two people already joined through Alice's link: Bob is the third, the
    // count that used to award "a month of Studio".
    ["refs:u1", JSON.stringify(["x1", "x2"])],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    PUBLIC_BASE: "https://hub.example",
    // The old secrets, still set: they must open nothing.
    STRIPE_SECRET_KEY: "sk_test_x",
    STRIPE_WEBHOOK_SECRET: "whsec_x",
    STRIPE_PRICE_MONTHLY: "price_m",
    STRIPE_PRICE_YEARLY: "price_y",
    STRIPE_PRICE_FOUNDING: "price_f",
    KRATE_BILLING_UNFROZEN: "1",
    _kv: kv,
  };
}

const call = (e, method, path, { token, body } = {}) =>
  worker.fetch(
    new Request(`https://hub.example${path}`, {
      method,
      headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
    e,
    { waitUntil() {} },
  );

test("every billing door and the founding list answer 410 in words, and nothing reaches Stripe", async () => {
  const realFetch = globalThis.fetch;
  const outside = [];
  globalThis.fetch = async (url, ...rest) => {
    outside.push(String(url));
    return realFetch(url, ...rest);
  };
  try {
    const e = env();
    const before = new Map(e._kv);
    const doors = [
      ["GET", "/billing/config"],
      ["GET", "/billing/status"],
      ["POST", "/billing/checkout", { plan: "monthly" }],
      ["POST", "/billing/portal", {}],
      ["POST", "/billing/webhook", { type: "checkout.session.completed" }],
      ["POST", "/founding", { email: "someone@example.com" }],
    ];
    for (const [method, path, body] of doors) {
      const res = await call(e, method, path, { token: "krs_alice", body });
      assert.equal(res.status, 410, `${method} ${path}`);
      const words = await res.text();
      assert.match(words, /Krate has no paid plans/, `${path} says why`);
      assert.match(words, /your own AI/, `${path} names the free way on`);
      assert.doesNotMatch(words, /\$|checkout\.stripe|price/i, `${path} quotes no price`);
    }
    assert.ok(!outside.some((u) => u.includes("stripe.com")), "nothing reached Stripe");
    assert.deepStrictEqual(e._kv, before, "nothing was written");
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("a referral is counted and gives no plan; /me names no plan", async () => {
  const e = env();
  const res = await call(e, "POST", "/referral/claim", { token: "krs_bob", body: { code: "abcdef12" } });
  assert.equal(res.status, 200);
  assert.deepStrictEqual(JSON.parse(e._kv.get("refs:u1")), ["x1", "x2", "u2"], "Bob is counted");
  assert.equal(e._kv.get("ent:u1"), undefined, "and no entitlement was written");
  assert.equal(e._kv.get("refawards:u1"), undefined);

  const me = await (await call(e, "GET", "/me", { token: "krs_alice" })).json();
  assert.equal(me.plan, undefined, "no plan in the profile");
  assert.equal(me.limits_lifted, false);
  assert.equal(me.referral.count, 3);
});
