/*
 * Notifications (src/org-notify.js), through the hub's real routes and
 * store: what each role is shown, worked out from the organisation's own
 * store (approvals waiting, decisions on your versions, app requests,
 * security); the shapes field by field; `since` and unread_hint; the
 * preferences on the member row; and the mail -- who is mailed for each
 * event, the preferences and the daily allowance honoured, each event
 * mailed once, never copied to the founder, nothing when the hub has no
 * mail, and an approval that never fails because a mail did. Gmail is a
 * stub: nothing here reaches it.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-notifications.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";
import { NOTIFY_DEFAULTS, MAIL_PER_ORG_PER_DAY } from "../src/org-notify.js";

const pair = (r) => [r.status, r.body && r.body.error];
const store = (env) => env.ORGS.object("acme");
const notes = async (env, who, since) => {
  const r = await call(env, who, "GET", `/orgs/acme/notifications${since ? `?since=${since}` : ""}`);
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  return r.body;
};
const kinds = (body) => body.items.map((i) => i.kind);

/* Gmail's SMTP server, as test/org-lifecycle.test.mjs stands it in. */
function fakeGmail(seen) {
  return () => {
    const enc = new TextEncoder();
    const dec = new TextDecoder();
    let toClient;
    let inData = false;
    let data = "";
    const readable = new ReadableStream({
      start(c) {
        toClient = c;
        c.enqueue(enc.encode("220 smtp.gmail.com ESMTP ready\r\n"));
      },
    });
    const answer = (line) => toClient.enqueue(enc.encode(`${line}\r\n`));
    const writable = new WritableStream({
      write(chunk) {
        const text = dec.decode(chunk);
        if (inData) {
          data += text;
          if (data.endsWith("\r\n.\r\n") || data.endsWith("\r\n.")) {
            inData = false;
            seen.mails.push({ commands: [...seen.commands], data });
            seen.commands = [];
            data = "";
            answer(seen.fail ? "552 5.3.4 Message refused" : "250 2.0.0 OK queued");
          }
          return;
        }
        const line = text.replace(/\r\n$/, "");
        seen.commands.push(line);
        if (line.startsWith("EHLO")) answer("250-smtp.gmail.com at your service\r\n250 AUTH LOGIN PLAIN");
        else if (line.startsWith("AUTH PLAIN")) answer("235 2.7.0 Accepted");
        else if (line.startsWith("MAIL FROM") || line.startsWith("RCPT TO")) answer("250 2.1.0 OK");
        else if (line === "DATA") {
          inData = true;
          answer("354 Go ahead");
        } else if (line === "QUIT") answer("221 2.0.0 closing");
      },
    });
    return { readable, writable, close: async () => {} };
  };
}
const mailText = (mail) => {
  const [, body] = mail.data.split("\r\n\r\n");
  return Buffer.from(body.replace(/\r\n\.?/g, ""), "base64").toString("utf8");
};
const subjectOf = (mail) => {
  const m = mail.data.match(/^Subject: =\?UTF-8\?B\?([^?]+)\?=/m);
  return m ? Buffer.from(m[1], "base64").toString("utf8") : "";
};
const rcpt = (mail) => mail.commands.filter((c) => c.startsWith("RCPT TO")).map((c) => c.slice(9, -1));

/* acme, erin an admin, and every GitHub login's account with a proved
 * inbox (<login>@acme.test), which is where the hub mails a GitHub member. */
async function world(extra = {}) {
  const seen = { commands: [], mails: [] };
  const env = orgEnv({ GMAIL_APP_PASSWORD: "app password", SMTP_CONNECT: fakeGmail(seen), ...extra });
  ["alice", "bob", "carol", "dan", "erin", "frank", "stranger"].forEach((login, i) => {
    env._kv.set(`ghlogin:${login}`, `u${i}`);
    env._kv.set(`user:u${i}`, JSON.stringify({ id: `u${i}`, login, name: login, avatar_url: "", providers: ["github"], verified_emails: [`${login}@acme.test`] }));
  });
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "admin" } });
  assert.strictEqual((await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`)).status, 200);
  seen.mails.length = 0;
  return { env, seen };
}

async function upload(env, name = "candidate-v1.krate", who = "carol") {
  const up = await call(env, who, "POST", "/orgs/acme/candidates", { bytes: fixture(name) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  return up.body;
}

test("whoever may approve sees what waits that they did not send, with every field the page reads", async () => {
  const { env } = await world();
  const up = await upload(env);
  const words = `${up.name} ${up.version}`;
  for (const who of ["alice", "bob"]) {
    const body = await notes(env, who);
    assert.deepStrictEqual(Object.keys(body).sort(), ["counts", "items"]);
    const item = body.items.find((i) => i.kind === "approval-waiting");
    assert.ok(item, `${who} sees the waiting version`);
    assert.deepStrictEqual(Object.keys(item).sort(), ["at", "id", "kind", "tab", "target", "text"]);
    assert.deepStrictEqual([item.text, item.tab, item.target], [`${words} waits for your approval`, "waiting", up.digest]);
    assert.strictEqual(typeof item.at, "number");
    assert.strictEqual(typeof item.id, "string");
    assert.deepStrictEqual(body.counts, { unread_hint: body.items.length });
  }
  for (const who of ["carol", "dan", "erin"]) {
    assert.ok(!kinds(await notes(env, who)).includes("approval-waiting"), `${who} does not approve, or sent it`);
  }
  assert.deepStrictEqual(pair(await call(env, "stranger", "GET", "/orgs/acme/notifications")), [403, "not-a-member"]);
  assert.deepStrictEqual(pair(await call(env, null, "GET", "/orgs/acme/notifications")), [401, "not-signed-in"]);
});

test("a sender hears their versions approved and declined, with who and why; only for 30 days", async () => {
  const { env } = await world();
  const one = await upload(env);
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${one.digest}/decline`, { json: { reason: "it asks for the camera" } })).status, 200);
  const two = await upload(env, "root-signed.krate");
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${two.digest}/approve`)).status, 200);
  const items = (await notes(env, "carol")).items.filter((i) => i.kind === "version-decided");
  const declined = items.find((i) => i.target === one.digest);
  const approved = items.find((i) => i.target === two.digest);
  assert.deepStrictEqual([declined.text, declined.tab], [`@bob declined ${one.name} ${one.version}: it asks for the camera`, "waiting"]);
  assert.deepStrictEqual([approved.text, approved.tab], [`@bob approved ${two.name} ${two.version}. It is released`, "apps"]);
  assert.ok(!kinds(await notes(env, "bob")).includes("version-decided"), "the decider does not hear their own decision");
  // Thirty-one days later it is old news.
  store(env).now = () => Math.floor(Date.now() / 1000) + 31 * 86400;
  assert.ok(!kinds(await notes(env, "carol")).includes("version-decided"));
});

test("owners, admins and developers see open app requests; security items reach owners and admins only", async () => {
  const { env } = await world();
  const asked = (await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "A visitor log for the front desk" } })).body.request;
  for (const who of ["alice", "erin", "carol"]) {
    const item = (await notes(env, who)).items.find((i) => i.kind === "request");
    assert.deepStrictEqual([item.text, item.tab, item.target], ['@dan asked for an app: "A visitor log for the front desk"', "requests", asked.id], who);
  }
  for (const who of ["bob", "dan"]) assert.ok(!kinds(await notes(env, who)).includes("request"), who);
  assert.strictEqual((await call(env, "carol", "PUT", `/orgs/acme/requests/${asked.id}`, { json: { state: "planned" } })).status, 200);
  assert.ok(!kinds(await notes(env, "alice")).includes("request"), "an answered request is not open");

  // The keyring's own states, as org-keys.js and org-insights.js keep them.
  const now = Math.floor(Date.now() / 1000);
  const sql = store(env).sql;
  sql.exec("INSERT INTO key_devices (device_id, principal, public_key, name, added_at, state) VALUES ('d1d1d1d1d1d1d1d1', 'github:carol', ?, 'Carol''s MacBook', ?, 'active')", "a".repeat(64), now - 3600);
  sql.exec("INSERT INTO key_devices (device_id, principal, public_key, name, added_at, state) VALUES ('d2d2d2d2d2d2d2d2', 'github:dan', ?, 'Old PC', ?, 'active')", "b".repeat(64), now - 9 * 86400);
  sql.exec("INSERT INTO key_versions (version, public_key, state, created_at, created_by, created_device, stale_since) VALUES (1, ?, 'stale', ?, 'github:alice', 'd1d1d1d1d1d1d1d1', ?)", "c".repeat(64), now - 86400, now - 600);
  const today = new Date(now * 1000).toISOString().slice(0, 10);
  sql.exec("INSERT INTO insights_daily (day, metric, key, n) VALUES (?, 'refusal', 'role-cannot|', 12)", today);
  for (const who of ["alice", "erin"]) {
    const sec = (await notes(env, who)).items.filter((i) => i.kind === "security");
    assert.ok(sec.every((i) => i.tab === "security"));
    const texts = sec.map((i) => i.text);
    assert.ok(texts.includes("@carol added a computer, Carol's MacBook"), `${who}: ${texts}`);
    assert.ok(!texts.some((t) => t.includes("Old PC")), "a computer added nine days ago is not news");
    assert.ok(texts.some((t) => /^The team data key is due for renewing/.test(t)), "the stale key");
    assert.ok(texts.includes("12 refused attempts this week, more than twice the usual"), "the refusal spike");
  }
  for (const who of ["bob", "carol", "dan"]) assert.ok(!kinds(await notes(env, who)).includes("security"), who);
});

test("newest first, at most 50, and unread_hint counts what is newer than the page's last look", async () => {
  const { env } = await world();
  const sql = store(env).sql;
  for (let i = 0; i < 60; i++) {
    sql.exec("INSERT INTO app_requests (id, text, asked_by, asked_at, state) VALUES (?, ?, 'github:dan', ?, 'open')", `rq_${String(i).padStart(12, "a")}`, `request ${i}`, 1000 + i);
  }
  const body = await notes(env, "alice");
  assert.strictEqual(body.items.length, 50);
  const ats = body.items.map((i) => i.at);
  assert.deepStrictEqual(ats, [...ats].sort((a, b) => b - a), "newest first");
  assert.strictEqual(body.counts.unread_hint, 50);
  assert.strictEqual((await notes(env, "alice", 1054)).counts.unread_hint, 5, "the five after the last look");
});

test("preferences: defaults, a change kept on the member row, and bad bodies refused", async () => {
  const { env } = await world();
  const got = await call(env, "bob", "GET", "/orgs/acme/me/notify");
  assert.strictEqual(got.status, 200);
  assert.deepStrictEqual(got.body, { prefs: NOTIFY_DEFAULTS });
  assert.deepStrictEqual(NOTIFY_DEFAULTS, {
    "approval-waiting": { email: true },
    "version-decided": { email: true },
    request: { email: false },
    security: { email: true },
  });
  const put = await call(env, "bob", "PUT", "/orgs/acme/me/notify", { json: { prefs: { "approval-waiting": { email: false }, request: { email: true } } } });
  assert.strictEqual(put.status, 200, JSON.stringify(put.body));
  const want = { "approval-waiting": { email: false }, "version-decided": { email: true }, request: { email: true }, security: { email: true } };
  assert.deepStrictEqual(put.body, { prefs: want });
  assert.deepStrictEqual((await call(env, "bob", "GET", "/orgs/acme/me/notify")).body, { prefs: want });
  assert.deepStrictEqual(JSON.parse(store(env).ctx.storage.db.prepare("SELECT notify FROM members WHERE principal = 'github:bob'").get().notify), want, "on the member row");
  assert.deepStrictEqual((await call(env, "alice", "GET", "/orgs/acme/me/notify")).body, { prefs: NOTIFY_DEFAULTS }, "someone else's are their own");
  for (const [body, why] of [[{}, "no prefs"], [{ prefs: [] }, "a list"], [{ prefs: { nonsense: { email: true } } }, "no such kind"], [{ prefs: { request: { email: "yes" } } }, "not a bool"], [{ prefs: { request: true } }, "not an object"]]) {
    assert.deepStrictEqual(pair(await call(env, "bob", "PUT", "/orgs/acme/me/notify", { json: body })), [400, "bad-request"], why);
  }
  assert.deepStrictEqual(pair(await call(env, "stranger", "PUT", "/orgs/acme/me/notify", { json: { prefs: {} } })), [403, "not-a-member"]);
});

test("mail: approvers hear a version waits, its sender hears the decision, each once, as their preferences say", async () => {
  const { env, seen } = await world();
  // bob turns off mail for waiting versions.
  assert.strictEqual((await call(env, "bob", "PUT", "/orgs/acme/me/notify", { json: { prefs: { "approval-waiting": { email: false } } } })).status, 200);
  const up = await upload(env);
  assert.deepStrictEqual(seen.mails.map(rcpt), [["alice@acme.test"]], "the owner who approves, not bob (off), not carol (sent it), and no copy to the founder");
  assert.strictEqual(subjectOf(seen.mails[0]), `${up.name} ${up.version} waits for your approval`);
  assert.ok(mailText(seen.mails[0]).includes("/org/?o=acme&tab=waiting"), mailText(seen.mails[0]));
  assert.match(seen.mails[0].data, /^From: Yashraj Pardeshi <incyashraj@gmail.com>/m);

  seen.mails.length = 0;
  const key = { json: { reason: "it asks for the camera" }, key: "decline-1" };
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${up.digest}/decline`, key)).status, 200);
  const replay = await call(env, "bob", "POST", `/orgs/acme/candidates/${up.digest}/decline`, key);
  assert.strictEqual(replay.status, 200, "the same request again is the same answer");
  assert.deepStrictEqual(seen.mails.map(rcpt), [["carol@acme.test"]], "the sender, once");
  assert.strictEqual(subjectOf(seen.mails[0]), `${up.name} ${up.version} was declined`);
  assert.ok(mailText(seen.mails[0]).includes("@bob declined") && mailText(seen.mails[0]).includes("it asks for the camera"));

  seen.mails.length = 0;
  const two = await upload(env, "root-signed.krate");
  seen.mails.length = 0;
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${two.digest}/approve`)).status, 200);
  assert.deepStrictEqual(seen.mails.map(rcpt), [["carol@acme.test"]]);
  assert.strictEqual(subjectOf(seen.mails[0]), `${two.name} ${two.version} is approved`);

  // An app request mails nobody by default; erin turns it on.
  seen.mails.length = 0;
  assert.strictEqual((await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "A visitor log, please" } })).status, 201);
  assert.strictEqual(seen.mails.length, 0, "off by default");
  assert.strictEqual((await call(env, "erin", "PUT", "/orgs/acme/me/notify", { json: { prefs: { request: { email: true } } } })).status, 200);
  assert.strictEqual((await call(env, "dan", "POST", "/orgs/acme/requests", { json: { text: "A rota for the weekend" } })).status, 201);
  assert.deepStrictEqual(seen.mails.map(rcpt), [["erin@acme.test"]]);

  // Nothing about mail is in the audit.
  const actions = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.map((e) => e.action);
  assert.ok(!actions.some((a) => /mail|notif/.test(a)), actions.join(" "));
});

test("an owner's own upload: they are neither shown it as waiting nor mailed about it; the other approver is", async () => {
  const { env, seen } = await world();
  const own = await upload(env, "root-signed.krate", "alice");
  assert.deepStrictEqual(seen.mails.map(rcpt), [["bob@acme.test"]], "bob approves it; alice sent it");
  assert.ok(!(await notes(env, "alice")).items.some((i) => i.target === own.digest), "not waiting on its sender");
  assert.ok((await notes(env, "bob")).items.some((i) => i.kind === "approval-waiting" && i.target === own.digest));
});

test("mail: the daily allowance holds, and a missing or failing mail never fails the approval", async () => {
  const { env, seen } = await world();
  const today = new Date().toISOString().slice(0, 10);
  store(env).sql.exec("INSERT INTO notify_mail (day, n) VALUES (?, ?)", today, MAIL_PER_ORG_PER_DAY - 1);
  await upload(env);
  assert.strictEqual(seen.mails.length, 1, "one left today: one of the two approvers is mailed");
  seen.mails.length = 0;
  const two = await upload(env, "root-signed.krate");
  assert.strictEqual(seen.mails.length, 0, "none left today");

  // Gmail refusing the mail: the approval still answers 200.
  seen.fail = true;
  const ok = await call(env, "bob", "POST", `/orgs/acme/candidates/${two.digest}/approve`);
  assert.strictEqual(ok.status, 200, JSON.stringify(ok.body));

  // No mail set up on the hub: nothing is attempted, and nothing counted.
  const bare = orgEnv();
  await acme(bare);
  const up = await call(bare, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.strictEqual(up.status, 201);
  assert.strictEqual((await call(bare, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`)).status, 200);
  assert.strictEqual(bare.ORGS.object("acme").ctx.storage.db.prepare("SELECT COUNT(*) AS n FROM notify_mail").get().n, 0);
});
