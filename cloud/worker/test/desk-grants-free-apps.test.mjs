/* The desk can give one person N free apps, and the money still has an end.
 *
 * The free allowance is one app and one change, and the only way past it
 * was the "comp" override, which lifts every limit with no ceiling at all.
 * On 2026-11-23 the founder promised an early user five free apps after
 * two of his builds failed on our side: a number, not a blank cheque. The
 * grant raises the make and change allowances to N, scales the spend
 * ceiling by N, keeps the daily tries limit, and shows on /me, the case
 * listing and the desk.
 *
 *   node --experimental-wasm-modules cloud/worker/test/desk-grants-free-apps.test.mjs
 */
import assert from "node:assert/strict";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";
import "./github-stub.mjs";

function env() {
  const kv = new Map([
    ["session:krs_admin", "a1"],
    ["user:a1", JSON.stringify({ id: "a1", login: "boss", name: "Boss" })],
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
    KRATE_ADMINS: "boss",
    _kv: kv,
  };
}
const call = (e, token, path, body) => worker.fetch(new Request(`https://hub.example${path}`, {
  method: body === undefined ? "GET" : "POST",
  headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
  ...(body === undefined ? {} : { body: JSON.stringify(body) }),
}), e);
const open = (e, body = {}) => call(e, "krs_pat", "/case/open", { request: "an app", ...body });
const made = async (e, body = {}) => {
  const r = await open(e, body);
  if (r.status !== 200) return r;
  await call(e, "krs_pat", "/case/attempt", { id: (await r.json()).id, outcome: "made" });
  return r;
};
const grant = (e, token, apps) => call(e, token, "/admin/api/user/limits", { id: "u1", apps });

/* ---- without a grant: one app, then the wall ----------------------------- */
{
  const e = env();
  assert.equal((await made(e)).status, 200);
  assert.equal((await open(e)).status, 402, "the second app meets the wall");
  console.log("ok  one free app without a grant");
}

/* ---- only the desk can grant, and only a sensible number ---------------- */
{
  const e = env();
  assert.equal((await grant(e, "krs_pat", 5)).status, 404, "a person cannot grant themselves apps");
  assert.equal((await grant(e, "gho_boss", 1)).status, 400, "1 is not a grant");
  assert.equal((await grant(e, "gho_boss", 51)).status, 400, "nor is 51");
  assert.equal((await grant(e, "gho_boss", 2.5)).status, 400, "nor a fraction");
  assert.equal(e._kv.get("ent:u1"), undefined, "nothing was written");
  console.log("ok  only the desk grants, from 2 to 50");
}

/* ---- five apps: five made, each with its change, then the wall --------- */
{
  const e = env();
  const r = await grant(e, "gho_boss", 5);
  assert.equal(r.status, 200, await r.clone().text());
  const ent = JSON.parse(e._kv.get("ent:u1"));
  assert.equal(ent.plan, "apps");
  assert.equal(ent.apps, 5);
  assert.equal(ent.by, "boss");

  for (let i = 1; i <= 5; i++) assert.equal((await made(e)).status, 200, `app ${i} of 5`);
  const sixth = await open(e);
  assert.equal(sixth.status, 402, "the sixth app meets the wall");
  assert.match((await sixth.json()).message, /free app is used/);

  for (let i = 1; i <= 5; i++) assert.equal((await made(e, { edit: true })).status, 200, `change ${i} of 5`);
  assert.equal((await open(e, { edit: true })).status, 402, "and the sixth change");
  console.log("ok  a grant of five is five apps and five changes, then the wall");

  const me = await (await call(e, "krs_pat", "/me")).json();
  assert.equal(me.free_apps, 5, "/me says five");
  assert.equal(me.limits_lifted, false, "a counted grant is not the unlimited override");
  const listed = await (await call(e, "krs_pat", "/case/list", {})).json();
  assert.equal(listed.limit, 5, "the case listing agrees with the wall");
  console.log("ok  /me and the case listing say five");
}

/* ---- the spend ceiling is per app ----------------------------------------- */
{
  const e = env();
  // $6.14 of Krate-paid building: over one app's ceiling ($6), under five's.
  e._kv.set("spend:u1", JSON.stringify([
    { at: 1, usd: 2.5825, paid_by: "krate" }, { at: 2, usd: 2.58, paid_by: "krate" }, { at: 3, usd: 0.9762, paid_by: "krate" },
  ]));
  assert.equal((await open(e)).status, 402, "without the grant, $6.14 is past the ceiling");
  await grant(e, "gho_boss", 5);
  assert.equal((await open(e)).status, 200, "with five apps, it is not");
  e._kv.set("spend:u1", JSON.stringify([{ at: 1, usd: 30.5, paid_by: "krate" }]));
  assert.equal((await open(e)).status, 402, "and five apps' worth ($30) still ends it");
  console.log("ok  the spend ceiling scales with the grant and still ends");
}

/* ---- the desk can take it back ------------------------------------------- */
{
  const e = env();
  await grant(e, "gho_boss", 5);
  await call(e, "gho_boss", "/admin/api/user/limits", { id: "u1", lifted: false });
  assert.equal(e._kv.get("ent:u1"), undefined);
  const me = await (await call(e, "krs_pat", "/me")).json();
  assert.equal(me.free_apps, 1);
  console.log("ok  the desk can take a grant back");
}
