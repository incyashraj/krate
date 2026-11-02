/* A new person gets the founder's plain welcome, once, 21 minutes on.
 *
 * Signing up marks the account welcome-due; the ten-minute run sends the
 * note to accounts at least 21 minutes old -- from the founder's krate.tech
 * address, replies to his Gmail, plain text, the first name filled in --
 * and records that it went, so a second run sends nothing. An account only
 * minutes old waits. An account that existed before (no due mark) is never
 * mailed.
 *
 * Drives the REAL worker with a Map for KV and a stubbed fetch.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/founder-welcome.test.mjs
 */
import assert from "node:assert/strict";
import test from "node:test";
import worker from "../src/index.js";

const KV = new Map();
const env = {
  PUBLIC_BASE: "https://hub.test",
  // The automatic welcome is off unless switched on; these first tests
  // switch it on. The founder's own way is the notification and the desk.
  WELCOME_AUTO: "1",
  KRATE_ADMINS: "boss",
  GITHUB_CLIENT_SECRET: "gh-secret",
  RESEND_API_KEY: "resend-key",
  APPS: {
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => { KV.set(k, v); },
    delete: async (k) => { KV.delete(k); },
    list: async ({ prefix }) => ({ keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })) }),
  },
};

const mailed = [];
const notified = [];
let githubUser = { id: 41, login: "ada", name: "Ada Lovelace", email: "ada@example.com" };
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String(input.url || input);
  const reply = (v) => new Response(JSON.stringify(v), { headers: { "content-type": "application/json" } });
  if (url === "https://github.com/login/oauth/access_token") return reply({ access_token: "gho_ada" });
  if (url === "https://api.github.com/user") return reply(githubUser);
  if (url === "https://api.github.com/user/emails") return reply([{ email: githubUser.email, primary: true, verified: true }]);
  if (url === "https://api.resend.com/emails") {
    const mail = JSON.parse(init.body);
    // The founder's sign-up notifications are kept apart from welcomes.
    (mail.subject && mail.subject.startsWith("New on Krate:") ? notified : mailed).push(mail);
    return reply({ id: "sent" });
  }
  return realFetch(input, init);
};

async function signUpWithGithub() {
  const ctx = { waitUntil() {} };
  const start = await worker.fetch(new Request("https://hub.test/login/start"), env, ctx);
  assert.equal(start.status, 302);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  const done = await worker.fetch(new Request(`https://hub.test/login/callback?state=${state}&code=abc`), env, ctx);
  assert.equal(done.status, 302, "the sign-in finishes");
}

const tick = (now) => {
  const real = Date.now;
  Date.now = () => now;
  return worker.scheduled({ cron: "*/10 * * * *" }, env).finally(() => {
    Date.now = real;
  });
};

test("a new account is welcomed once, 21 minutes on, in the founder's words", async () => {
  await signUpWithGithub();
  const due = [...KV.keys()].filter((k) => k.startsWith("welcome-due:"));
  assert.equal(due.length, 1, "signing up marks the account welcome-due");
  const created = Number(KV.get(due[0]));

  await tick(created + 3 * 60 * 1000);
  assert.equal(mailed.length, 0, "three minutes in, nothing is sent");
  await tick(created + 20 * 60 * 1000);
  assert.equal(mailed.length, 0, "twenty minutes in, still nothing");

  await tick(created + 22 * 60 * 1000);
  assert.equal(mailed.length, 1, "twenty-two minutes in, the note goes");
  const mail = mailed[0];
  assert.equal(mail.from, "Yashraj Pardeshi <yashraj@krate.tech>");
  assert.equal(mail.reply_to, "incyashraj@gmail.com");
  assert.deepEqual(mail.to, ["ada@example.com"]);
  assert.equal(mail.subject, "You’re in. One question from me");
  assert.equal(mail.html, undefined, "plain text only");
  assert.ok(mail.text.startsWith("Hi Ada,\n"), mail.text.slice(0, 40));
  assert.ok(mail.text.includes("What’s the first thing you want to make with Krate?"));
  assert.ok(mail.text.includes("https://calendly.com/incyashraj/30min\n"));
  assert.ok(!mail.text.includes("{"), "no template field left unfilled");

  await tick(created + 25 * 60 * 1000);
  assert.equal(mailed.length, 1, "never twice");
  assert.equal([...KV.keys()].filter((k) => k.startsWith("welcome-due:")).length, 0);
});

test("a nameless sign-up is greeted as 'there', and an old account is never mailed", async () => {
  githubUser = { id: 42, login: "nobody", name: "", email: "x@example.com" };
  await signUpWithGithub();
  // An account from before the welcome existed: a user with no due mark.
  KV.set("user:old-person", JSON.stringify({ id: "old-person", created: 1, email: "old@example.com" }));
  await tick(Date.now() + 22 * 60 * 1000);
  const latest = mailed[mailed.length - 1];
  assert.deepEqual(latest.to, ["x@example.com"]);
  assert.ok(latest.text.startsWith("Hi there,\n"), latest.text.slice(0, 30));
  assert.ok(!mailed.some((m) => m.to.includes("old@example.com")), "no mail to a person from before");
});

/// A stand-in for Gmail's SMTP server: answers each command as Gmail does
/// and keeps what was said, as cloudflare:sockets' connect would hand back.
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
            seen.data = data;
            answer("250 2.0.0 OK queued");
          }
          return;
        }
        const line = text.replace(/\r\n$/, "");
        seen.commands.push(line);
        if (line.startsWith("EHLO")) answer("250-smtp.gmail.com at your service\r\n250 AUTH LOGIN PLAIN");
        else if (line.startsWith("AUTH PLAIN")) answer(seen.wrongPassword ? "535 5.7.8 Username and Password not accepted" : "235 2.7.0 Accepted");
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

test("with the founder's app password, the welcome goes through Gmail itself, cc krate.tech", async () => {
  const seen = { commands: [], data: "" };
  const gmailEnv = { ...env, GMAIL_APP_PASSWORD: "abcd efgh ijkl mnop", SMTP_CONNECT: fakeGmail(seen) };
  githubUser = { id: 43, login: "grace", name: "Grace Hopper", email: "grace@example.com" };
  const ctx = { waitUntil() {} };
  const start = await worker.fetch(new Request("https://hub.test/login/start"), gmailEnv, ctx);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  await worker.fetch(new Request(`https://hub.test/login/callback?state=${state}&code=abc`), gmailEnv, ctx);
  const before = mailed.length;
  const real = Date.now;
  const later = Date.now() + 22 * 60 * 1000;
  Date.now = () => later;
  await worker.scheduled({ cron: "*/10 * * * *" }, gmailEnv).finally(() => {
    Date.now = real;
  });
  assert.equal(mailed.length, before, "not through Resend");
  const auth = seen.commands.find((c) => c.startsWith("AUTH PLAIN "));
  assert.equal(
    Buffer.from(auth.slice("AUTH PLAIN ".length), "base64").toString(),
    "\0incyashraj@gmail.com\0abcd efgh ijkl mnop",
    "signed in as the founder with his app password",
  );
  assert.ok(seen.commands.includes("MAIL FROM:<incyashraj@gmail.com>"));
  assert.ok(seen.commands.includes("RCPT TO:<grace@example.com>"));
  assert.ok(seen.commands.includes("RCPT TO:<yashraj@krate.tech>"), "the krate.tech copy");
  assert.ok(seen.data.includes("From: Yashraj Pardeshi <incyashraj@gmail.com>\r\n"));
  assert.ok(seen.data.includes("Cc: <yashraj@krate.tech>\r\n"));
  assert.ok(seen.data.includes("Content-Type: text/plain; charset=UTF-8"));
  const subject = seen.data.match(/Subject: =\?UTF-8\?B\?([^?]+)\?=/)[1];
  assert.equal(Buffer.from(subject, "base64").toString(), "You’re in. One question from me");
  const body = seen.data.split("\r\n\r\n")[1].replace(/\r\n\.?$/, "").replace(/\r\n/g, "");
  assert.ok(Buffer.from(body, "base64").toString().startsWith("Hi Grace,\n"));
});

test("a wrong app password leaves the welcome due, to try again", async () => {
  const seen = { commands: [], data: "", wrongPassword: true };
  const gmailEnv = { ...env, GMAIL_APP_PASSWORD: "wrong", SMTP_CONNECT: fakeGmail(seen) };
  KV.set("user:due-person", JSON.stringify({ id: "due-person", created: 1000, email: "due@example.com" }));
  KV.set("welcome-due:due-person", "1000");
  await worker.scheduled({ cron: "*/10 * * * *" }, gmailEnv);
  assert.equal(KV.get("welcome-due:due-person"), "1000", "still due");
  assert.equal(JSON.parse(KV.get("user:due-person")).welcomed, undefined, "not marked welcomed");
});


test("a sign-up tells the founder at once, with the welcome ready in Gmail; nothing goes by itself", async () => {
  const offEnv = { ...env, WELCOME_AUTO: undefined };
  githubUser = { id: 44, login: "katherine", name: "Katherine Johnson", email: "kj@example.com" };
  const before = notified.length;
  const ctx = { waitUntil() {} };
  const start = await worker.fetch(new Request("https://hub.test/login/start"), offEnv, ctx);
  const state = new URL(start.headers.get("location")).searchParams.get("state");
  await worker.fetch(new Request(`https://hub.test/login/callback?state=${state}&code=abc`), offEnv, ctx);
  assert.equal(notified.length, before + 1, "the founder hears straight away");
  const note = notified[notified.length - 1];
  assert.deepEqual(note.to, ["incyashraj@gmail.com", "yashraj@krate.tech"]);
  assert.equal(note.subject, "New on Krate: Katherine Johnson");
  assert.ok(note.text.includes("Email:  kj@example.com"));
  assert.ok(note.text.includes("Signed in with: github"));
  const compose = new URL(note.text.match(/https:\/\/mail\.google\.com\/mail\/\S+/)[0]);
  assert.equal(compose.searchParams.get("to"), "kj@example.com");
  assert.equal(compose.searchParams.get("cc"), "yashraj@krate.tech");
  assert.equal(compose.searchParams.get("authuser"), "incyashraj@gmail.com");
  assert.equal(compose.searchParams.get("su"), "You\u2019re in. One question from me");
  assert.ok(compose.searchParams.get("body").startsWith("Hi Katherine,\n"));
  // Nothing marked due, and the ten-minute run sends nothing.
  const id = [...KV.entries()].find(([k, v]) => k.startsWith("user:") && v.includes("kj@example.com"))[0].slice(5);
  assert.equal(KV.get(`welcome-due:${id}`), undefined, "no automatic welcome queued");
  const welcomesBefore = mailed.length;
  const real = Date.now;
  const later = Date.now() + 30 * 60 * 1000;
  Date.now = () => later;
  await worker.scheduled({ cron: "*/10 * * * *" }, offEnv).finally(() => {
    Date.now = real;
  });
  assert.equal(mailed.length, welcomesBefore, "the founder writes them; nothing goes by itself");
});

test("the desk lists who is waiting for a welcome, and marks them welcomed", async () => {
  // KRATE_ADMINS is a GitHub login: the desk is opened with a GitHub token (K-1085).
  githubUser = { id: 7, login: "boss", name: "Boss", email: "boss@example.com" };
  KV.set("session:krs_admin", "a1");
  KV.set("user:a1", JSON.stringify({ id: "a1", login: "boss", name: "Boss" }));
  const as = (path, init = {}) =>
    worker.fetch(
      new Request(`https://hub.test${path}`, {
        ...init,
        headers: { authorization: "Bearer gho_boss", "content-type": "application/json" },
      }),
      env,
      { waitUntil() {} },
    );
  const list = await (await as("/admin/api/welcomes")).json();
  const kj = list.people.find((p) => p.email === "kj@example.com");
  assert.ok(kj && !kj.welcomed, "a new person is waiting");
  assert.ok(kj.compose.startsWith("https://mail.google.com/mail/?"), "with the one-click compose link");
  const waiting = list.waiting;
  await as("/admin/api/welcomed", { method: "POST", body: JSON.stringify({ id: kj.id, welcomed: true }) });
  const after = await (await as("/admin/api/welcomes")).json();
  assert.ok(after.people.find((p) => p.id === kj.id).welcomed > 0);
  assert.equal(after.waiting, waiting - 1);
  const stranger = await worker.fetch(new Request("https://hub.test/admin/api/welcomes"), env, { waitUntil() {} });
  assert.equal(stranger.status, 404, "nobody but an admin sees the list");
});
