/* Notifications in Slack and Microsoft Teams (G12), through the hub's real
 * routes and the real Org store over SQLite. The channels are a fake fetch
 * that answers for Slack's and Teams' webhook hosts only, and records what
 * was posted; anything else goes where it always went, and is listed.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-chat.test.mjs
 */
import { test, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";
import { device, register } from "./org-keys-tools.mjs";
import { chatHookUrl, chatBody, CHAT_PER_ORG_PER_DAY } from "../src/org-chat.js";

const SLACK = "https://hooks.slack.com/services/T000/B000/abcdEFGHijkl9Z7q";
const TEAMS = "https://prod-12.westus.logic.azure.com:443/workflows/5f0e/triggers/manual/paths/invoke?api-version=2016-06-01&sig=Tm9pc2VXz81";
const pair = (r) => [r.status, r.body && r.body.error];

/* The fake channels: every post, by URL; `answer` says what they reply. */
let posted;
let others;
let answer;
let realFetch;
beforeEach(() => {
  posted = [];
  others = [];
  answer = () => new Response("ok", { status: 200 });
  realFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    const url = String(input && input.url ? input.url : input);
    const host = new URL(url).hostname;
    if (!/(^hooks\.slack\.com$)|(\.logic\.azure\.com$)|(\.webhook\.office\.com$)|(\.environment\.api\.powerplatform\.com$)/.test(host)) {
      // Not a channel: the hub's own business (the test stores), as ever.
      others.push(url);
      return realFetch(input, init);
    }
    posted.push({ url, body: JSON.parse(init.body), headers: init.headers });
    return answer(url);
  };
});
afterEach(() => {
  globalThis.fetch = realFetch;
});

async function world(extra = {}) {
  const env = orgEnv({ ORG_SECRET_KEY: "org-secret-key-for-tests", ...extra });
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "admin" } });
  await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  return env;
}
const add = (env, who, url, kinds = ["approval-waiting", "version-decided", "request", "security"], label) =>
  call(env, who, "POST", "/orgs/acme/chat", { json: { url, kinds, label } });
const list = async (env, who = "alice") => (await call(env, who, "GET", "/orgs/acme/chat")).body;
const textOf = (p) => (p.body.text !== undefined ? p.body.text : p.body.attachments[0].content.body[0].text);

test("only Slack's and Teams' own webhook hosts are taken, on https and port 443, with nothing else in the URL", async () => {
  for (const ok of [
    SLACK,
    "https://hooks.slack.com/triggers/T000/123/abc",
    "https://hooks.slack.com/workflows/T000/A1/1/abc",
    "https://contoso.webhook.office.com/webhookb2/a@b/IncomingWebhook/c/d",
    TEAMS,
    "https://default8e1c.12.environment.api.powerplatform.com:443/powerautomate/automations/direct/workflows/ab/triggers/manual/paths/invoke?sig=x",
  ]) {
    assert.ok(chatHookUrl(ok).service, ok);
  }
  assert.deepStrictEqual([chatHookUrl(SLACK).service, chatHookUrl(TEAMS).service], ["slack", "teams"]);
  for (const bad of [
    "",
    "http://hooks.slack.com/services/T/B/x",
    "https://hooks.slack.com.evil.example/services/T/B/x",
    "https://evil.example/hooks.slack.com/services/T/B/x",
    "https://evil.example/?u=https://hooks.slack.com/services/T/B/x",
    "https://hooks.slack.com/api/chat.postMessage",
    "https://user:pw@hooks.slack.com/services/T/B/x",
    "https://hooks.slack.com:8443/services/T/B/x",
    "https://hooks.slack.com/services/T/B/x#frag",
    "https://slack.com/services/T/B/x",
    "https://webhook.office.com/webhookb2/x",
    "https://a.b.webhook.office.com/webhookb2/x",
    "https://webhook.office.com.evil.example/webhookb2/x",
    "https://logic.azure.com/workflows/x",
    "https://evil.logic.azure.com.attacker.example/workflows/x",
    "https://127.0.0.1/services/T/B/x",
    "https://169.254.169.254/latest/meta-data",
    "https://[::1]/services/x",
    "https://localhost/services/x",
    "https://hooks.slack.com./services/T/B/x",
    "javascript:alert(1)",
    `https://hooks.slack.com/services/${"x".repeat(2000)}`,
  ]) {
    assert.ok(chatHookUrl(bad).problem, bad);
  }

  // Through the route: refused before anything is stored or sent.
  const env = await world();
  for (const bad of ["https://evil.example/services/T/B/x", "http://hooks.slack.com/services/T/B/x", "https://127.0.0.1/hook"]) {
    const r = await add(env, "alice", bad);
    assert.deepStrictEqual(pair(r), [400, "bad-request"], bad);
    assert.match(r.body.message, /webhook URL from Slack/);
  }
  assert.deepStrictEqual((await list(env)).hooks, []);
  assert.strictEqual(posted.length, 0);
});

test("a channel's URL is sealed at rest and never shown back: the page reads its host and last four", async () => {
  const env = await world();
  const r = await add(env, "erin", SLACK, ["approval-waiting"], "#apps");
  assert.strictEqual(r.status, 201, JSON.stringify(r.body));
  const h = r.body.hook;
  assert.deepStrictEqual([h.service, h.host, h.last4, h.label, h.kinds], ["slack", "hooks.slack.com", "9Z7q", "#apps", ["approval-waiting"]]);
  assert.strictEqual(h.url, undefined);
  const store = env.ORGS.object("acme");
  const dump = JSON.stringify(store.rows("SELECT * FROM chat_hooks")) + JSON.stringify(store.rows("SELECT * FROM audit"));
  for (const part of [SLACK, "abcdEFGHijkl9Z7q", "T000/B000"]) assert.ok(!dump.includes(part), `stored in the clear: ${part}`);
  assert.ok(!JSON.stringify(await list(env)).includes("abcdEFGHijkl"));
  // The audit names the host.
  const ev = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.find((e) => e.action === "chat.add");
  assert.deepStrictEqual([ev.actor, ev.target, ev.reason], ["github:erin", "chat:hooks.slack.com", "approval-waiting"]);

  // Owners and admins only.
  for (const who of ["bob", "carol", "dan"]) {
    assert.deepStrictEqual(pair(await call(env, who, "GET", "/orgs/acme/chat")), [403, "role-cannot"], who);
    assert.deepStrictEqual(pair(await add(env, who, SLACK)), [403, "role-cannot"], who);
  }
  assert.deepStrictEqual(pair(await add(env, "stranger", SLACK)), [403, "not-a-member"]);
  // Kinds are the four there are.
  assert.deepStrictEqual(pair(await add(env, "alice", SLACK, ["everything"])), [400, "bad-request"]);
  assert.deepStrictEqual(pair(await add(env, "alice", SLACK, [])), [400, "bad-request"]);

  // Without ORG_SECRET_KEY there is nothing to seal with.
  const bare = orgEnv({});
  await acme(bare);
  assert.deepStrictEqual(pair(await add(bare, "alice", SLACK)), [503, "chat-off"]);
});

test("each kind posts where it was asked for, once, with a link to the console page; Teams gets a card", async () => {
  const env = await world();
  const s = await add(env, "alice", SLACK, ["approval-waiting", "version-decided", "request"]);
  const t = await add(env, "alice", TEAMS, ["security", "version-decided"]);
  assert.strictEqual(s.status, 201);
  assert.strictEqual(t.status, 201);
  assert.ok(env._kv.has("org-chat:acme"), "the nightly run knows to look");

  // A version waits: Slack only.
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.strictEqual(up.status, 201);
  assert.strictEqual(posted.length, 1);
  assert.ok(posted[0].url.startsWith("https://hooks.slack.com/services/"));
  assert.match(textOf(posted[0]), /^Bounce 1\.0\.0 waits for approval in Acme\. @carol sent it\. <https:\/\/[^|]+\/org\/\?o=acme&tab=waiting\|Open in Krate>$/);

  // Approved: both (both asked for decisions). Teams' is an Adaptive Card with the link as its button.
  posted = [];
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`)).status, 200);
  assert.strictEqual(posted.length, 2);
  const card = posted.find((p) => p.url.includes("logic.azure.com")).body;
  assert.strictEqual(card.attachments[0].contentType, "application/vnd.microsoft.card.adaptive");
  assert.strictEqual(card.attachments[0].content.body[0].text, "@bob approved Bounce 1.0.0 in Acme. Everyone it is for can open it now.");
  assert.match(card.attachments[0].content.actions[0].url, /\/org\/\?o=acme&tab=releases$/);

  // Withdrawn: both again.
  posted = [];
  assert.strictEqual((await call(env, "alice", "POST", `/orgs/acme/releases/${up.body.digest}/withdraw`, { json: { reason: "loses <data> & more" } })).status, 200);
  assert.strictEqual(posted.length, 2);
  // Slack's text is escaped the way Slack asks.
  assert.match(posted.find((p) => p.url.includes("slack")).body.text, /withdrew Bounce 1\.0\.0 in Acme: loses &lt;data&gt; &amp; more/);

  // An app request: Slack.
  posted = [];
  assert.strictEqual((await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "A rota for the front desk" } })).status, 201);
  assert.deepStrictEqual(posted.map((p) => textOf(p).split(" <")[0]), ['@dan asked Acme for an app: "A rota for the front desk"']);

  // A computer added: Teams (security), once.
  posted = [];
  const mac = device();
  assert.strictEqual((await register(env, "carol", mac, "Carol's Mac")).status, 200);
  assert.deepStrictEqual(posted.map((p) => textOf(p)), ["@carol added a computer to Acme: Carol's Mac."]);
  posted = [];
  await register(env, "carol", mac, "Carol's Mac");
  await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "Another app" } });
  assert.ok(!posted.some((p) => /added a computer/.test(textOf(p))), "each item once");

  // Recorded per channel; a post is not audited, a test is not either a change.
  const hooks = (await list(env)).hooks;
  assert.ok(hooks.every((h) => h.last_ok_at && !h.last_error));
  assert.ok(!(await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.some((e) => e.action === "chat.post"));
});

test("a channel that fails never fails the action; the page says what went wrong; Send a test message", async () => {
  const env = await world();
  const s = (await add(env, "alice", SLACK)).body.hook;
  answer = () => new Response("no_service", { status: 404 });
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.strictEqual(up.status, 201, "the upload is answered as ever");
  answer = () => {
    throw new TypeError("network down");
  };
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`)).status, 200);
  const h = (await list(env)).hooks[0];
  assert.match(h.last_error, /could not be reached \(network down\)/);

  // The test button: what it posted, or why not.
  answer = () => new Response("gone", { status: 410 });
  const failed = await call(env, "alice", "POST", `/orgs/acme/chat/${s.id}/test`);
  assert.deepStrictEqual(pair(failed), [502, "chat-failed"]);
  assert.match(failed.body.message, /the test did not arrive: the channel's webhook answered 410: it may have been removed in Slack -- add it again/);
  answer = () => new Response("ok", { status: 200 });
  posted = [];
  const ok = await call(env, "erin", "POST", `/orgs/acme/chat/${s.id}/test`);
  assert.deepStrictEqual([ok.status, ok.body.posted], [200, true]);
  assert.match(textOf(posted[0]), /^Krate is connected: Acme posts here when a version waits for approval, a version is approved, declined or withdrawn, someone asks for an app, something about security needs a look\./);
  assert.strictEqual((await list(env)).hooks[0].last_error, null);
  assert.deepStrictEqual(pair(await call(env, "carol", "POST", `/orgs/acme/chat/${s.id}/test`)), [403, "role-cannot"]);
  assert.deepStrictEqual(pair(await call(env, "alice", "POST", "/orgs/acme/chat/nope/test")), [404, "no-such-channel"]);

  // Change what it posts, then remove it.
  const ch = await call(env, "alice", "PUT", `/orgs/acme/chat/${s.id}`, { json: { kinds: ["security"] } });
  assert.deepStrictEqual(ch.body.hook.kinds, ["security"]);
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/chat/${s.id}`)).status, 204);
  assert.ok(!env._kv.has("org-chat:acme"));
  const did = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.map((e) => e.action);
  for (const a of ["chat.add", "chat.test", "chat.change", "chat.remove"]) assert.ok(did.includes(a), a);
});

test(`at most ${CHAT_PER_ORG_PER_DAY} posts a day per organisation, counted in its store`, async () => {
  const env = await world();
  const s = (await add(env, "alice", SLACK)).body.hook;
  const store = env.ORGS.object("acme");
  const day = store.orgDay(store.now());
  store.sql.exec("INSERT INTO chat_posts (day, n) VALUES (?, ?)", day, CHAT_PER_ORG_PER_DAY - 1);
  await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "a rota, one" } });
  await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "a rota, two" } });
  assert.deepStrictEqual(posted.map((p) => textOf(p).split(" <")[0]), ['@dan asked Acme for an app: "a rota, one"']);
  assert.strictEqual((await list(env)).used_today, CHAT_PER_ORG_PER_DAY);
  const t = await call(env, "alice", "POST", `/orgs/acme/chat/${s.id}/test`);
  assert.strictEqual(t.status, 429);
  assert.match(t.body.message, /posted 50 times today, the most in one day/);
  // A new day has room again.
  store.now = () => Math.floor(Date.now() / 1000) + 86400 * 2;
  await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "a rota, three" } });
  assert.strictEqual(posted.length, 2);
});

test("the nightly run posts a refusal spike once a week to channels that asked for security", async () => {
  const env = await world();
  await add(env, "alice", TEAMS, ["security"]);
  const store = env.ORGS.object("acme");
  const day = store.orgDay(store.now());
  store.sql.exec("INSERT INTO insights_daily (day, metric, key, n) VALUES (?, 'refusal', 'not-a-member|', 25)", day);
  const { chatSweep } = await import("../src/org-chat.js");
  const ask = (e, slug, command) => e.ORGS.get(e.ORGS.idFromName(`org:${slug}`)).fetch("https://org/", { method: "POST", body: JSON.stringify(command) }).then((r) => r.json());
  assert.strictEqual(await chatSweep(env, ask, "https://krate.tech"), 1);
  assert.match(textOf(posted[0]), /^25 refused attempts in Acme this week, more than twice the usual\.$/);
  assert.match(posted[0].body.attachments[0].content.actions[0].url, /^https:\/\/krate\.tech\/org\/\?o=acme&tab=security$/);
  assert.strictEqual(await chatSweep(env, ask, "https://krate.tech"), 0, "once");
});

test("the body each service is sent", () => {
  assert.deepStrictEqual(chatBody("slack", "a <b>", "https://x/y"), { text: "a &lt;b&gt; <https://x/y|Open in Krate>" });
  const t = chatBody("teams", "hello", "https://x/y");
  assert.deepStrictEqual(t.attachments[0].content.actions, [{ type: "Action.OpenUrl", title: "Open in Krate", url: "https://x/y" }]);
});
