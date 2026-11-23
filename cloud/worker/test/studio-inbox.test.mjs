/* Messages in Studio: the founder can reach anyone who uses it (2026-11-23).
 *
 * A person signed in with GitHub sent a report from web Studio after two
 * failed builds. The hub had no email for him, and the desk could only
 * answer by mail, so the founder could not reach him at all. Now the desk
 * replies on the report's thread, sends a note to one person or to
 * everyone, and Studio reads it -- signed in any way, or not at all (a
 * report sent signed out carries a secret that reads its replies). The
 * person can answer on the thread, and the founder is mailed as for the
 * report itself.
 *
 * Driven against the real worker: a Map for KV, the R2 stand-in, a stubbed
 * GitHub and a stubbed mail API, so nothing leaves the machine.
 *
 *   node --experimental-wasm-modules cloud/worker/test/studio-inbox.test.mjs
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";
import "./github-stub.mjs";

const DAY = 86400000;
const kv = new Map([
  // Lee signed in with Google long ago; Nia made her account today.
  ["session:krs_lee", "u-lee"],
  ["user:u-lee", JSON.stringify({ id: "u-lee", login: "lee", name: "Lee Park", email: "lee@example.com", providers: ["google"], created: Date.now() - 40 * DAY })],
  ["session:krs_nia", "u-nia"],
  ["user:u-nia", JSON.stringify({ id: "u-nia", login: "nia", name: "Nia", email: "nia@example.com", providers: ["email"], created: Date.now() + 60000 })],
]);
const puts = [];
const env = {
  APPS: {
    get: async (k) => kv.get(k) ?? null,
    put: async (k, v) => { puts.push(k); kv.set(k, v); },
    delete: async (k) => { kv.delete(k); },
    list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
  },
  BUNDLES: r2Mock(new Map()),
  PUBLIC_BASE: "https://hub.example",
  KRATE_ADMINS: "boss",
  RESEND_API_KEY: "resend-key",
  FOUNDER_NOTIFY: "founder@example.com",
};

const mails = [];
const before = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  if (String(input.url || input) === "https://api.resend.com/emails") {
    mails.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
  }
  return before(input, init);
};

const call = async (path, { token, body, method } = {}) => {
  const headers = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(new Request(`https://hub.example${path}`, {
    method: method || (body === undefined ? "GET" : "POST"),
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  }), env);
  const text = await res.text();
  let data = null;
  try { data = JSON.parse(text); } catch (_) { data = text; }
  return { status: res.status, data };
};
const desk = (route, body) => call(`/admin/api/${route}`, { token: "gho_boss", body });
const inbox = (token, body = {}) => call("/inbox", { token, body });

/// A report as Studio sends it: a zip, signed in or not.
let n = 0;
async function report(token, note) {
  n += 1;
  const zip = new Uint8Array([0x50, 0x4b, 3, 4, n, n, n, n]);
  const headers = { "content-type": "application/zip", "x-krate-session": `s-${n}`, "x-krate-note": note };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(new Request("https://hub.example/report", { method: "POST", headers, body: zip }), env);
  assert.equal(res.status, 200, await res.clone().text());
  return { ...(await res.json()), zip };
}

/* ---- a GitHub person's report is answered in Studio, and they answer ---- */
{
  const sent = await report("gho_sam", "two builds failed");
  assert.match(sent.key, /^[0-9a-f]{32}$/, "a report hands back a reply secret");
  const stored = JSON.parse(kv.get(`report:${sent.id}`));
  assert.equal(stored.owner, "github:sam");
  assert.ok(!JSON.stringify(stored).includes(sent.key), "only the secret's hash is kept");

  const listed = (await desk("reports")).data.reports.find((r) => r.id === sent.id);
  assert.equal(listed.keys, undefined, "the desk never lists a report's secrets");
  assert.equal(listed.has_key, true);

  mails.length = 0;
  const reply = await desk("report/reply", { id: sent.id, text: "Sorry about that. Your second build hit the size limit; try it as two apps." });
  assert.equal(reply.status, 200, JSON.stringify(reply.data));
  assert.equal(reply.data.reaches, "account", "the reply reaches the account that sent the report");
  assert.equal(mails.length, 0, "a desk reply sends no mail to anyone");

  const box = (await inbox("gho_sam")).data;
  assert.equal(box.signed_in, true);
  assert.equal(box.unread, 1, "one unread: the reply");
  assert.equal(box.messages[0].kind, "report");
  assert.equal(box.messages[0].report, sent.id);
  assert.match(box.messages[0].text, /size limit/);
  const thread = box.reports.find((r) => r.id === sent.id);
  assert.ok(thread, "the report comes back with its thread");
  assert.equal(thread.note, "two builds failed");
  assert.deepEqual(thread.thread.map((m) => m.who), ["krate"]);

  const answer = await call("/inbox/answer", { token: "gho_sam", body: { report: sent.id, text: "Thanks! Two apps worked." } });
  assert.equal(answer.status, 200, JSON.stringify(answer.data));
  assert.deepEqual(answer.data.report.thread.map((m) => m.who), ["krate", "you"], "their answer joins the thread");
  const after = JSON.parse(kv.get(`report:${sent.id}`));
  assert.equal(after.state, "new", "an answer puts the report back on the desk's pile");
  assert.deepEqual(after.thread.map((m) => m.who), ["krate", "them"]);
  assert.equal(mails.length, 1, "the founder is mailed when they answer, as for the report");
  assert.deepEqual(mails[0].to, ["founder@example.com"]);
  assert.match(mails[0].subject, /^Answer from sam: Thanks! Two apps worked\./);
  assert.ok(mails[0].text.includes("Their answer: Thanks! Two apps worked."), mails[0].text);
  assert.ok(mails[0].text.includes("YOU: Sorry about that."), "the mail carries the thread");
  assert.ok(mails[0].text.includes("THEM: Thanks! Two apps worked."));
  assert.equal(mails[0].attachments[0].filename, `krate-report-${sent.id}.zip`, "and the report, as before");

  const deskView = (await desk("reports")).data.reports.find((r) => r.id === sent.id);
  assert.deepEqual(deskView.thread.map((m) => m.who), ["krate", "them"], "the desk sees the whole thread");

  const stranger = await call("/inbox/answer", { token: "gho_mallory", body: { report: sent.id, text: "hi" } });
  assert.equal(stranger.status, 404, "nobody else can answer on it");
  assert.equal((await inbox("gho_mallory")).data.reports.length, 0, "or read it");
  console.log("ok  a GitHub person's report is answered in Studio, and their answer mails the founder");
}

/* ---- a report sent signed out is read with its secret ------------------- */
{
  const sent = await report(null, "it froze");
  const reply = await desk("report/reply", { id: sent.id, text: "Which browser was it?" });
  assert.equal(reply.data.reaches, "device", "signed out, the reply waits for the device that holds the secret");

  const box = (await inbox(null, { reports: [{ id: sent.id, key: sent.key }] })).data;
  assert.equal(box.signed_in, false);
  assert.equal(box.reports.length, 1);
  assert.equal(box.messages.find((m) => m.report === sent.id).text, "Which browser was it?");

  const wrong = (await inbox(null, { reports: [{ id: sent.id, key: "0".repeat(32) }] })).data;
  assert.equal(wrong.reports.length, 0, "a wrong secret reads nothing");
  assert.ok(!wrong.messages.some((m) => m.report), "and lists nothing of it");

  assert.equal((await call("/inbox/answer", { body: { report: sent.id, text: "Safari" } })).status, 404, "no secret, no answer");
  assert.equal((await call("/inbox/answer", { body: { report: sent.id, key: "f".repeat(32), text: "Safari" } })).status, 404);
  const ok = await call("/inbox/answer", { body: { report: sent.id, key: sent.key, text: "Safari 18" } });
  assert.equal(ok.status, 200);
  assert.match(mails.at(-1).subject, /^Answer from Someone without an account: Safari 18/);

  // The same zip sent again is the same report: the thread stays, and both
  // secrets read it.
  const res = await worker.fetch(new Request("https://hub.example/report", { method: "POST", headers: { "content-type": "application/zip" }, body: sent.zip }), env);
  const again = await res.json();
  assert.equal(again.id, sent.id);
  for (const key of [sent.key, again.key]) {
    const b = (await inbox(null, { reports: [{ id: sent.id, key }] })).data;
    assert.equal(b.reports[0].thread.length, 2, "the thread survives a second send");
  }
  console.log("ok  a report sent signed out is answered, and answers back, with its secret");
}

/* ---- to everyone: stored once, read by anyone --------------------------- */
{
  puts.length = 0;
  const sent = await desk("messages/send", { to: "everyone", text: "Builds are slow this morning; they all finish." });
  assert.equal(sent.status, 200, JSON.stringify(sent.data));
  const written = puts.filter((k) => !k.startsWith("token:"));
  assert.deepEqual(written.sort(), ["inbox-everyone", "inbox-sent"], "one broadcast is two writes, not one per person");

  const lee = (await inbox("krs_lee")).data;
  assert.ok(lee.messages.some((m) => m.kind === "everyone" && /slow/.test(m.text)));
  assert.equal(lee.unread, 1, "an older account counts it unread");
  const nia = (await inbox("krs_nia")).data;
  assert.ok(nia.messages.some((m) => m.kind === "everyone"), "an account made after it can still read it");
  assert.equal(nia.unread, 0, "but no dot for something sent before the account was made");
  const anon = (await inbox(null, { read: 0 })).data;
  assert.ok(anon.messages.some((m) => m.kind === "everyone"), "signed out sees it too");
  assert.equal(anon.unread, 1);
  assert.equal((await inbox(null, { read: Date.now() + 1000 })).data.unread, 0, "the browser's own read mark counts");

  // A month-old broadcast has gone quiet.
  const list = JSON.parse(kv.get("inbox-everyone"));
  list.push({ id: "old1", at: Date.now() - 31 * DAY, text: "an old one" });
  kv.set("inbox-everyone", JSON.stringify(list));
  assert.ok(!(await inbox("krs_lee")).data.messages.some((m) => m.id === "old1"), "after 30 days a broadcast is no longer shown");

  // Read: one write, and only when it moves.
  // Studio marks read up to the newest message it showed.
  const newest = Math.max(...(await inbox("krs_lee")).data.messages.map((m) => m.at));
  puts.length = 0;
  assert.equal((await call("/inbox/read", { token: "krs_lee", body: { at: newest } })).status, 200);
  assert.equal((await inbox("krs_lee")).data.unread, 0, "read means read");
  assert.deepEqual(puts, ["inbox:account:u-lee"]);
  puts.length = 0;
  await call("/inbox/read", { token: "krs_lee", body: { at: newest } });
  assert.deepEqual(puts, [], "reading again writes nothing");

  // Taken back: gone for everyone.
  assert.equal((await desk("messages/remove", { id: sent.data.id })).status, 200);
  assert.ok(!(await inbox("krs_nia")).data.messages.some((m) => m.id === sent.data.id), "a removed message leaves Studio");
  console.log("ok  a message to everyone is stored once, read signed in or not, and can be taken back");
}

/* ---- to one person, found by what the desk knows ------------------------ */
{
  const found = (await desk("messages/people?q=lee@example.com")).data.people;
  assert.equal(found.length, 1);
  assert.equal(found[0].id, "u-lee");
  assert.equal((await desk("messages/people?q=park")).data.people[0].id, "u-lee", "by name too");

  const long = "x".repeat(2500);
  const sent = await desk("messages/send", { to: "u-lee", text: `<b>Hi Lee</b> ${long}` });
  assert.equal(sent.status, 200);
  const lee = (await inbox("krs_lee")).data;
  const msg = lee.messages.find((m) => m.id === sent.data.id);
  assert.equal(msg.kind, "you");
  assert.equal(msg.text.length, 2000, "a message is cut to 2000 characters");
  assert.ok(msg.text.startsWith("<b>Hi Lee</b>"), "kept as text: every place that shows it escapes it");
  assert.equal(lee.unread, 1);
  assert.ok(!(await inbox("krs_nia")).data.messages.some((m) => m.id === sent.data.id), "nobody else sees it");

  assert.equal((await desk("messages/send", { to: "u-lee", text: "   " })).status, 400, "an empty message is refused");
  assert.equal((await desk("messages/send", { to: "u-nobody", text: "hi" })).status, 404);
  const history = (await desk("messages")).data.sent;
  assert.equal(history[0].label, "Lee Park", "the Sent list names who it went to");
  assert.ok(history.some((s) => s.report), "report replies are in the Sent list too");
  console.log("ok  a message to one person reaches only them, cut to size, as text");
}

/* ---- only admins send, and the desk page carries it --------------------- */
{
  for (const route of ["messages", "messages/people?q=lee"]) {
    assert.equal((await call(`/admin/api/${route}`, { token: "krs_lee" })).status, 404, `${route} is admins only`);
  }
  assert.equal((await call("/admin/api/messages/send", { token: "krs_lee", body: { to: "everyone", text: "hi" } })).status, 404);
  assert.equal((await call("/admin/api/report/reply", { token: "gho_sam", body: { id: "0".repeat(16), text: "hi" } })).status, 404);

  const page = await (await worker.fetch(new Request("https://hub.example/admin"), env)).text();
  assert.match(page, /data-t="messages"/, "the desk has a Messages tab");
  assert.match(page, /reportThreadInto\(d,r\)/, "each report card gets its thread and reply box");
  for (const code of [...page.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1])) new Function(code);
  // Everything a person wrote, and every message, goes in through esc().
  const script = readFileSync(new URL("../src/inbox.js", import.meta.url), "utf8");
  const desking = script.slice(script.indexOf("export function deskInboxScript"));
  assert.ok(!/\+\s*(m|s)\.text\s*\+/.test(desking), "no message text is put into the page unescaped");
  console.log("ok  only admins send, and the desk page carries the tab and the thread");
}

globalThis.fetch = before;
console.log("studio inbox: all checks passed");
