/* A way out of Krate's non-essential mail, and the founder's announcements.
 *
 * The switch on the account (mail-prefs.js), the link in every such mail
 * (GET only asks, POST does it, the RFC 8058 one-click POST does it too),
 * the setting route, every non-essential sender skipping a person who turned
 * it off while essential mail still goes, and the announcement sender: dry
 * run, a real run through a stand-in, once per person, queued past the
 * day's ceiling, and refused without the admin's GitHub sign-in.
 *
 * Resend is a stand-in: nothing here sends real mail.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/mail-unsubscribe.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, tokenFor, worker } from "./org-tools.mjs";
import { tellPublished, tellMade, tellFirstOpen } from "../src/mail.js";
import { mailInvite } from "../src/org-mail.js";
import { unsubscribeToken, unsubscribeLink, tokenValid } from "../src/mail-prefs.js";
import { renderAnnouncement, runQueuedAnnouncements } from "../src/mail-announce.js";

const here = dirname(fileURLToPath(import.meta.url));
const ported = (name) => new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", name)));

/* Resend, standing in: every mail it is handed; `refuse` makes it say no. */
function stubResend() {
  const sent = [];
  const real = globalThis.fetch;
  const stub = { sent, refuse: null };
  globalThis.fetch = async (input, init = {}) => {
    if (String(input.url || input) === "https://api.resend.com/emails") {
      const mail = JSON.parse(init.body);
      if (stub.refuse && stub.refuse(mail)) return new Response("refused", { status: 422 });
      sent.push(mail);
      return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
    }
    return real(input, init);
  };
  stub.restore = () => {
    globalThis.fetch = real;
  };
  return stub;
}

const hubEnv = (extra = {}) => {
  const env = orgEnv({
    RESEND_API_KEY: "re_test",
    VISITOR_SALT: "salt",
    PUBLIC_BASE: "https://hub.example",
    KEY_WRAP_SECRET: "wrap-secret",
    KRATE_ADMINS: "boss",
    ANNOUNCE_GAP_MS: "0",
    ...extra,
  });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

/* An email account (tokenFor makes it) with a name; answers its id. */
function person(env, email, name = "") {
  tokenFor(env, email);
  const id = `mail-${email.replace(/[^a-z0-9]/gi, "-")}`;
  const user = JSON.parse(env._kv.get(`user:${id}`));
  user.name = name;
  user.created = Date.now();
  env._kv.set(`user:${id}`, JSON.stringify(user));
  return id;
}
/* A signed-in request as account `id` (tokenFor would reset the record). */
const as = (env, id, method, path, opts = {}) => call(env, null, method, path, { ...opts, headers: { ...(opts.headers || {}), authorization: `Bearer krs_${id}` } });
const userOf = (env, id) => JSON.parse(env._kv.get(`user:${id}`));
const turnOff = (env, id) => env._kv.set(`user:${id}`, JSON.stringify({ ...userOf(env, id), mail_news: false }));

/* One raw request to the worker. */
async function raw(env, method, path, { body, headers = {} } = {}) {
  const res = await worker.fetch(new Request(`https://hub.example${path}`, { method, headers, body }), env);
  return { status: res.status, text: await res.text(), headers: res.headers };
}
const form = (fields) => ({ body: new URLSearchParams(fields).toString(), headers: { "content-type": "application/x-www-form-urlencoded" } });
const pathOf = (link) => link.slice("https://hub.example".length);
/* The link a mail carries, from its last line. */
const linkIn = (text) => /Unsubscribe: (\S+)\s*$/.exec(text)[1];

test("the link's token is the account's own: right for it, wrong for anyone else, and none without the secret", async () => {
  const env = hubEnv();
  const t = await unsubscribeToken(env, "acct-a");
  assert.match(t, /^[0-9a-f]{32}$/);
  assert.equal(await tokenValid(env, "acct-a", t), true);
  assert.equal(await tokenValid(env, "acct-b", t), false, "another account's token does not work");
  assert.equal(await tokenValid(env, "acct-a", t.replace(/^./, (c) => (c === "0" ? "1" : "0"))), false, "one character off");
  assert.equal(await tokenValid(env, "acct-a", ""), false);
  assert.equal(await tokenValid(env, "acct-a", t.slice(0, 31)), false, "a link copied only in part");
  assert.equal(await tokenValid({ ...env, KEY_WRAP_SECRET: "another" }, "acct-a", t), false, "made under another secret");
  assert.equal(await unsubscribeToken({ ...env, KEY_WRAP_SECRET: "" }, "acct-a"), null);
  // Domain separation: not the bare HMAC of the id under the secret.
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode("wrap-secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const bare = Buffer.from(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode("acct-a"))).toString("hex").slice(0, 32);
  assert.notEqual(t, bare);
});

test("GET asks and changes nothing; POST stops the emails, and Undo brings them back", async () => {
  const env = hubEnv();
  const id = person(env, "ada@example.com", "Ada");
  const link = await unsubscribeLink(env, id);
  assert.ok(link.startsWith(`https://hub.example/mail/unsubscribe?u=${id}&t=`), link);

  // A mail scanner opening the link, three times over.
  for (let i = 0; i < 3; i++) {
    const page = await raw(env, "GET", pathOf(link));
    assert.equal(page.status, 200);
    assert.ok(page.text.includes("Stop emails from Krate?"), page.text);
    assert.ok(page.text.includes('<form method="post">') && page.text.includes('value="stop"'));
    assert.ok(page.text.includes("https://krate.tech/account/#mail"), "a link back to the settings");
  }
  assert.equal(userOf(env, id).mail_news, undefined, "a GET never unsubscribes");

  const done = await raw(env, "POST", pathOf(link), form({ do: "stop" }));
  assert.equal(done.status, 200);
  assert.ok(done.text.includes("Done.") && done.text.includes("You won't get these emails any more. Sign-in links and team invitations still arrive."), done.text);
  assert.ok(done.text.includes('value="resume"') && done.text.includes(">Undo<"), "an undo button");
  assert.equal(userOf(env, id).mail_news, false);
  assert.ok((await raw(env, "GET", pathOf(link))).text.includes("These emails are off"));

  const undone = await raw(env, "POST", pathOf(link), form({ do: "resume" }));
  assert.ok(undone.text.includes("Emails are back on"), undone.text);
  assert.equal(userOf(env, id).mail_news, undefined);

  // A wrong link changes nothing and says so.
  const bad = await raw(env, "POST", `/mail/unsubscribe?u=${id}&t=${"0".repeat(32)}`, form({ do: "stop" }));
  assert.equal(bad.status, 400);
  assert.equal(userOf(env, id).mail_news, undefined);
  const badPage = await raw(env, "GET", `/mail/unsubscribe?u=someone-else&t=${link.split("t=")[1]}`);
  assert.equal(badPage.status, 400);
  assert.ok(badPage.text.includes("This link does not work"));
});

test("the one-click POST a mail app sends (RFC 8058) unsubscribes too, and writes only when it changes", async () => {
  const env = hubEnv();
  const id = person(env, "bo@example.com");
  const link = await unsubscribeLink(env, id);
  let puts = 0;
  const put = env.APPS.put;
  env.APPS.put = async (k, v, o) => {
    if (k === `user:${id}`) puts += 1;
    return put(k, v, o);
  };
  const one = await raw(env, "POST", pathOf(link), form({ "List-Unsubscribe": "One-Click" }));
  assert.equal(one.status, 200);
  assert.equal(one.text, "unsubscribed");
  assert.equal(userOf(env, id).mail_news, false);
  await raw(env, "POST", pathOf(link), form({ "List-Unsubscribe": "One-Click" }));
  assert.equal(puts, 1, "the second one-click finds it already off and writes nothing");
  // A bare POST with nothing in it does nothing.
  await raw(env, "POST", pathOf(link), form({}));
  assert.equal(puts, 1);
});

test("the setting: GET and PUT /me/mail for the signed-in account, shown in /me, refused signed out", async () => {
  const env = hubEnv();
  const id = person(env, "cy@example.com");
  assert.equal((await call(env, null, "GET", "/me/mail")).status, 401);
  assert.deepEqual((await as(env, id, "GET", "/me/mail")).body, { news: true });
  assert.equal((await as(env, id, "GET", "/me")).body.mail.news, true);
  const off = await as(env, id, "PUT", "/me/mail", { json: { news: false } });
  assert.deepEqual(off.body, { news: false });
  assert.equal(off.headers.get("access-control-allow-origin"), "*", "krate.tech/account calls it from another origin");
  assert.equal(userOf(env, id).mail_news, false);
  assert.equal((await as(env, id, "GET", "/me")).body.mail.news, false);
  assert.equal((await as(env, id, "PUT", "/me/mail", { json: { news: "no" } })).status, 400);
  // Back on from the settings after unsubscribing by link.
  assert.deepEqual((await as(env, id, "PUT", "/me/mail", { json: { news: true } })).body, { news: true });
  assert.equal(userOf(env, id).mail_news, undefined);
});

test("every app mail ends with the unsubscribe line, carries the one-click headers, and skips a person who turned it off", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const id = person(env, "ada@example.com", "Ada");
    // Through the real publish door.
    const up = await call(env, "ada@example.com", "POST", "/publish", { bytes: ported("bounce.krate"), headers: { "x-krate-name": "Bounce" } });
    assert.equal(up.status, 200, JSON.stringify(up.body));
    assert.equal(mail.sent.length, 1);
    const [m] = mail.sent;
    const link = await unsubscribeLink(env, id);
    assert.ok(m.text.endsWith(`\n\nDon't want these emails? Unsubscribe: ${link}`), m.text.slice(-160));
    assert.deepEqual(m.headers, { "List-Unsubscribe": `<${link}>`, "List-Unsubscribe-Post": "List-Unsubscribe=One-Click" });

    // Each sender, for an account that is on and then off.
    const owner = `account:${id}`;
    mail.sent.length = 0;
    assert.equal(await tellPublished(env, owner, { name: "Two", url: "https://hub.example/a/2", listed: false }), null);
    assert.equal(await tellMade(env, userOf(env, id), { request: "a timer" }), null);
    assert.equal(await tellFirstOpen(env, "c".repeat(64), { owner, name: "Three", line: "line-3" }, "https://hub.example"), null);
    assert.equal(mail.sent.length, 3);
    assert.ok(mail.sent.every((x) => x.headers["List-Unsubscribe"] === `<${link}>` && linkIn(x.text) === link));

    turnOff(env, id);
    mail.sent.length = 0;
    assert.equal(await tellPublished(env, owner, { name: "Four", url: "https://hub.example/a/4", listed: false }), "turned off");
    assert.equal(await tellMade(env, userOf(env, id), { request: "a clock" }), "turned off");
    assert.equal(await tellFirstOpen(env, "d".repeat(64), { owner, name: "Five", line: "line-5" }, "https://hub.example"), "turned off");
    const again = await as(env, id, "POST", "/publish", { bytes: ported("chart.krate"), headers: { "x-krate-name": "Chart" } });
    assert.equal(again.status, 200);
    assert.equal(mail.sent.length, 0, "nothing non-essential reaches a person who turned it off");

    // A hub with no secret cannot make a link, so it sends none of these.
    const bare = hubEnv({ KEY_WRAP_SECRET: "" });
    const id2 = person(bare, "nolink@example.com");
    assert.equal(await tellPublished(bare, `account:${id2}`, { name: "Six", url: "u", listed: false }), "no unsubscribe link");
    assert.equal(mail.sent.length, 0);
  } finally {
    mail.restore();
  }
});

test("essential mail still reaches a person who turned Krate's emails off: the sign-in link and a team invitation", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const id = person(env, "off@example.com");
    turnOff(env, id);
    const signIn = await call(env, null, "POST", "/login/email", { json: { email: "off@example.com" } });
    assert.equal(signIn.status, 200, JSON.stringify(signIn.body));
    const invite = await mailInvite(env, null, { kind: "email", display: "off@example.com", role: "recipient", join_url: "https://krate.tech/join/x", expires_at: 2e9 }, { orgName: "Acme", slug: "acme", inviter: "Alice", orgSlot: async () => true });
    assert.deepEqual(invite, { emailed: true });
    assert.equal(mail.sent.length, 2);
    assert.equal(mail.sent[0].subject, "Sign in to Krate");
    assert.ok(mail.sent[1].subject.includes("invited you to Acme"));
    assert.ok(mail.sent.every((m) => !m.headers && !m.text.includes("Unsubscribe")), "essential mail carries no unsubscribe");
  } finally {
    mail.restore();
  }
});

test("the welcome carries the line and headers, and is never sent to an account that turned emails off first", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv({ WELCOME_AUTO: "1" });
    const on = person(env, "new@example.com", "Nia Long");
    const off = person(env, "quiet@example.com", "Quinn");
    turnOff(env, off);
    const long_ago = String(Date.now() - 60 * 60 * 1000);
    env._kv.set(`welcome-due:${on}`, long_ago);
    env._kv.set(`welcome-due:${off}`, long_ago);
    await worker.scheduled({ cron: "*/10 * * * *" }, env);
    assert.equal(mail.sent.length, 1);
    const [w] = mail.sent;
    assert.deepEqual(w.to, ["new@example.com"]);
    assert.ok(w.text.startsWith("Hi Nia,\n"));
    const link = await unsubscribeLink(env, on);
    assert.equal(linkIn(w.text), link);
    assert.equal(w.headers["List-Unsubscribe-Post"], "List-Unsubscribe=One-Click");
    assert.equal(env._kv.get(`welcome-due:${off}`), undefined, "the turned-off account's welcome is dropped, not retried");
    assert.ok(!userOf(env, off).welcomed);
  } finally {
    mail.restore();
  }
});

/* The admin's GitHub token (KRATE_ADMINS=boss; test/github-stub.mjs). */
const ADMIN = { authorization: "Bearer gho_boss", "content-type": "application/json" };
const announce = (env, payload, headers = ADMIN) => raw(env, "POST", "/admin/mail/announce", { body: JSON.stringify(payload), headers });
const desk = async (env, method, route, payload, headers = ADMIN) => {
  const r = await raw(env, method, `/admin/api/${route}`, { body: payload ? JSON.stringify(payload) : undefined, headers });
  let body = r.text;
  try {
    body = JSON.parse(r.text);
  } catch (_) {}
  return { status: r.status, body };
};

/* Five accounts: three get mail, one turned it off, one has no address. */
function crowd(env) {
  const ids = {
    ada: person(env, "ada@example.com", "Ada Lovelace"),
    bo: person(env, "bo@example.com", ""),
    cy: person(env, "cy@example.com", "Cy"),
    off: person(env, "off@example.com", "Otto"),
  };
  turnOff(env, ids.off);
  env._kv.set("user:noaddr", JSON.stringify({ id: "noaddr", login: "ghost", name: "Ghost", created: 1 }));
  return ids;
}
const thanks = { id: "thank-you-2026-11", subject: "Thank you, {first_name}", body: "Hi {first_name},\n\nThank you for trying Krate.\nIt means a lot.\n\nSee https://krate.tech/app/." };

test("an announcement: no admin token, no answer; a dry run counts and names nobody; a real run sends once to each", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const ids = crowd(env);
    // Without the admin's GitHub sign-in it does not exist.
    assert.equal((await announce(env, thanks, { "content-type": "application/json" })).status, 404);
    assert.equal((await announce(env, thanks, { ...ADMIN, authorization: `Bearer krs_${ids.ada}` })).status, 404, "a signed-in non-admin");
    assert.equal((await announce(env, thanks, { ...ADMIN, authorization: "Bearer gho_ada" })).status, 404, "a GitHub user who is not an admin");
    assert.equal((await desk(env, "GET", "mail/people", null, { authorization: "Bearer gho_ada" })).status, 404);
    assert.equal(mail.sent.length, 0);

    const dry = await announce(env, thanks);
    assert.equal(dry.status, 200);
    const d = JSON.parse(dry.text);
    // orgEnv's seven people have no address either.
    assert.equal(d.dry_run, true);
    assert.equal(d.would_send, 3);
    assert.equal(d.skipped.unsubscribed, 1);
    assert.equal(d.skipped.no_address, 8);
    assert.ok(!dry.text.includes("@example.com"), "a dry run never shows an address");
    assert.equal(mail.sent.length, 0, "a dry run sends nothing");
    assert.equal(env._kv.get(`announce:${thanks.id}`), undefined, "and writes nothing");

    const real = JSON.parse((await announce(env, { ...thanks, dry_run: false })).text);
    assert.equal(real.counts.sent, 3);
    assert.equal(mail.sent.length, 3);
    assert.deepEqual(mail.sent.map((m) => m.to[0]).sort(), ["ada@example.com", "bo@example.com", "cy@example.com"]);
    assert.ok(!mail.sent.some((m) => m.to[0] === "off@example.com"), "the unsubscribed person is skipped");
    for (const m of mail.sent) {
      assert.equal(m.from, "Yashraj Pardeshi <yashraj@krate.tech>");
      assert.equal(m.reply_to, "yashraj@krate.tech");
      const id = Object.entries(ids).find(([k]) => m.to[0].startsWith(k))[1];
      const link = await unsubscribeLink(env, id);
      assert.equal(m.headers["List-Unsubscribe"], `<${link}>`);
      assert.equal(m.headers["List-Unsubscribe-Post"], "List-Unsubscribe=One-Click");
      assert.ok(m.text.includes(`Unsubscribe: ${link}`) && m.html.includes(`href="${link.replace(/&/g, "&amp;")}"`), "the person's own link in both");
    }
    const ada = mail.sent.find((m) => m.to[0] === "ada@example.com");
    assert.equal(ada.subject, "Thank you, Ada");
    assert.ok(ada.text.startsWith("Hi Ada,\n\nThank you for trying Krate.\nIt means a lot."));
    assert.ok(mail.sent.find((m) => m.to[0] === "bo@example.com").text.startsWith("Hi there,"), "no name: there");

    // A retry, a second click: nobody twice.
    const again = JSON.parse((await announce(env, { ...thanks, dry_run: false })).text);
    assert.equal(mail.sent.length, 3, "a second run sends nothing");
    assert.equal(again.counts.sent, 3);
    assert.equal(JSON.parse((await announce(env, thanks)).text).would_send, 0, "the dry run now finds them all sent");
    // The same id with other words is refused.
    assert.equal((await announce(env, { ...thanks, body: "Different words.", dry_run: false })).status, 409);
    assert.equal(mail.sent.length, 3);
  } finally {
    mail.restore();
  }
});

test("the HTML and the text both end with the signature and the unsubscribe link; the logo is the site's", () => {
  const link = "https://hub.example/mail/unsubscribe?u=x&t=abc";
  const m = renderAnnouncement({ subject: "Hello", body: "Para one <b>not bold</b>.\n\nPara two: https://krate.tech/app/." }, { firstName: "", link });
  assert.ok(m.text.endsWith(`--\nbest,\nYashraj\nkrate.tech\nWe value your contribution to Krate.\n\nDon't want these emails? Unsubscribe: ${link}\n`), m.text);
  assert.ok(m.html.includes("--<br>best,<br>Yashraj<br>"));
  assert.ok(m.html.includes('<img src="https://krate.tech/krate-logo.png" width="40" height="40" alt="Krate"'));
  assert.ok(m.html.includes("<em>We value your contribution to Krate.</em>"));
  assert.ok(m.html.indexOf("krate-logo.png") < m.html.indexOf("Unsubscribe"), "the unsubscribe line below the signature");
  assert.ok(m.html.includes('color:#8a8a92;">Don\'t want these emails? <a href="https://hub.example/mail/unsubscribe?u=x&amp;t=abc"'));
  assert.ok(m.html.includes("&lt;b&gt;not bold&lt;/b&gt;"), "the body is text, never markup");
  assert.ok(m.html.includes('<a href="https://krate.tech/app/" style="color:#3d6df0;">https://krate.tech/app/</a>.'), "links become links, the full stop stays out");
  assert.equal((m.html.match(/<p style="margin:0 0 16px 0;">/g) || []).length, 2, "a blank line is a new paragraph");
});

test("the desk: people with who is unsubscribed, a draft that survives, a test to the founder only, send once, history", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const ids = crowd(env);
    const P = await desk(env, "GET", "mail/people");
    assert.equal(P.status, 200);
    assert.deepEqual(P.body.counts, { accounts: 12, gets_mail: 3, turned_off: 1, no_address: 8 });
    assert.equal(P.body.people.find((p) => p.id === ids.off).news, false);

    // The draft is kept in the hub, so a reload finds it.
    const saved = await desk(env, "POST", "mail/draft", { subject: "Hi", body: "Words", audience: "selected", selected: [ids.ada, ids.cy] });
    const draftId = saved.body.draft.id;
    assert.match(draftId, /^mail-\d{4}-\d\d-\d\d-[0-9a-f]{6}$/);
    const back = await desk(env, "GET", "mail/draft");
    assert.deepEqual(back.body.draft.selected, [ids.ada, ids.cy]);
    assert.equal((await desk(env, "POST", "mail/draft", { subject: "Hi again", body: "Words" })).body.draft.id, draftId, "the same draft keeps its id");

    // The test goes to the founder alone and is not the announcement.
    const t = await desk(env, "POST", "mail/test", { subject: "Thank you", body: "Hi {first_name}" });
    assert.equal(t.status, 200);
    assert.equal(mail.sent.length, 1);
    assert.deepEqual(mail.sent[0].to, ["yashraj@krate.tech"]);
    assert.equal(mail.sent[0].subject, "[test] Thank you");
    assert.ok(mail.sent[0].html.includes("krate-logo.png"));
    // Clicking Unsubscribe in a test must not stop the founder's own email:
    // the line points at the setting, with no one-click link or headers.
    assert.ok(mail.sent[0].text.includes("https://krate.tech/account/#mail"), mail.sent[0].text);
    assert.ok(!/mail\/unsubscribe\?u=/.test(mail.sent[0].text + mail.sent[0].html), "no real one-click link in a test");
    assert.ok(!mail.sent[0].headers || !mail.sent[0].headers["List-Unsubscribe"], "no one-click header in a test");
    assert.equal(env._kv.get(`announce:${draftId}`), undefined);

    // Send to two picked people (one picked person later turned off).
    mail.sent.length = 0;
    const payload = { id: draftId, subject: "Thanks", body: "Hi {first_name}", audience: "selected", selected: [ids.ada, ids.cy, ids.off] };
    const s1 = await desk(env, "POST", "mail/send", payload);
    assert.equal(s1.status, 200, JSON.stringify(s1.body));
    assert.equal(mail.sent.length, 2);
    assert.equal(s1.body.results[ids.off].s, "skipped");
    assert.equal(s1.body.results[ids.off].why, "unsubscribed");
    assert.equal(s1.body.results[ids.ada].s, "sent");
    assert.equal((await desk(env, "GET", "mail/draft")).body.draft, null, "sending clears the draft");
    // A double click or a reload sends nothing new.
    const s2 = await desk(env, "POST", "mail/send", payload);
    assert.equal(mail.sent.length, 2);
    assert.equal(s2.body.counts.sent, 2);

    const H = await desk(env, "GET", "mail/history");
    assert.equal(H.body.announcements.length, 1);
    assert.deepEqual(H.body.announcements[0].counts, { people: 3, sent: 2, skipped: 1, failed: 0, waiting: 0 });
    const one = await desk(env, "GET", `mail/announcement?id=${draftId}`);
    assert.equal(one.body.body, "Hi {first_name}");
    assert.equal(Object.keys(one.body.results).length, 3);

    // A refused send is shown as failed, with why, and only that one.
    mail.refuse = (m) => m.to[0] === "bo@example.com";
    const s3 = await desk(env, "POST", "mail/send", { id: "mail-two", subject: "Two", body: "Second" });
    assert.equal(s3.body.results[ids.bo].s, "failed");
    assert.match(s3.body.results[ids.bo].why, /resend 422/);
    assert.equal(s3.body.counts.sent, 2);
  } finally {
    mail.restore();
  }
});

test("past the day's ceiling the rest waits for the ten-minute run, which sends it without sending anyone twice", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv({ MAIL_MOMENTS_PER_DAY: "2" });
    crowd(env);
    const s = await desk(env, "POST", "mail/send", { id: "big-one", subject: "Big", body: "Hello" });
    assert.equal(s.body.counts.sent, 2);
    assert.equal(s.body.counts.waiting, 1);
    assert.equal(s.body.status, "queued");
    assert.equal(s.body.stopped, "today's email limit is reached");
    assert.deepEqual(JSON.parse(env._kv.get("announce-queue")), ["big-one"]);
    // The cron, still today: nothing more goes.
    await runQueuedAnnouncements(env);
    assert.equal(mail.sent.length, 2);
    // Tomorrow (a new day's counter): the rest.
    for (const k of [...env._kv.keys()]) if (k.startsWith("mail-day:")) env._kv.delete(k);
    await worker.scheduled({ cron: "*/10 * * * *" }, env);
    assert.equal(mail.sent.length, 3);
    assert.equal(new Set(mail.sent.map((m) => m.to[0])).size, 3, "three people, each once");
    assert.deepEqual(JSON.parse(env._kv.get("announce-queue")), []);
    assert.equal(JSON.parse(env._kv.get("announce:big-one")).status, "done");
    await worker.scheduled({ cron: "*/10 * * * *" }, env);
    assert.equal(mail.sent.length, 3);
  } finally {
    mail.restore();
  }
});

test("end to end: the link in an announcement unsubscribes, the desk shows it, the next one skips them, and the settings bring it back", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const ids = crowd(env);
    await desk(env, "POST", "mail/send", { id: "first", subject: "One", body: "First" });
    const toCy = mail.sent.find((m) => m.to[0] === "cy@example.com");
    // Cy clicks the link in the mail: the page asks, Cy confirms.
    const link = linkIn(toCy.text);
    assert.ok((await raw(env, "GET", pathOf(link))).text.includes("Stop emails from Krate?"));
    assert.ok((await raw(env, "POST", pathOf(link), form({ do: "stop" }))).text.includes("Done."));
    // Ada's mail app uses the one-click header instead.
    const toAda = mail.sent.find((m) => m.to[0] === "ada@example.com");
    const header = toAda.headers["List-Unsubscribe"].slice(1, -1);
    assert.equal((await raw(env, "POST", pathOf(header), form({ "List-Unsubscribe": "One-Click" }))).status, 200);

    const P = (await desk(env, "GET", "mail/people")).body;
    assert.equal(P.people.find((p) => p.id === ids.cy).news, false, "the desk shows Cy unsubscribed");
    assert.equal(P.people.find((p) => p.id === ids.ada).news, false);
    assert.equal(P.counts.gets_mail, 1);

    mail.sent.length = 0;
    const second = await desk(env, "POST", "mail/send", { id: "second", subject: "Two", body: "Second" });
    assert.deepEqual(mail.sent.map((m) => m.to[0]), ["bo@example.com"], "only the one still subscribed");
    assert.equal(second.body.results[ids.cy].why, "unsubscribed");

    // Cy turns it back on in the account settings.
    assert.deepEqual((await as(env, ids.cy, "PUT", "/me/mail", { json: { news: true } })).body, { news: true });
    mail.sent.length = 0;
    await desk(env, "POST", "mail/send", { id: "third", subject: "Three", body: "Third" });
    assert.deepEqual(mail.sent.map((m) => m.to[0]).sort(), ["bo@example.com", "cy@example.com"]);
  } finally {
    mail.restore();
  }
});

test("the desk page carries the Mail section, and its preview runs the same render as the send", async () => {
  const env = hubEnv();
  const page = await raw(env, "GET", "/admin");
  assert.equal(page.status, 200);
  assert.ok(page.text.includes('<button data-t="mail">Mail</button>'));
  assert.ok(page.text.includes("async function deskMail(v)"));
  assert.ok(page.text.includes(`const renderAnnouncement=${renderAnnouncement.toString()}`));
  // The page's script still parses as a whole.
  const script = page.text.slice(page.text.indexOf("<script>") + 8, page.text.lastIndexOf("</script>"));
  assert.doesNotThrow(() => new Function(script));
});

test("someone who unsubscribes while an announcement waits is skipped when the run reaches them", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv({ MAIL_MOMENTS_PER_DAY: "1" });
    crowd(env);
    const s = await desk(env, "POST", "mail/send", { id: "slow-one", subject: "Slow", body: "Hello" });
    assert.equal(s.body.counts.sent, 1);
    assert.equal(s.body.counts.waiting, 2);
    const waiting = JSON.parse(env._kv.get("announce:slow-one")).targets.filter((id) => !s.body.results[id]);
    turnOff(env, waiting[0]);
    for (const k of [...env._kv.keys()]) if (k.startsWith("mail-day:")) env._kv.delete(k);
    await runQueuedAnnouncements(env);
    for (const k of [...env._kv.keys()]) if (k.startsWith("mail-day:")) env._kv.delete(k);
    await runQueuedAnnouncements(env);
    const record = JSON.parse(env._kv.get("announce:slow-one"));
    assert.deepEqual(record.results[waiting[0]].why, "unsubscribed");
    assert.equal(mail.sent.length, 2, "the one who left is not mailed");
    assert.equal(record.status, "done");
  } finally {
    mail.restore();
  }
});
