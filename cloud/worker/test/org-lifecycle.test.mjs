/* The organisation pilot's last pieces (IC-948, Addendum 3 A, B, D and E,
 * shapes from Addendum 4): invitation mail, the refused-attempt limit,
 * close with approval and a window, the standard export, recovery of a
 * lost owner, suspension, and Stripe billing. Mail and Stripe are stubs:
 * nothing here reaches Gmail or Stripe.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-lifecycle.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture, worker } from "./org-tools.mjs";
import { directory } from "./zip-tools.mjs";
import { sha256Hex } from "./r2-mock.mjs";
import { RateGate } from "../src/index.js";

const DAY = 24 * 60 * 60;
const store = (env, slug = "acme") => env.ORGS.object(slug);
const at = (env, seconds) => {
  store(env).now = () => seconds;
};
const now = () => Math.floor(Date.now() / 1000);
const auditLines = async (env, who = "alice") =>
  (await call(env, who, "GET", "/orgs/acme/audit")).body.events.map((e) => `${e.actor} ${e.action} ${e.result}${e.reason ? ` ${e.reason}` : ""}`);

async function released(env, name = "candidate-v1.krate") {
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`)).status, 200);
  return up.body.digest;
}

/* Gmail's SMTP server, as test/founder-welcome.test.mjs stands it in. */
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

function mailEnv(extra = {}) {
  const seen = { commands: [], mails: [] };
  const env = orgEnv({ GMAIL_APP_PASSWORD: "app password", SMTP_CONNECT: fakeGmail(seen), ...extra });
  return { env, seen };
}

/* ---- A: invitation mail ---------------------------------------------- */

test("an email invitation is mailed from the founder's Gmail with krate.tech in copy, saying what it is", async () => {
  const { env, seen } = mailEnv();
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "erin@corp.example", role: "developer" } });
  assert.strictEqual(inv.status, 201);
  assert.strictEqual(inv.body.emailed, true);
  assert.strictEqual(inv.body.email_problem, undefined);
  assert.strictEqual(seen.mails.length, 1);
  const [mail] = seen.mails;
  assert.ok(mail.commands.includes("MAIL FROM:<incyashraj@gmail.com>"));
  assert.ok(mail.commands.includes("RCPT TO:<erin@corp.example>"));
  assert.ok(mail.commands.includes("RCPT TO:<yashraj@krate.tech>"), "cc yashraj@krate.tech");
  assert.match(mail.data, /^From: Yashraj Pardeshi <incyashraj@gmail.com>/m);
  const text = mailText(mail);
  // The role in the dashboard's own words, the one link, no code to copy.
  for (const want of ["alice invited you to Acme on Krate. There you can build apps and send them.", `Join Acme: ${inv.body.join_url}`, "sign in with this address and press Join", "/download/", "only for erin@corp.example, until"]) {
    assert.ok(text.includes(want), `${want} in\n${text}`);
  }
  // The same request again is the same answer, and no second mail.
  const key = { json: { email: "frank@corp.example", role: "reviewer" }, key: "inv-1" };
  const first = await call(env, "alice", "POST", "/orgs/acme/invites", key);
  const again = await call(env, "alice", "POST", "/orgs/acme/invites", key);
  assert.deepStrictEqual([first.body.emailed, again.body.emailed, again.body.code], [true, true, first.body.code]);
  assert.strictEqual(seen.mails.length, 2);
});

test("a GitHub invitation is not mailed, and a failed or unconfigured send never fails the invitation", async () => {
  const { env, seen } = mailEnv();
  await acme(env);
  const gh = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "developer" } });
  assert.deepStrictEqual([gh.status, gh.body.emailed, gh.body.email_problem], [201, false, "GitHub invitations are not mailed -- copy the link and send it"]);
  assert.strictEqual(seen.mails.length, 0);
  seen.fail = true;
  const failed = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "erin@corp.example", role: "developer" } });
  assert.strictEqual(failed.status, 201);
  assert.strictEqual(failed.body.emailed, false);
  assert.match(failed.body.email_problem, /could not be sent/);
  assert.strictEqual((await call(env, "erin@corp.example", "POST", `/orgs/acme/invites/${failed.body.code}/accept`)).status, 200, "the invitation stands");
  const bare = orgEnv();
  await acme(bare);
  const none = await call(bare, "alice", "POST", "/orgs/acme/invites", { json: { email: "erin@corp.example", role: "developer" } });
  assert.deepStrictEqual([none.status, none.body.emailed], [201, false]);
  assert.match(none.body.email_problem, /not set up/);
});

test("without Gmail an invitation goes through Resend from krate.tech, replies to the Gmail", async () => {
  const sent = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    if (String(input.url || input) === "https://api.resend.com/emails") {
      sent.push(JSON.parse(init.body));
      return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
    }
    return realFetch(input, init);
  };
  try {
    const env = orgEnv({ RESEND_API_KEY: "re_test" });
    await acme(env);
    const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "erin@corp.example", role: "developer" } });
    assert.deepStrictEqual([inv.status, inv.body.emailed, inv.body.email_problem], [201, true, undefined]);
    assert.strictEqual(sent.length, 1);
    const [mail] = sent;
    assert.strictEqual(mail.from, "Yashraj Pardeshi <yashraj@krate.tech>");
    assert.deepStrictEqual(mail.to, ["erin@corp.example"]);
    assert.strictEqual(mail.reply_to, "incyashraj@gmail.com");
    assert.ok(mail.text.includes(`Join Acme: ${inv.body.join_url}`), mail.text);
    // A refusal from Resend is the same plain answer, and the invitation stands.
    globalThis.fetch = async () => new Response("domain not verified", { status: 403 });
    const refused = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "frank@corp.example", role: "recipient" } });
    assert.deepStrictEqual([refused.status, refused.body.emailed], [201, false]);
    assert.match(refused.body.email_problem, /could not be sent/);
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("at most 20 invitation mails a day per organisation and 100 per hub; past that the invite is still made", async () => {
  const { env, seen } = mailEnv();
  await acme(env);
  for (let i = 0; i < 20; i++) {
    const r = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: `p${i}@corp.example`, role: "recipient" } });
    assert.strictEqual(r.body.emailed, true, `mail ${i + 1}`);
  }
  const over = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "p20@corp.example", role: "recipient" } });
  assert.deepStrictEqual([over.status, over.body.emailed, over.body.email_problem], [201, false, "This organisation has sent its 20 invitation mails for today -- copy the link and send it"]);
  assert.strictEqual(seen.mails.length, 20);
  // The hub's own ceiling.
  const hub = mailEnv();
  await acme(hub.env);
  hub.env._kv.set(`org-invite-mail:${new Date().toISOString().slice(0, 10)}`, "100");
  const capped = await call(hub.env, "alice", "POST", "/orgs/acme/invites", { json: { email: "x@corp.example", role: "recipient" } });
  assert.deepStrictEqual([capped.status, capped.body.emailed], [201, false]);
  assert.match(capped.body.email_problem, /Krate has sent all its invitation mails for today/);
  assert.strictEqual(hub.seen.mails.length, 0);
});

/* ---- B: refused attempts per person ---------------------------------- */

function gateNamespace() {
  const objects = new Map();
  return {
    idFromName: (n) => n,
    get(id) {
      if (!objects.has(id)) objects.set(id, new RateGate({}));
      const g = objects.get(id);
      return { fetch: (url, init) => g.fetch(new Request(url, init)) };
    },
  };
}

test("30 refused attempts a minute per principal, then 429 before any store is asked", async () => {
  const env = orgEnv({ RATE_GATE: gateNamespace() });
  await acme(env);
  const x = await released(env);
  const before = (await auditLines(env)).length;
  for (let i = 0; i < 30; i++) {
    const r = await call(env, "stranger", "GET", i % 2 ? "/orgs/acme" : `/a/${x}`);
    assert.strictEqual(r.status, 403, `refusal ${i + 1}`);
  }
  const asked = env.ORGS.requests;
  const limited = await call(env, "stranger", "GET", "/orgs/acme");
  assert.deepStrictEqual([limited.status, limited.body.error], [429, "rate-limited"]);
  assert.strictEqual((await call(env, "stranger", "GET", `/a/${x}`)).status, 429);
  assert.strictEqual((await call(env, "stranger", "POST", "/orgs", { json: { slug: "fresh" } })).status, 429);
  assert.strictEqual(env.ORGS.requests, asked, "no store was asked");
  assert.strictEqual((await auditLines(env)).length - before, 30, "thirty refusals audited, nothing after");
  // Another principal, and a member's successes, are unaffected.
  assert.strictEqual((await call(env, "erin", "GET", "/orgs/acme")).status, 403);
  for (let i = 0; i < 40; i++) assert.strictEqual((await call(env, "dan", "GET", `/a/${x}`)).status, 200);
});

/* ---- D: close -------------------------------------------------------- */

test("a sole owner closes by retyping the slug; the window keeps releases open and stops anything new", async () => {
  const env = orgEnv();
  await acme(env);
  const x = await released(env);
  const wrong = await call(env, "alice", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme-co" } });
  assert.deepStrictEqual([wrong.status, wrong.body.error], [400, "close-confirm-mismatch"]);
  const notOwner = await call(env, "bob", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme" } });
  assert.deepStrictEqual([notOwner.status, notOwner.body.error], [403, "role-cannot"]);
  const t0 = now();
  const c = await call(env, "alice", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme" } });
  assert.strictEqual(c.status, 201);
  const close = c.body.close;
  assert.deepStrictEqual(Object.keys(close).sort(), ["approved_by", "ends_at", "id", "requested_at", "requested_by", "state", "statement", "statement_digest"]);
  assert.strictEqual(close.state, "scheduled");
  assert.ok(close.ends_at >= t0 + 30 * DAY && close.ends_at <= now() + 30 * DAY, "the default window is 30 days");
  assert.match(close.statement_digest, /^[0-9a-f]{64}$/);
  const org = (await call(env, "bob", "GET", "/orgs/acme")).body.org;
  assert.deepStrictEqual([org.state, org.close.id, org.closed_at, org.billing_state, org.suspended], ["closing", close.id, null, "pilot", false]);
  // During the window.
  assert.strictEqual((await call(env, "dan", "GET", `/a/${x}`)).status, 200, "members keep opening releases");
  const publish = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v2.krate") });
  assert.deepStrictEqual([publish.status, publish.body.error], [409, "org-closing"]);
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "developer" } })).body.error, "org-closing");
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/export")).status, 202, "owners can export");
  // Owners can cancel.
  assert.strictEqual((await call(env, "bob", "DELETE", `/orgs/acme/close/${close.id}`)).body.error, "role-cannot");
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/close/${close.id}`)).status, 204);
  const back = (await call(env, "alice", "GET", "/orgs/acme")).body.org;
  assert.deepStrictEqual([back.state, back.close], ["active", null]);
  assert.strictEqual((await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v2.krate") })).status, 201);
});

async function twoOwners() {
  const env = orgEnv();
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "owner" } });
  await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  return env;
}

test("with two owners a close waits for the second owner to approve the exact statement digest", async () => {
  const env = await twoOwners();
  const c = await call(env, "alice", "POST", "/orgs/acme/close", { json: {} });
  const close = c.body.close;
  assert.deepStrictEqual([c.status, close.state, close.ends_at], [201, "waiting-for-second-owner", null]);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.state, "active", "nothing changes until approved");
  const self = await call(env, "alice", "POST", `/orgs/acme/close/${close.id}/approve`, { json: { statement_digest: close.statement_digest } });
  assert.deepStrictEqual([self.status, self.body.error], [403, "self-approval"]);
  const wrong = await call(env, "erin", "POST", `/orgs/acme/close/${close.id}/approve`, { json: { statement_digest: "0".repeat(64) } });
  assert.deepStrictEqual([wrong.status, wrong.body.error], [409, "close-statement-mismatch"]);
  const ok = await call(env, "erin", "POST", `/orgs/acme/close/${close.id}/approve`, { json: { statement_digest: close.statement_digest } });
  assert.deepStrictEqual([ok.status, ok.body.close.state, ok.body.close.approved_by], [200, "scheduled", "github:erin"]);
  assert.strictEqual((await call(env, "bob", "GET", "/orgs/acme")).body.org.state, "closing");
  const lines = await auditLines(env);
  assert.ok(lines.includes("github:alice org.close allowed waiting for a second owner"));
  assert.ok(lines.some((l) => l.startsWith("github:erin org.close.approve allowed")));
  assert.ok(lines.includes("github:alice org.close.approve denied self-approval"));
});

test("after the window it is closed: releases stop, former owners read and export within their window, then the files go", async () => {
  const env = await twoOwners();
  const x = await released(env);
  const c = (await call(env, "alice", "POST", "/orgs/acme/close", { json: {} })).body.close;
  await call(env, "erin", "POST", `/orgs/acme/close/${c.id}/approve`, { json: { statement_digest: c.statement_digest } });
  const ends = (await call(env, "alice", "GET", "/orgs/acme")).body.org.close.ends_at;
  at(env, ends + 1);
  const read = await call(env, "alice", "GET", "/orgs/acme");
  assert.strictEqual(read.status, 200, "former owners read it");
  assert.deepStrictEqual([read.body.org.state, read.body.org.closed_at], ["closed", ends]);
  for (const [who, path] of [["bob", "/orgs/acme"], ["dan", `/a/${x}`], ["dan", "/orgs/acme/releases"], ["alice", "/orgs/acme/releases"]]) {
    const r = await call(env, who, "GET", path);
    assert.strictEqual(r.body.error, "org-closed", `${who} ${path}`);
  }
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "x", role: "developer" } })).body.error, "org-closed", "read only");
  const ex = await call(env, "erin", "POST", "/orgs/acme/export");
  assert.deepStrictEqual([ex.status, ex.body.export.state], [202, "ready"]);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/exports")).body.length, 1);
  assert.strictEqual((await call(env, "erin", "GET", `/orgs/acme/exports/${ex.body.export.id}`)).status, 200);
  assert.strictEqual((await call(env, "bob", "GET", "/orgs/acme/exports")).body.error, "org-closed", "only former owners");

  // The scheduled sweep: nothing before the export window ends...
  env.ORGS.object("acme").now = () => ends + 29 * DAY;
  assert.deepStrictEqual(await worker.scheduled({ cron: "17 3 * * *" }, env), undefined);
  assert.ok(await env.BUNDLES.head(x), "bytes kept inside the export window");
  // ...then the bytes and packages go, and the store records what went.
  env.ORGS.object("acme").now = () => ends + 31 * DAY;
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/export")).body.error, "org-closed", "the export window has ended");
  await worker.scheduled({ cron: "17 3 * * *" }, env);
  assert.strictEqual(await env.BUNDLES.head(x), null, "release bytes deleted");
  assert.strictEqual(await env.BUNDLES.head(`org-exports/acme/${ex.body.export.id}.zip`), null, "export package deleted");
  const audit = (await call(env, "alice", "GET", "/orgs/acme/audit")).body;
  const lines = audit.events.map((e) => `${e.actor} ${e.action} ${e.reason || ""}`);
  assert.ok(lines.some((l) => l.startsWith("system:krate org.closed")));
  assert.ok(lines.some((l) => l.startsWith("system:krate org.files.deleted") && l.includes(x.slice(0, 16))), lines.join("\n"));
  assert.strictEqual(audit.chain_ok, true);
  assert.strictEqual(env._kv.get("org-closing:acme"), undefined, "swept once");
});

/* ---- D: export -------------------------------------------------------- */

function unzip(bytes) {
  const out = {};
  for (const e of directory(bytes)) {
    const nameLen = bytes[e.local + 26] | (bytes[e.local + 27] << 8);
    const extraLen = bytes[e.local + 28] | (bytes[e.local + 29] << 8);
    const start = e.local + 30 + nameLen + extraLen;
    out[e.name] = bytes.subarray(start, start + e.size);
  }
  return out;
}

test("the standard export: a versioned manifest with digests and exclusions, the releases, and nothing secret", async () => {
  const env = orgEnv();
  await acme(env);
  const x = await released(env);
  await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v2.krate") });
  const open = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "developer" } });
  for (const who of ["bob", "carol", "dan", "stranger"]) {
    const r = await call(env, who, "POST", "/orgs/acme/export");
    assert.ok(["role-cannot", "not-a-member"].includes(r.body.error), who);
  }
  const ex = await call(env, "alice", "POST", "/orgs/acme/export");
  assert.strictEqual(ex.status, 202);
  assert.deepStrictEqual(Object.keys(ex.body.export).sort(), ["created_at", "expires_at", "id", "size", "state"]);
  assert.strictEqual(ex.body.export.state, "ready");
  const got = await call(env, "alice", "GET", `/orgs/acme/exports/${ex.body.export.id}`);
  assert.strictEqual(got.headers.get("content-type"), "application/zip");
  const files = unzip(got.bytes);
  const manifest = JSON.parse(new TextDecoder().decode(files["export-manifest.json"]));
  assert.strictEqual(manifest.format, "krate.org-export.v1");
  assert.deepStrictEqual(manifest.classes.map((c) => c.class), ["organisation", "members and roles", "candidates", "audit", "app data", "keyring", "releases"]);
  for (const cls of manifest.classes) {
    for (const f of cls.files) {
      assert.strictEqual(await sha256Hex(files[f.path]), f.sha256, `${f.path} digest`);
      assert.strictEqual(files[f.path].length, f.size);
    }
  }
  const releaseFile = manifest.classes.find((c) => c.class === "releases").files[0];
  assert.strictEqual(await sha256Hex(files[releaseFile.path]), x, "the release's own bytes");
  assert.strictEqual(manifest.audit_chain_ok, true);
  assert.deepStrictEqual(manifest.excluded.map((e) => e.class).slice(0, 6), ["signing keys", "tokens and sessions", "secrets", "invitation codes", "idempotency records", "billing identifiers"]);
  const everything = Object.entries(files).filter(([n]) => !n.endsWith(".krate")).map(([, b]) => new TextDecoder().decode(b)).join("\n");
  const { KEYS } = await import("./org-tools.mjs");
  for (const secret of ["krs_", "gho_", KEYS.root, "request_hash", open.body.code]) {
    assert.ok(!everything.includes(secret), `no ${secret}`);
  }
  const members = JSON.parse(new TextDecoder().decode(files["members.json"]));
  assert.deepStrictEqual(members.map((m) => [m.principal, m.role]), [["github:alice", "owner"], ["github:bob", "reviewer"], ["github:carol", "developer"], ["github:dan", "recipient"]]);
  // The package expires on its own clock; a new one can be built (R28, R29).
  at(env, ex.body.export.expires_at + 1);
  const late = await call(env, "alice", "GET", `/orgs/acme/exports/${ex.body.export.id}`);
  assert.deepStrictEqual([late.status, late.body.error], [409, "export-not-ready"]);
  const again = await call(env, "alice", "POST", "/orgs/acme/export");
  assert.strictEqual(again.body.export.state, "ready");
  assert.strictEqual((await call(env, "alice", "GET", `/orgs/acme/exports/${again.body.export.id}`)).status, 200);
});

/* ---- D: recovery ------------------------------------------------------ */

test("a Krate admin restores a lost owner only after 72 hours with no objection, and the audit says so", async () => {
  const { env, seen } = mailEnv({ KRATE_ADMINS: "support" });
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { email: "ops@acme.example", role: "admin" } });
  await call(env, "ops@acme.example", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  seen.mails.length = 0;
  const body = { json: { principal: "email:new-owner@acme.example", reason: "the only owner lost access; checked with the company's IT lead by phone" } };
  const notAdmin = await call(env, "alice", "POST", "/orgs/acme/recover", body);
  assert.deepStrictEqual([notAdmin.status, notAdmin.body.error], [403, "role-cannot"]);
  const r = await call(env, "support", "POST", "/orgs/acme/recover", body);
  assert.strictEqual(r.status, 201, JSON.stringify(r.body));
  assert.deepStrictEqual(Object.keys(r.body.recovery).sort(), ["by", "effective_at", "for", "id", "not_mailed", "reason", "requested_at", "state"]);
  assert.deepStrictEqual([r.body.recovery.state, r.body.recovery.by, r.body.recovery.for], ["waiting", "github:support", "email:new-owner@acme.example"]);
  assert.strictEqual(r.body.recovery.effective_at - r.body.recovery.requested_at, 72 * 3600);
  assert.deepStrictEqual(r.body.mailed, ["email:ops@acme.example"]);
  assert.deepStrictEqual(r.body.not_mailed, ["github:alice"], "GitHub principals have no address the hub proved");
  assert.match(mailText(seen.mails[0]), /It takes effect on .* unless an owner or admin objects/);
  // Shown to owners and admins only.
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.recovery.id, r.body.recovery.id);
  assert.strictEqual((await call(env, "bob", "GET", "/orgs/acme")).body.org.recovery, null);
  // Nobody gains anything during the wait.
  at(env, r.body.recovery.effective_at - 60);
  assert.strictEqual((await call(env, "new-owner@acme.example", "GET", "/orgs/acme")).body.error, "not-a-member");
  // An objection cancels it.
  const objection = await call(env, "ops@acme.example", "POST", `/orgs/acme/recover/${r.body.recovery.id}/object`);
  assert.deepStrictEqual([objection.status, objection.body.recovery.state], [200, "cancelled"]);
  assert.strictEqual((await call(env, "alice", "POST", `/orgs/acme/recover/${r.body.recovery.id}/object`)).body.error, "recovery-not-waiting");
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/recover/${r.body.recovery.id}/object`)).body.error, "role-cannot");
  at(env, r.body.recovery.effective_at + 60);
  assert.strictEqual((await call(env, "new-owner@acme.example", "GET", "/orgs/acme")).body.error, "not-a-member", "a cancelled recovery grants nothing");
  // Without an objection it completes.
  store(env).now = () => Math.floor(Date.now() / 1000);
  const second = await call(env, "support", "POST", "/orgs/acme/recover", body);
  at(env, second.body.recovery.effective_at + 1);
  const restored = await call(env, "new-owner@acme.example", "GET", "/orgs/acme");
  assert.deepStrictEqual([restored.status, restored.body.me.role], [200, "owner"]);
  const lines = await auditLines(env);
  assert.ok(lines.some((l) => l.startsWith("krate-support:github:support member.recover.request allowed")));
  assert.ok(lines.includes("email:ops@acme.example member.recover.object allowed objected; recovery cancelled"), lines.join("\n"));
  assert.ok(lines.includes(`krate-support:github:support member.recover allowed ${body.json.reason}`), lines.join("\n"));
});

/* ---- D and E: suspension, billing, and states kept apart ------------- */

test("nothing is suspended until ORG_BILLING_ENFORCED=1; then the pilot is, and only paid services stop (R24)", async () => {
  const env = orgEnv();
  await acme(env);
  const x = await released(env);
  assert.deepStrictEqual(
    [(await call(env, "alice", "GET", "/orgs/acme")).body.org.billing_state, (await call(env, "alice", "GET", "/orgs/acme")).body.org.suspended],
    ["pilot", false],
  );
  assert.strictEqual((await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v2.krate") })).status, 201, "the pilot allows everything");
  env.ORG_BILLING_ENFORCED = "1";
  const org = (await call(env, "alice", "GET", "/orgs/acme")).body.org;
  assert.deepStrictEqual([org.billing_state, org.suspended, org.state], ["suspended", true, "active"], "suspended is not a lifecycle state");
  const publish = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("root-signed.krate") });
  assert.deepStrictEqual([publish.status, publish.body.error], [402, "org-suspended"]);
  assert.strictEqual((await call(env, "dan", "GET", `/a/${x}`)).status, 200, "members still open existing releases");
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/export")).body.export.state, "ready", "export always works");
  // Recipients are free; a seat beyond the free three is not.
  const rec = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "recipient" } });
  assert.strictEqual(rec.status, 201);
  const seat = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "developer" } });
  assert.deepStrictEqual([seat.status, seat.body.error], [402, "org-suspended"]);
  // A manual invoice puts it in good standing; it lifts at once.
  const notAdmin = await call(env, "alice", "PUT", "/orgs/acme/billing/manual", { json: { reference: "INV-1", paid_until: now() + 30 * DAY } });
  assert.strictEqual(notAdmin.body.error, "role-cannot");
  env.KRATE_ADMINS = "support";
  const manual = await call(env, "support", "PUT", "/orgs/acme/billing/manual", { json: { reference: "INV-1", paid_until: now() + 30 * DAY } });
  assert.deepStrictEqual([manual.status, manual.body.billing_state], [200, "invoiced"]);
  assert.strictEqual((await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("root-signed.krate") })).status, 201);
  // When the paid-until date passes it is past due, and suspended again.
  at(env, now() + 31 * DAY);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.billing_state, "suspended");
  env.ORG_BILLING_ENFORCED = "";
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.billing_state, "past-due", "past due, not suspended, until enforced");
});

test("lifecycle and billing are separate states (R17): neither moves the other", async () => {
  const env = orgEnv({ ORG_BILLING_ENFORCED: "1", KRATE_ADMINS: "support" });
  await acme(env);
  const c = (await call(env, "alice", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme" } })).body.close;
  let org = (await call(env, "alice", "GET", "/orgs/acme")).body.org;
  assert.deepStrictEqual([org.state, org.billing_state], ["closing", "suspended"]);
  // Paying does not reopen a closing organisation...
  await call(env, "support", "PUT", "/orgs/acme/billing/manual", { json: { reference: "INV-2", paid_until: now() + 400 * DAY } });
  org = (await call(env, "alice", "GET", "/orgs/acme")).body.org;
  assert.deepStrictEqual([org.state, org.billing_state, org.suspended], ["closing", "invoiced", false]);
  // ...and cancelling the close does not touch billing.
  await call(env, "alice", "DELETE", `/orgs/acme/close/${c.id}`);
  org = (await call(env, "alice", "GET", "/orgs/acme")).body.org;
  assert.deepStrictEqual([org.state, org.billing_state], ["active", "invoiced"]);
  // A suspended organisation is not closed by it, and can still close.
  const env2 = orgEnv({ ORG_BILLING_ENFORCED: "1" });
  await acme(env2);
  assert.strictEqual((await call(env2, "alice", "GET", "/orgs/acme")).body.org.state, "active");
  assert.strictEqual((await call(env2, "alice", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme" } })).status, 201);
});

/* ---- E: Stripe -------------------------------------------------------- */

function fakeStripe() {
  const real = globalThis.fetch;
  const calls = [];
  let n = 0;
  globalThis.fetch = async (input, init = {}) => {
    const url = String((input && input.url) || input);
    if (!url.startsWith("https://api.stripe.com/")) return real(input, init);
    const headers = new Headers(init.headers || {});
    const form = init.body ? Object.fromEntries(new URLSearchParams(String(init.body))) : null;
    calls.push({ url, method: init.method || "GET", idempotency: headers.get("idempotency-key"), form });
    if (url.endsWith("/v1/customers")) return Response.json({ id: `cus_${++n}` });
    if (url.endsWith("/v1/checkout/sessions")) return Response.json({ id: "cs_1", url: "https://checkout.stripe.com/c/pay/cs_1" });
    if (url.includes("/v1/invoices")) return Response.json({ data: [{ id: "in_1", created: 1, status: "paid", number: "A-1", amount_due: 4500, amount_paid: 4500, currency: "usd", hosted_invoice_url: "https://invoice.stripe.com/i/in_1" }] });
    if (url.includes("/v1/subscriptions/")) return Response.json(calls.sub || {});
    return Response.json({ error: { message: "unexpected" } }, { status: 400 });
  };
  calls.restore = () => (globalThis.fetch = real);
  return calls;
}

test("checkout: billing-not-open without the price; with it, the org's own customer, seats as quantity, idempotent", async () => {
  const env = orgEnv();
  await acme(env);
  const closed = await call(env, "alice", "POST", "/orgs/acme/billing/checkout");
  assert.deepStrictEqual([closed.status, closed.body.error], [503, "billing-not-open"]);
  assert.match(closed.body.message, /yashraj@krate.tech/);
  // Stripe set up, but no organisation price: still invoiced by hand.
  env.STRIPE_SECRET_KEY = "sk_test_x";
  const noPrice = await call(env, "alice", "POST", "/orgs/acme/billing/checkout");
  assert.deepStrictEqual([noPrice.status, noPrice.body.error], [503, "billing-not-open"]);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/billing")).body.checkout_open, false);
  Object.assign(env, { STRIPE_SECRET_KEY: "sk_test_x", STRIPE_PRICE_ORG_SEAT: "price_seat" });
  const stripe = fakeStripe();
  try {
    assert.strictEqual((await call(env, "bob", "POST", "/orgs/acme/billing/checkout")).body.error, "role-cannot", "owners only");
    const r = await call(env, "alice", "POST", "/orgs/acme/billing/checkout", { key: "chk-1" });
    assert.deepStrictEqual(r.body, { url: "https://checkout.stripe.com/c/pay/cs_1" });
    const [customer, session] = stripe.filter((c) => c.method === "POST");
    assert.strictEqual(customer.idempotency, "krate-org-customer-acme");
    assert.strictEqual(customer.form["metadata[krate_org]"], "acme");
    assert.strictEqual(session.idempotency, "krate-org-checkout-acme-chk-1", "the request's key, through to Stripe");
    assert.deepStrictEqual(
      [session.form.customer, session.form["line_items[0][price]"], session.form["line_items[0][quantity]"], session.form.client_reference_id],
      ["cus_1", "price_seat", "3", "org:acme"],
      "seats are owners, admins, developers and reviewers; the recipient is free",
    );
    // Again: the same customer, not a second one.
    await call(env, "alice", "POST", "/orgs/acme/billing/checkout", { key: "chk-2" });
    assert.strictEqual(stripe.filter((c) => c.url.endsWith("/v1/customers")).length, 1);
    // The billing read.
    const read = await call(env, "alice", "GET", "/orgs/acme/billing");
    assert.deepStrictEqual(Object.keys(read.body).sort(), ["checkout_open", "invoices", "manual", "period_end", "plan", "seats_paid", "seats_used", "state"]);
    assert.deepStrictEqual([read.body.plan, read.body.state, read.body.seats_used, read.body.checkout_open], ["pilot", "pilot", 3, true]);
    assert.deepStrictEqual(read.body.invoices[0], { id: "in_1", at: 1, status: "paid", number: "A-1", amount: 4500, currency: "usd", url: "https://invoice.stripe.com/i/in_1" });
    assert.strictEqual((await call(env, "carol", "GET", "/orgs/acme/billing")).body.error, "role-cannot");
  } finally {
    stripe.restore();
  }
});

async function signed(env, event) {
  const payload = JSON.stringify(event);
  const t = Math.floor(Date.now() / 1000);
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(env.STRIPE_WEBHOOK_SECRET), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(`${t}.${payload}`));
  const v1 = [...new Uint8Array(mac)].map((b) => b.toString(16).padStart(2, "0")).join("");
  return worker.fetch(new Request("https://hub.example/billing/webhook", { method: "POST", headers: { "stripe-signature": `t=${t},v1=${v1}` }, body: payload }), env);
}

const subscription = (over = {}) => ({
  id: "sub_1",
  customer: "cus_1",
  status: "active",
  current_period_end: now() + 30 * DAY,
  metadata: { krate_org: "acme" },
  items: { data: [{ quantity: 3, price: { id: "price_seat", product: "prod_seat" } }] },
  ...over,
});

test("the webhook moves the entitlement, and fails closed on an unknown price or customer", async () => {
  const env = orgEnv({ STRIPE_SECRET_KEY: "sk_test_x", STRIPE_PRICE_ORG_SEAT: "price_seat", STRIPE_WEBHOOK_SECRET: "whsec_x", ORG_BILLING_ENFORCED: "1", KRATE_ADMINS: "support" });
  await acme(env);
  const stripe = fakeStripe();
  try {
    await call(env, "alice", "POST", "/orgs/acme/billing/checkout", { key: "k" });
  } finally {
    stripe.restore();
  }
  const bad = await worker.fetch(new Request("https://hub.example/billing/webhook", { method: "POST", headers: { "stripe-signature": "t=1,v1=00" }, body: "{}" }), env);
  assert.strictEqual(bad.status, 400, "unsigned events are refused");
  // An unknown price: the org stays where it was, and the desk is told.
  const unknown = await signed(env, { id: "evt_1", type: "customer.subscription.updated", data: { object: subscription({ items: { data: [{ quantity: 9, price: { id: "price_other" } }] } }) } });
  assert.strictEqual(unknown.status, 200);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.billing_state, "suspended");
  const alerts = await (await worker.fetch(new Request("https://hub.example/admin/org-billing-alerts", { headers: { authorization: "Bearer gho_support" } }), env)).json();
  assert.strictEqual(alerts.length, 1);
  assert.match(alerts[0].why, /unknown price price_other/);
  // Somebody else's customer: refused the same way.
  await signed(env, { id: "evt_2", type: "customer.subscription.updated", data: { object: subscription({ customer: "cus_999" }) } });
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.billing_state, "suspended");
  // The real thing: paid, seats recorded.
  await signed(env, { id: "evt_3", type: "customer.subscription.updated", data: { object: subscription() } });
  const read = (await call(env, "alice", "GET", "/orgs/acme/billing")).body;
  assert.deepStrictEqual([read.plan, read.state, read.seats_paid], ["stripe", "paid", 3]);
  assert.strictEqual((await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") })).status, 201, "suspension lifted");
  // Deleted: no longer paid, suspended again under enforcement.
  await signed(env, { id: "evt_4", type: "customer.subscription.deleted", data: { object: subscription({ status: "canceled" }) } });
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.billing_state, "suspended");
  const lines = await auditLines(env);
  assert.ok(lines.some((l) => l.startsWith("stripe billing.update denied unknown price")));
  assert.ok(lines.some((l) => l.startsWith("stripe billing.update allowed active 3 seats")));
});
