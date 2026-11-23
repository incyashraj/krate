// The inbox in Studio: what Krate wrote to you, read in the app (2026-11-23).
//
// Runs the SHIPPED inbox section of studio/ui/app.js in a small fake page,
// with `invoke` going to the REAL hub worker (cloud/worker/src/index.js),
// so the seam between the two is what is tested: a report sent signed out
// keeps its secret, the founder's reply on the desk shows up with a count,
// opening the inbox reads it, the person answers back, and every message is
// set as text, never as HTML.
//
//   node docs/landing/studio/inbox-test.mjs
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import assert from "node:assert/strict";
import vm from "node:vm";

// The hub imports WebAssembly modules, which Node 22 loads only behind a
// flag; this folder's tests run as plain `node <file>`, so ask for it here.
if (!process.execArgv.includes("--experimental-wasm-modules")) {
  const again = spawnSync(process.execPath, ["--experimental-wasm-modules", "--no-warnings", ...process.argv.slice(1)], { stdio: "inherit" });
  process.exit(again.status ?? 1);
}
const { default: worker } = await import("../../../cloud/worker/src/index.js");
const { r2Mock } = await import("../../../cloud/worker/test/r2-mock.mjs");
await import("../../../cloud/worker/test/github-stub.mjs");

const app = readFileSync("studio/ui/app.js", "utf8");
const html = readFileSync("studio/ui/index.html", "utf8");
const bridge = readFileSync("docs/landing/studio/bridge.js", "utf8");
const css = readFileSync("studio/ui/style.css", "utf8");
const section = app.slice(app.indexOf("/* ---- the inbox: what Krate says to you"));
assert.ok(section.length > 2000 && section.includes("function inboxPoll"), "the inbox section was found");

/* ---- the hub, in memory ---------------------------------------------- */
const kv = new Map([
  ["session:krs_ana", "u-ana"],
  ["user:u-ana", JSON.stringify({ id: "u-ana", login: "ana", name: "Ana", email: "ana@example.com", providers: ["email"], created: Date.now() - 86400000 })],
]);
const env = {
  APPS: {
    get: async (k) => kv.get(k) ?? null,
    put: async (k, v) => { kv.set(k, v); },
    delete: async (k) => { kv.delete(k); },
    list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
  },
  BUNDLES: r2Mock(new Map()),
  PUBLIC_BASE: "https://hub.example",
  KRATE_ADMINS: "boss",
};
const hub = async (path, body, token) => {
  const headers = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  const res = await worker.fetch(new Request(`https://hub.example${path}`, { method: "POST", headers, body: JSON.stringify(body) }), env);
  if (!res.ok) throw new Error(await res.text());
  return res.json();
};

/* ---- a page just big enough for the inbox ----------------------------- */
class El {
  constructor(id = "", tag = "div") {
    this.id = id; this.tagName = tag.toUpperCase(); this.children = []; this._text = ""; this.value = "";
    this.hidden = false; this.disabled = false; this.title = ""; this.type = ""; this.on = {}; this.className = "";
    const set = new Set();
    this.classList = {
      add: (c) => set.add(c), remove: (c) => set.delete(c), contains: (c) => set.has(c),
      toggle: (c, on) => { const v = on === undefined ? !set.has(c) : on; if (v) set.add(c); else set.delete(c); return v; },
    };
  }
  set innerHTML(_) { throw new Error(`innerHTML set on #${this.id || this.tagName}: the inbox must build with text only`); }
  get textContent() { return this._text + this.children.map((c) => c.textContent).join(""); }
  set textContent(v) { this._text = String(v); this.children = []; }
  appendChild(c) { this.children.push(c); return c; }
  append(...cs) { for (const c of cs) this.appendChild(c); }
  addEventListener(t, f) { (this.on[t] ||= []).push(f); }
  click() { for (const f of this.on.click || []) f({ preventDefault() {} }); }
  querySelector(sel) { return this[`q:${sel}`] || (this[`q:${sel}`] = new El("", sel)); }
  all() { return [this, ...this.children.flatMap((c) => c.all())]; }
  set onclick(f) { this.on.click = [f]; }
}
const ids = [...html.matchAll(/id="(inbox[A-Za-z]*|sideBell)"/g)].map((m) => m[1]);
for (const need of ["inboxSheet", "inboxList", "inboxThread", "inboxCount", "inboxReply", "inboxReplySend", "inboxBack", "inboxTitle", "inboxThreadMsgs", "inboxThreadNote", "inboxThreadWhen", "inboxUpdate", "sideBell"]) {
  assert.ok(ids.includes(need), `index.html carries #${need}`);
}
const els = Object.fromEntries(ids.map((id) => [id, new El(id)]));
els.inboxSheet.classList.add("hidden");
els.inboxCount.classList.add("hidden");
els.inboxThread.classList.add("hidden");
const toggles = [new El("sideToggle", "button"), new El("sideCloseToggle", "button")];
const storage = new Map();
const toasts = [];
let token = null;
const calls = [];
const ctx = {
  $: (id) => els[id] || null,
  lsGet: (k) => (storage.has(k) ? storage.get(k) : null),
  lsSet: (k, v) => { storage.set(k, String(v)); return true; },
  invoke: async (cmd, args) => {
    calls.push(cmd);
    if (cmd === "inbox_list") return hub("/inbox", { reports: args.reports, read: args.read }, token);
    if (cmd === "inbox_read") return hub("/inbox/read", { at: args.at }, token);
    if (cmd === "inbox_answer") return hub("/inbox/answer", { report: args.report, key: args.key, text: args.text }, token);
    throw new Error(`unknown ${cmd}`);
  },
  document: {
    hidden: false,
    createElement: (tag) => new El("", tag),
    querySelectorAll: (sel) => (sel === ".side-toggle" ? toggles : []),
    addEventListener() {},
  },
  window: { addEventListener() {}, krToast: (t) => toasts.push(t) },
  setTimeout: () => 0,
  setInterval: () => 0,
  Date, Math, JSON, Number, String, Set, Array, Promise, console,
};
vm.createContext(ctx);
vm.runInContext(section + "\n;globalThis.__t = { takeReportKey, inboxPoll, inboxOpen, inboxKeys, inbox, inboxAnswer };", ctx);
const T = ctx.__t;

/* ---- a report sent signed out keeps its secret ------------------------ */
const zip = new Uint8Array([0x50, 0x4b, 3, 4, 9, 9, 9]);
const sent = await (await worker.fetch(new Request("https://hub.example/report", { method: "POST", headers: { "content-type": "application/zip", "x-krate-note": "the timer froze" }, body: zip }), env)).json();
assert.match(sent.key, /^[0-9a-f]{32}$/);
// The desktop engine prints this; the web bridge returns the same shape.
const shown = T.takeReportKey(`sent -- reference ${sent.id}\nreply key ${sent.key}\n`);
assert.equal(shown, `sent -- reference ${sent.id}`, "the secret is never shown to the person");
assert.deepEqual(JSON.parse(JSON.stringify(T.inboxKeys())), [{ id: sent.id, key: sent.key }], "and is kept for the inbox");
assert.equal(T.takeReportKey(`report ${sent.id}`), `report ${sent.id}`, "an older hub without secrets still reads plainly");
assert.match(bridge, /`report \$\{out\.id \|\| "sent"\}` \+ \(out\.key \? `\\nreply key \$\{out\.key\}` : ""\)/, "the web bridge hands the secret back the same way");

/* ---- signed out: a first visit starts reading from now ---------------- */
await hub("/admin/api/messages/send", { to: "everyone", text: "Welcome to Krate Studio." }, "gho_boss");
await T.inboxPoll(true);
assert.equal(T.inbox.data.unread, 0, "something sent to everyone before a first visit gets no dot");
assert.ok(els.inboxCount.classList.contains("hidden"));

/* ---- the founder replies on the desk: a count, a dot, a toast --------- */
await hub("/admin/api/report/reply", { id: sent.id, text: "Thanks! <b>Which</b> browser was it?" }, "gho_boss");
await T.inboxPoll(true);
assert.equal(T.inbox.data.unread, 1, "the reply is unread");
assert.equal(els.inboxCount.textContent, "1", "the bell shows the count");
assert.ok(!els.inboxCount.classList.contains("hidden"));
assert.ok(toggles[0].classList.contains("has-unread"), "the shut drawer's toggle carries a dot");
assert.ok(!toggles[1].classList.contains("has-unread"), "never the close button inside the open drawer");
assert.match(toasts.at(-1), /replied to your report/, "it says so once while Studio is open");

/* ---- opening the inbox reads it, as text ------------------------------ */
T.inboxOpen();
await new Promise((r) => setImmediate(r));
await new Promise((r) => setTimeout(r, 50));
assert.ok(!els.inboxSheet.classList.contains("hidden"), "the sheet opens");
const listText = els.inboxList.textContent;
assert.ok(listText.includes("Thanks! <b>Which</b> browser was it?"), "the message is shown as the words it is, tags and all");
assert.ok(listText.includes("Welcome to Krate Studio."), "the note to everyone is in the list too");
assert.ok(listText.indexOf("browser was it") < listText.indexOf("Welcome to Krate"), "newest first");
assert.equal(T.inbox.data.unread, 0, "opening it reads it");
assert.ok(els.inboxCount.classList.contains("hidden"), "and the count goes");
assert.ok(calls.includes("inbox_read"));

/* ---- the conversation, and answering back ----------------------------- */
const row = els.inboxList.all().find((e) => e.tagName === "BUTTON" && e.className.includes("is-report"));
assert.ok(row, "a reply to a report opens its conversation");
row.click();
assert.equal(els.inboxTitle.textContent, "Your report");
const thread = els.inboxThreadMsgs.textContent;
assert.ok(thread.includes("Sent a report: the timer froze"), "the conversation starts with what they sent");
assert.ok(thread.includes("Which</b> browser"), "then the reply");
els.inboxReply.value = "Firefox, on a phone";
await T.inboxAnswer();
assert.match(els.inboxThreadNote.textContent, /^Sent\./, els.inboxThreadNote.textContent);
const stored = JSON.parse(kv.get(`report:${sent.id}`));
assert.deepEqual(stored.thread.map((m) => [m.who, m.text]), [["krate", "Thanks! <b>Which</b> browser was it?"], ["them", "Firefox, on a phone"]], "the answer lands on the report's thread on the desk");
assert.ok(els.inboxThreadMsgs.textContent.includes("Firefox, on a phone"), "and in the conversation");
assert.equal(els.inboxReply.value, "");

/* ---- signed in: the account's own messages ----------------------------- */
token = "krs_ana";
await hub("/admin/api/messages/send", { to: "u-ana", text: "Hi Ana, your free apps are topped up." }, "gho_boss");
T.inbox.at = 0;
await T.inboxPoll(true);
assert.equal(T.inbox.data.signed_in, true);
assert.ok(T.inbox.data.messages.some((m) => m.kind === "you" && /topped up/.test(m.text)));
assert.equal(T.inbox.data.unread, 1, "signed in, only the note to the account is new: this browser already read the rest");

/* ---- offline: quiet ---------------------------------------------------- */
const before = ctx.invoke;
ctx.invoke = async () => { throw new Error("could not reach krate.tech"); };
const count = els.inboxCount.textContent;
await T.inboxPoll(true);
assert.equal(els.inboxCount.textContent, count, "a failed check changes nothing and says nothing");
ctx.invoke = before;

/* ---- the look: tokens only, and phones --------------------------------- */
const block = css.slice(css.indexOf("/* ---- the inbox (app.js"));
assert.ok(block.length > 500, "the inbox styles are one block");
for (const m of block.matchAll(/#[0-9a-fA-F]{3,8}\b/g)) assert.equal(m[0], "#fff", `only tokens, but found ${m[0]}`);
assert.match(block, /width: min\(480px, calc\(100vw - 32px\)\)/, "the sheet keeps a 16px gutter on a phone");
assert.match(html, /<button class="side-bell" id="sideBell" title="Inbox"/, "the bell is the inbox");
assert.match(app, /window\.krInbox\.open\(\)/, "and opens it");

console.log("ok  studio inbox: a signed-out report's reply shows, counts, reads as text, and is answered");
