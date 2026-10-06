/* What a free person's tries may cost Krate, whatever they produce (2026-10-06).
 *
 * The free allowance counts only an app that was made, so tries that failed
 * were unlimited: one person's three killed builds took about $5 of the
 * Krate key in half an hour and made nothing. The hub now refuses a new
 * Krate-paid case after FREE_TRIES_PER_DAY (5) unmade cases in 24 hours, or once
 * the account's Krate-paid spend reaches FREE_SPEND_CEILING_USD. A paid plan
 * lifts both; spend on the person's own key does not count.
 *
 *   node --experimental-wasm-modules cloud/worker/test/free-tier-cost-limits.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

function env(extra = {}) {
  const kv = new Map([
    ["session:krs_pat", "u1"],
    ["user:u1", JSON.stringify({ id: "u1", login: "pat", name: "Pat", email: "pat@example.com" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    BUNDLES: r2Mock(new Map()),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
    ...extra,
  };
}
const open = (e, body = {}) => worker.fetch(new Request("https://hub.example/case/open", {
  method: "POST", headers: { authorization: "Bearer krs_pat", "content-type": "application/json" },
  body: JSON.stringify({ request: "an expense tracker", ...body }),
}), e);
const attempt = (e, id, outcome) => worker.fetch(new Request("https://hub.example/case/attempt", {
  method: "POST", headers: { authorization: "Bearer krs_pat", "content-type": "application/json" },
  body: JSON.stringify({ id, outcome }),
}), e);

/* ---- five tries that made nothing, then a sentence, not a sixth --------- */
{
  const e = env();
  for (let i = 0; i < 5; i++) {
    const r = await open(e);
    assert.strictEqual(r.status, 200, `try ${i + 1} opens: ` + (await r.clone().text()));
    await attempt(e, (await r.json()).id, "infra-failed");
  }
  const sixth = await open(e);
  assert.strictEqual(sixth.status, 429);
  const body = await sixth.json();
  assert.strictEqual(body.wall, true);
  assert.match(body.message, /Today's free tries are used/);
  assert.match(body.message, /own API key/, "it names a way on");
}

/* ---- the tries are a day's: older ones do not count ---------------------- */
{
  const e = env();
  const old = new Date(Date.now() - 30 * 60 * 60 * 1000).toISOString();
  for (let i = 0; i < 5; i++) {
    e._kv.set(`case:acct:u1:old${i}`, JSON.stringify({ id: `old${i}`, opened: old, state: "open", made: false, edit: false, attempts: [{ at: old, outcome: "stopped" }] }));
  }
  assert.strictEqual((await open(e)).status, 200, "yesterday's failures do not use today's tries");
}

/* ---- Krate-paid spend reaches the ceiling -------------------------------- */
{
  const e = env();
  e._kv.set("spend:u1", JSON.stringify([
    { at: 1, usd: 3.1, paid_by: "krate" },
    { at: 2, usd: 2.95, paid_by: "krate" },
    { at: 3, usd: 40, paid_by: "own" },
  ]));
  const r = await open(e);
  assert.strictEqual(r.status, 402);
  const body = await r.json();
  assert.strictEqual(body.wall, true);
  assert.match(body.message, /free building on this account is used up/);

  // Under the ceiling, the person's own-key spend is not counted against it.
  const e2 = env();
  e2._kv.set("spend:u1", JSON.stringify([{ at: 1, usd: 2, paid_by: "krate" }, { at: 2, usd: 50, paid_by: "own" }]));
  assert.strictEqual((await open(e2)).status, 200);
}

/* ---- the founder's settings move both limits ----------------------------- */
{
  const e = env({ FREE_TRIES_PER_DAY: "1", FREE_SPEND_CEILING_USD: "100" });
  const first = await open(e);
  await attempt(e, (await first.json()).id, "krate-failed");
  assert.strictEqual((await open(e)).status, 429, "one try a day when set to 1");
}

/* ---- a paid plan lifts both ---------------------------------------------- */
{
  const e = env();
  e._kv.set("ent:u1", JSON.stringify({ plan: "comp", active: true, until: Date.now() + 86400000 }));
  e._kv.set("spend:u1", JSON.stringify([{ at: 1, usd: 99, paid_by: "krate" }]));
  for (let i = 0; i < 4; i++) {
    const r = await open(e);
    assert.strictEqual(r.status, 200, "a plan is not limited: " + (await r.clone().text()));
    await attempt(e, (await r.json()).id, "stopped");
  }
}

console.log("free-tier cost limits: all checks passed");
