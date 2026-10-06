/* The desk shows what someone sent, with the session it is about (2026-10-06).
 *
 * A person making an app in web Studio hit a problem and sent it to
 * support. The desk showed a line of text and nothing else: a ticket kept no
 * session, Studio's reports (a zip with the session, version and OS) were
 * stored but listed nowhere, and there was no way to read the conversation
 * the person had with Studio. Now a ticket can name its session, reports
 * have their own tab, and the desk opens a person's sessions beside either.
 *
 * Driven against the real worker.
 *
 *   node --experimental-wasm-modules cloud/worker/test/desk-reports-sessions.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

function env() {
  const kv = new Map([
    ["session:krs_admin", "a1"],
    ["user:a1", JSON.stringify({ id: "a1", login: "boss", name: "Boss" })],
    ["session:krs_sam", "u7"],
    ["user:u7", JSON.stringify({ id: "u7", login: "sam", name: "Sam", email: "sam@example.com" })],
    ["sess:u7:s-weather", JSON.stringify({ id: "s-weather", title: "a weather app", updated: 1791300000, messages: [
      { who: "YOU", body: "a weather app with a 5 day forecast", when: 1791299000 },
      { who: "KRATE", body: "The build failed: net.connect is not granted", when: 1791299900 },
    ] })],
    ["sess:u7:s-older", JSON.stringify({ id: "s-older", title: "a timer", updated: 1791200000, messages: [] })],
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
const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
const as = (token, path, init = {}) => req(path, { ...init, headers: { authorization: `Bearer ${token}`, "content-type": "application/json", ...(init.headers || {}) } });

const e = env();

/* ---- a ticket names the session it is about ---------------------------- */
{
  const r = await worker.fetch(as("krs_sam", "/support/new", { method: "POST", body: JSON.stringify({ subject: "build failed", text: "it says net.connect", session: "s-weather" }) }), e);
  assert.strictEqual(r.status, 200, await r.clone().text());
  const t = JSON.parse(e._kv.get(`tick:${JSON.parse(await r.text()).id}`));
  assert.strictEqual(t.session, "s-weather");
  const bad = await worker.fetch(as("krs_sam", "/support/new", { method: "POST", body: JSON.stringify({ subject: "x", text: "y", session: "../../etc" }) }), e);
  const t2 = JSON.parse(e._kv.get(`tick:${JSON.parse(await bad.text()).id}`));
  assert.strictEqual(t2.session, "", "a session id that is not an id is dropped, not stored");
}

/* ---- the desk lists the person's sessions, by account id or login ------- */
{
  const byId = await (await worker.fetch(as("krs_admin", "/admin/api/person/sessions?id=u7"), e)).json();
  assert.deepStrictEqual(byId.sessions.map((s) => s.id), ["s-weather", "s-older"], "newest first");
  assert.match(byId.sessions[0].messages[1].body, /net\.connect/, "the whole conversation comes back");
  const byLogin = await (await worker.fetch(as("krs_admin", "/admin/api/person/sessions?login=SAM"), e)).json();
  assert.strictEqual(byLogin.sessions.length, 2, "a report's login finds the same account");
  const notAdmin = await worker.fetch(as("krs_sam", "/admin/api/person/sessions?id=u7"), e);
  assert.strictEqual(notAdmin.status, 404, "only admins can read anyone's sessions");
}

/* ---- reports have a tab, and a state ------------------------------------ */
{
  const zip = new Uint8Array([0x50, 0x4b, 3, 4, 1, 2, 3, 4, 5]);
  const sent = await worker.fetch(req("/report", { method: "POST", body: zip, headers: { authorization: "Bearer krs_sam", "content-length": String(zip.length), "x-krate-session": "s-weather", "x-krate-version": "0.5.4", "x-krate-os": "web", "x-krate-note": "it stopped at the build" } }), e);
  assert.strictEqual(sent.status, 200, await sent.clone().text());
  const { id } = await sent.json();
  const list = await (await worker.fetch(as("krs_admin", "/admin/api/reports"), e)).json();
  assert.strictEqual(list.reports.length, 1);
  assert.strictEqual(list.reports[0].session, "s-weather");
  assert.strictEqual(list.reports[0].from, "sam");
  const mark = await worker.fetch(as("krs_admin", "/admin/api/report/state", { method: "POST", body: JSON.stringify({ id, state: "done" }) }), e);
  assert.strictEqual(mark.status, 200);
  assert.strictEqual(JSON.parse(e._kv.get(`report:${id}`)).state, "done");
  const zipBack = await worker.fetch(as("krs_admin", `/admin/report/${id}`), e);
  assert.strictEqual(zipBack.status, 200, "the evidence downloads for an admin");
  const overview = await (await worker.fetch(as("krs_admin", "/admin/api/overview"), e)).json();
  assert.strictEqual(overview.reports, 1);
}

/* ---- the desk page carries the tab, and its script parses ---------------- */
{
  const page = await (await worker.fetch(req("/admin"), e)).text();
  assert.match(page, /data-t="reports"/, "the Reports tab is on the desk");
  const scripts = [...page.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  assert.ok(scripts.length > 0);
  for (const code of scripts) new Function(code); // throws on a syntax error
  assert.match(page, /sessionsInto/, "tickets and reports can open the sessions");
  assert.doesNotMatch(page, /data-t="payments"/, "no Payments tab: billing is not live");
  assert.match(page, /How sign-ins went, 7 days/, "sign-ins are broken down by way in");
  assert.match(page, /attempts, not people/, "a sign-in is labelled as an attempt");
  const people = await (await worker.fetch(as("krs_admin", "/admin/api/people?days=7"), e)).json();
  assert.ok(Array.isArray(people.signin_ways_7d), "the people view carries the sign-in breakdown");
  assert.strictEqual(people.accounts_total, 2, "accounts are people: one record each");
}

/* ---- the welcome is remembered on the account, written once ------------- */
{
  const before = JSON.parse((await (await worker.fetch(as("krs_sam", "/me"), e)).text())).user.onboarded;
  assert.strictEqual(before, false);
  let writes = 0;
  const put = e.APPS.put;
  e.APPS.put = async (k, v) => { if (k === "user:u7") writes++; return put(k, v); };
  assert.strictEqual((await worker.fetch(as("krs_sam", "/me/onboarded", { method: "POST", body: "{}" }), e)).status, 200);
  assert.strictEqual((await worker.fetch(as("krs_sam", "/me/onboarded", { method: "POST", body: "{}" }), e)).status, 200);
  assert.strictEqual(writes, 1, "one write, the first time only");
  const after = JSON.parse((await (await worker.fetch(as("krs_sam", "/me"), e)).text())).user.onboarded;
  assert.strictEqual(after, true);
  assert.strictEqual((await worker.fetch(req("/me/onboarded", { method: "POST" }), e)).status, 401, "only for a signed-in person");
  e.APPS.put = put;
}

console.log("desk reports and sessions: all checks passed");
