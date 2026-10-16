/* SIEM presets for the audit export (G19): Splunk's HTTP Event Collector,
 * Microsoft Sentinel through the Logs Ingestion API, Datadog's Logs intake,
 * beside the generic signed JSON. The same queue, in chain order, with the
 * same retries; each preset's token sealed at rest and never shown back.
 *
 * Splunk is a real HTTP receiver on this machine (the hub is a local proof,
 * so http to 127.0.0.1 is allowed, as for the generic webhook). Sentinel's
 * and Datadog's hosts are fixed by the preset, so they are a fake fetch
 * that answers for those hosts only.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-audit-presets.test.mjs
 */
import { test, after, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import { createServer } from "node:http";
import { orgEnv, call, acme } from "./org-tools.mjs";
import { presetConfig, presetRequest, verifySignature } from "../src/org-audit-export.js";

const HUB = "http://127.0.0.1:8787";
const TENANT = "11111111-2222-3333-4444-555555555555";
const CLIENT = "66666666-7777-8888-9999-000000000000";
const DCR = `dcr-${"ab".repeat(16)}`;
const ENDPOINT = "https://krate-ingest-a1b2.westeurope-1.ingest.monitor.azure.com";
const HEC = "1b2c3d4e-5f60-4a7b-8c9d-0e1f2a3b4c5d";
const DD_KEY = "0123456789abcdef0123456789abcdef";
const pair = (r) => [r.status, r.body && r.body.error];

const servers = [];
after(async () => {
  for (const s of servers) await new Promise((res) => s.close(res));
});
async function splunk() {
  const r = { got: [], status: 200 };
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      r.got.push({ path: req.url, auth: req.headers.authorization, sig: req.headers["x-krate-signature"], json: JSON.parse(body), body });
      res.writeHead(r.status, { "content-type": "application/json" }).end('{"text":"Success","code":0}');
    });
  });
  await new Promise((res) => server.listen(0, "127.0.0.1", res));
  servers.push(server);
  r.base = `http://127.0.0.1:${server.address().port}`;
  return r;
}

/* Sentinel's and Datadog's hosts. */
let seen;
let realFetch;
let tokenAnswer;
beforeEach(() => {
  seen = [];
  tokenAnswer = () => new Response(JSON.stringify({ token_type: "Bearer", expires_in: 3599, access_token: "eyJ.fake.token" }), { status: 200 });
  realFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    const url = String(input && input.url ? input.url : input);
    const host = new URL(url).hostname;
    if (host === "login.microsoftonline.com") {
      seen.push({ url, body: String(init.body), headers: init.headers });
      return tokenAnswer();
    }
    if (host.endsWith(".ingest.monitor.azure.com") || host === "http-intake.logs.datadoghq.eu") {
      seen.push({ url, body: String(init.body), json: JSON.parse(init.body), headers: init.headers });
      return new Response(null, { status: host.startsWith("http-intake") ? 202 : 204 });
    }
    return realFetch(input, init);
  };
});
afterEach(() => {
  globalThis.fetch = realFetch;
});

async function world() {
  const env = orgEnv({ PUBLIC_BASE: HUB, ORG_SECRET_KEY: "org-secret-key-for-tests" });
  await acme(env);
  return { env, obj: env.ORGS.object("acme") };
}
const put = (env, body) => call(env, "alice", "PUT", "/orgs/acme/audit-export", { json: body });
const state = async (env) => (await call(env, "alice", "GET", "/orgs/acme/audit-export")).body;
const runAlarm = (obj) => {
  obj.ctx.storage.alarmAt = null;
  return obj.alarm();
};

test("each preset's fields are checked: Splunk's collector path, Sentinel's ingestion host and ids, Datadog's sites and key", () => {
  const env = { PUBLIC_BASE: "https://hub.krate.tech" };
  const s = presetConfig({ preset: "splunk", url: "https://http-inputs-acme.splunkcloud.com", token: HEC, fields: { index: "krate" } }, env);
  assert.deepStrictEqual([s.url, s.fields, s.token], ["https://http-inputs-acme.splunkcloud.com/services/collector/event", { index: "krate" }, HEC]);
  assert.strictEqual(presetConfig({ preset: "splunk", url: "https://splunk.acme.com:8088/services/collector" }, env).url, "https://splunk.acme.com:8088/services/collector/event");
  for (const bad of [
    { preset: "splunk", url: "https://splunk.acme.com/services/collector/raw" },
    { preset: "splunk", url: "https://10.0.0.5/services/collector/event" },
    { preset: "splunk", url: "http://splunk.acme.com/services/collector/event" },
    { preset: "splunk", url: "https://splunk.acme.com", token: "has spaces in it" },
    { preset: "nope", url: "https://x.example" },
  ]) {
    assert.ok(presetConfig(bad, env).problem, JSON.stringify(bad));
  }
  const fields = { endpoint: ENDPOINT, tenant_id: TENANT, client_id: CLIENT, dcr_id: DCR, stream: "Custom-KrateAudit_CL" };
  const m = presetConfig({ preset: "sentinel", fields, token: "s3cr3t~value" }, env);
  assert.strictEqual(m.url, `${ENDPOINT}/dataCollectionRules/${DCR}/streams/Custom-KrateAudit_CL?api-version=2023-01-01`);
  assert.strictEqual(m.host, new URL(ENDPOINT).hostname);
  for (const endpoint of [
    "https://evil.example",
    "https://ingest.monitor.azure.com.evil.example",
    "http://krate.westeurope-1.ingest.monitor.azure.com",
    "https://krate.westeurope-1.ingest.monitor.azure.com/elsewhere",
    "https://user:pw@krate.westeurope-1.ingest.monitor.azure.com",
    "https://krate.westeurope-1.ingest.monitor.azure.com:8443",
  ]) {
    assert.ok(presetConfig({ preset: "sentinel", fields: { ...fields, endpoint } }, env).problem, endpoint);
  }
  for (const [k, v] of [["tenant_id", "acme"], ["client_id", "x"], ["dcr_id", "dcr-12"], ["stream", "KrateAudit"]]) {
    assert.ok(presetConfig({ preset: "sentinel", fields: { ...fields, [k]: v } }, env).problem, k);
  }
  const d = presetConfig({ preset: "datadog", fields: { site: "datadoghq.eu" }, token: DD_KEY }, env);
  assert.deepStrictEqual([d.url, d.host], ["https://http-intake.logs.datadoghq.eu/api/v2/logs", "http-intake.logs.datadoghq.eu"]);
  assert.strictEqual(presetConfig({ preset: "datadog", fields: {} }, env).url, "https://http-intake.logs.datadoghq.com/api/v2/logs");
  assert.ok(presetConfig({ preset: "datadog", fields: { site: "evil.example" } }, env).problem);
  assert.ok(presetConfig({ preset: "datadog", fields: { site: "datadoghq.com" }, token: "not-a-key" }, env).problem);
  // The generic one is the webhook check it always was.
  assert.ok(presetConfig({ url: "https://siem.acme.com/krate" }, env).url);
  assert.ok(presetConfig({ url: "https://192.168.1.4/krate" }, env).problem);
});

test("the body each preset sends", () => {
  const e = { seq: 7, at: 1900000000, actor: "github:alice", action: "member.role", target: "member:github:bob", result: "allowed", reason: null, prev_hash: "a".repeat(64), hash: "b".repeat(64) };
  const s = presetRequest("splunk", { index: "sec" }, "acme", e, 1, HEC);
  assert.deepStrictEqual(JSON.parse(s.body), { time: 1900000000, host: "krate", source: "krate:acme", sourcetype: "krate:audit", index: "sec", event: { type: "krate.audit.event", v: 1, org: "acme", attempt: 1, event: e } });
  assert.deepStrictEqual(s.headers, { authorization: `Splunk ${HEC}` });
  const m = presetRequest("sentinel", {}, "acme", e, 2, "tok");
  assert.deepStrictEqual(JSON.parse(m.body), [{ TimeGenerated: "2030-03-17T17:46:40Z", Org: "acme", Seq: 7, Actor: "github:alice", Action: "member.role", Target: "member:github:bob", Result: "allowed", Reason: "", PrevHash: "a".repeat(64), Hash: "b".repeat(64), Attempt: 2 }]);
  assert.deepStrictEqual(m.headers, { authorization: "Bearer tok" });
  const d = presetRequest("datadog", {}, "acme", e, 1, DD_KEY);
  const log = JSON.parse(d.body)[0];
  assert.deepStrictEqual([log.ddsource, log.service, log.ddtags, log.timestamp, log.message], ["krate", "krate", "org:acme,action:member.role,result:allowed", 1900000000000, "github:alice member.role member:github:bob allowed"]);
  assert.deepStrictEqual(log.krate.event, e);
  assert.deepStrictEqual(d.headers, { "dd-api-key": DD_KEY });
  assert.deepStrictEqual(JSON.parse(presetRequest("generic", {}, "acme", e, 1, null).body), { type: "krate.audit.event", v: 1, org: "acme", attempt: 1, event: e });
});

test("Splunk: the HEC envelope with its token, in chain order, retried; the token sealed and never shown", async () => {
  const { env, obj } = await world();
  const r = await splunk();
  const set = await put(env, { preset: "splunk", url: r.base, token: HEC, fields: { sourcetype: "krate:audit", index: "security" } });
  assert.strictEqual(set.status, 201, JSON.stringify(set.body));
  assert.deepStrictEqual([set.body.preset, set.body.has_token, set.body.fields, set.body.url], ["splunk", true, { sourcetype: "krate:audit", index: "security" }, `${r.base}/services/collector/event`]);
  const dump = JSON.stringify(obj.sql.exec("SELECT * FROM audit_export").toArray()) + JSON.stringify(await state(env)) + JSON.stringify((await call(env, "alice", "GET", "/orgs/acme/audit")).body);
  assert.ok(!dump.includes(HEC), "the HEC token is not stored or shown in the clear");

  // The test button's event goes the same way, after the setting itself.
  r.status = 503;
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/audit-export/test")).status, 202);
  assert.deepStrictEqual(await runAlarm(obj), { delivered: 0, failed: 1, given_up: 0 });
  assert.match((await state(env)).delivery.last_error, /the receiver answered 503/);
  r.status = 200;
  obj.now = () => Math.floor(Date.now() / 1000) + 120;
  assert.deepStrictEqual(await runAlarm(obj), { delivered: 2, failed: 0, given_up: 0 });
  const got = r.got.slice(1);
  assert.deepStrictEqual(got.map((g) => [g.path, g.auth, g.json.event.event.action]), [
    ["/services/collector/event", `Splunk ${HEC}`, "audit-export.set"],
    ["/services/collector/event", `Splunk ${HEC}`, "audit-export.test"],
  ]);
  assert.deepStrictEqual([got[0].json.sourcetype, got[0].json.index, got[0].json.source, got[0].json.time], ["krate:audit", "security", "krate:acme", got[0].json.event.event.at]);
  assert.strictEqual(await verifySignature(set.body.secret, got[0].sig, got[0].body, { now: Number(/t=(\d+)/.exec(got[0].sig)[1]) }), true, "still signed");

  // Changing the fields keeps the token; another preset needs its own.
  assert.strictEqual((await put(env, { preset: "splunk", url: r.base, fields: { index: "main" } })).status, 200);
  assert.strictEqual((await state(env)).has_token, true);
  const switched = await put(env, { preset: "datadog", fields: { site: "datadoghq.eu" } });
  assert.deepStrictEqual(pair(switched), [400, "bad-request"]);
  assert.match(switched.body.message, /give the API key for Datadog/);
  // Only an owner sets it up.
  assert.deepStrictEqual(pair(await call(env, "bob", "PUT", "/orgs/acme/audit-export", { json: { preset: "splunk", url: r.base, token: HEC } })), [403, "role-cannot"]);
});

test("Sentinel: a token by the app's client credentials, then a one-row array to the rule's stream; a refused token keeps the event queued", async () => {
  const { env, obj } = await world();
  const fields = { endpoint: ENDPOINT, tenant_id: TENANT, client_id: CLIENT, dcr_id: DCR, stream: "Custom-KrateAudit_CL" };
  tokenAnswer = () => new Response(JSON.stringify({ error: "invalid_client" }), { status: 401 });
  const set = await put(env, { preset: "sentinel", fields, token: "s3cr3t~value" });
  assert.strictEqual(set.status, 201, JSON.stringify(set.body));
  assert.deepStrictEqual(set.body.fields, fields);
  assert.ok(!JSON.stringify(obj.sql.exec("SELECT * FROM audit_export").toArray()).includes("s3cr3t~value"));
  assert.deepStrictEqual(await runAlarm(obj), { delivered: 0, failed: 1, given_up: 0 });
  assert.match((await state(env)).delivery.last_error, /^Microsoft did not give a token \(401, invalid_client\) -- check the tenant id, client id and client secret$/);
  const ask = seen[0];
  assert.strictEqual(ask.url, `https://login.microsoftonline.com/${TENANT}/oauth2/v2.0/token`);
  const form = new URLSearchParams(ask.body);
  assert.deepStrictEqual([form.get("client_id"), form.get("client_secret"), form.get("scope"), form.get("grant_type")], [CLIENT, "s3cr3t~value", "https://monitor.azure.com//.default", "client_credentials"]);

  tokenAnswer = () => new Response(JSON.stringify({ token_type: "Bearer", expires_in: 3599, access_token: "eyJ.fake.token" }), { status: 200 });
  seen = [];
  obj.now = () => Math.floor(Date.now() / 1000) + 120;
  await call(env, "alice", "POST", "/orgs/acme/audit-export/test");
  assert.deepStrictEqual(await runAlarm(obj), { delivered: 2, failed: 0, given_up: 0 });
  const posts = seen.filter((x) => x.url.startsWith(ENDPOINT));
  assert.strictEqual(seen.filter((x) => x.url.includes("login.microsoftonline.com")).length, 1, "one token for the run");
  assert.deepStrictEqual(posts.map((p) => [p.url, p.headers.authorization, p.json.length, p.json[0].Action]), [
    [`${ENDPOINT}/dataCollectionRules/${DCR}/streams/Custom-KrateAudit_CL?api-version=2023-01-01`, "Bearer eyJ.fake.token", 1, "audit-export.set"],
    [`${ENDPOINT}/dataCollectionRules/${DCR}/streams/Custom-KrateAudit_CL?api-version=2023-01-01`, "Bearer eyJ.fake.token", 1, "audit-export.test"],
  ]);
  assert.deepStrictEqual(Object.keys(posts[0].json[0]), ["TimeGenerated", "Org", "Seq", "Actor", "Action", "Target", "Result", "Reason", "PrevHash", "Hash", "Attempt"]);
  // The next run reuses the token while it lasts.
  seen = [];
  await call(env, "alice", "POST", "/orgs/acme/audit-export/test");
  await runAlarm(obj);
  assert.strictEqual(seen.filter((x) => x.url.includes("login")).length, 0);
});

test("Datadog: the chosen site's intake with DD-API-KEY", async () => {
  const { env, obj } = await world();
  const set = await put(env, { preset: "datadog", fields: { site: "datadoghq.eu", service: "krate-acme" }, token: DD_KEY });
  assert.strictEqual(set.status, 201, JSON.stringify(set.body));
  assert.strictEqual(set.body.host, "http-intake.logs.datadoghq.eu");
  assert.ok(!JSON.stringify(obj.sql.exec("SELECT * FROM audit_export").toArray()).includes(DD_KEY));
  assert.deepStrictEqual(await runAlarm(obj), { delivered: 1, failed: 0, given_up: 0 });
  const p = seen[0];
  assert.deepStrictEqual([p.url, p.headers["dd-api-key"], p.json[0].service, p.json[0].ddtags], [
    "https://http-intake.logs.datadoghq.eu/api/v2/logs",
    DD_KEY,
    "krate-acme",
    "org:acme,action:audit-export.set,result:allowed",
  ]);
  assert.ok(p.headers["x-krate-signature"].startsWith("t="));
});
