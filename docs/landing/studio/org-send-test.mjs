// Describe it, approve it, ship it, all in a browser (G5), and start an app
// from a spreadsheet (G6): the shipped bridge.js, studio/ui/app.js and the
// console's New app page, run against stand-ins for the hub, the build
// service and the browser's signing (kit/org-device.js is proved against
// the real hub in cloud/worker/test/org-web-send.test.mjs).
//
// - Only an owner or a developer is offered "Send to <org> for approval";
//   the list follows the hub's role matrix.
// - Sending fetches the build's bytes, hands them to the browser's signing
//   with the app's name, the release note and the person's role, and says
//   the answer in words with a link to the console.
// - /app/?org=<slug> orders things and never sends; ?start=sheet opens the
//   spreadsheet start once.
// - A spreadsheet's build request carries its columns and a sample of rows,
//   and the whole sheet only as the app's asset.
// - An owner's browser that holds the key approves waiting publishers when
//   the console opens.
//
//   node docs/landing/studio/org-send-test.mjs
import { readFileSync } from "node:fs";
import { DASH, page, settle, orgRoutes, member, ACME } from "./org-test-kit.mjs";
import "../kit/sheet.js";

const bridgeSrc = readFileSync("docs/landing/studio/bridge.js", "utf8");
const appSrc = readFileSync("studio/ui/app.js", "utf8");
const html = readFileSync("studio/ui/index.html", "utf8");

let bad = 0;
function check(ok, name) {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
}
const tick = async () => { for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 0)); };

/* ---- pieces of bridge.js, as shipped ------------------------------------ */
function between(src, from, to) {
  const a = src.indexOf(from);
  const b = src.indexOf(to, a + 1);
  if (a < 0 || b < 0) throw new Error(`cannot find ${from} .. ${to}`);
  return src.slice(a, b);
}
function topFn(src, name) {
  const re = new RegExp(`\\n(async )?function ${name}\\(`);
  const m = re.exec(src);
  if (!m) throw new Error(`no function ${name}`);
  const end = src.indexOf("\n}\n", m.index + 1);
  return src.slice(m.index, end + 3);
}
function method(name) {
  const at = bridgeSrc.indexOf(`  async ${name}(`);
  if (at < 0) throw new Error(`no command ${name}`);
  const end = bridgeSrc.indexOf("\n  },\n", at);
  return bridgeSrc.slice(at, end) + "\n  }";
}
const REGION = between(bridgeSrc, "/* ---- an organisation's apps, made here", "\nconst COMMANDS = {");

/// A fake <input type=file> whose picker "chooses" `file` at once.
function fakeDocument(pickFile) {
  return {
    head: { appendChild() {} },
    body: { appendChild() {} },
    createElement(tag) {
      const listeners = {};
      return {
        tag, style: {}, files: null, accept: "",
        addEventListener(k, fn) { listeners[k] = fn; },
        remove() {},
        click() { this.files = pickFile ? [pickFile()] : []; listeners.change && listeners.change(); },
      };
    },
  };
}

/// The bridge's org and spreadsheet commands in a world of stand-ins.
function bridgeWorld({ url = "https://krate.tech/app/", orgs = [], device = null, appBytes = new Uint8Array([80, 75, 3, 4, 9]), file = null, token = "tok-carol" } = {}) {
  const calls = { hub: [], fetch: [], build: [], configure: [], publish: [], kit: [] };
  const session = new Map();
  const u = new URL(url);
  const win = { KrateSheet: globalThis.KrateSheet, addEventListener() {} };
  const bridge = { token, jobResult: { name: "Stock", download: "/build/abc/file" } };
  const attached = new Map();
  const HUB = "https://hub.krate.tech";
  const BUILDER = "https://build.krate.tech";
  async function hub(path) {
    calls.hub.push(path);
    if (path === "/orgs") return orgs;
    throw Object.assign(new Error("no route"), { status: 404 });
  }
  async function fetch(to, init = {}) {
    calls.fetch.push({ url: String(to), headers: init.headers || {} });
    return { ok: true, status: 200, arrayBuffer: async () => appBytes.buffer.slice(0), text: async () => "" };
  }
  const refusal = (m) => Object.assign(new Error(m), { refusal: true });
  const refuse = (m) => Promise.reject(refusal(m));
  let signIns = 0;
  const goSignIn = () => { signIns++; return new Promise(() => {}); };
  const fileFetchError = async () => refusal("gone");
  const currentWebApp = (path) => ({ url: String(path || `${BUILDER}${bridge.jobResult.download}`), name: "Stock.krate" });
  const webMode = () => "build";
  const localSessions = () => [];
  const deviceId = () => "dev-1";
  const watchJob = async (id) => `watching ${id}`;
  const sessionOver = () => null;
  const dropUnbuilt = () => {};
  async function builder(path, opts = {}) {
    calls.build.push({ path, body: JSON.parse(opts.body || "{}") });
    return { id: "job-1" };
  }
  const location = { search: u.search, href: u.href };
  const sessionStorage = { getItem: (k) => (session.has(k) ? session.get(k) : null), setItem: (k, v) => session.set(k, String(v)), removeItem: (k) => session.delete(k) };
  const document = fakeDocument(file);
  const loadKit = { "org-device.js": "KrateOrgDevice", "sheet.js": "KrateSheet" };
  if (device) {
    win.KrateOrgDevice = {
      configure(c) { calls.configure.push(c); },
      publish: async (slug, bytes, opts) => { calls.publish.push({ slug, bytes, opts }); return device(slug, bytes, opts); },
    };
  }
  const extra = [topFn(bridgeSrc, "tooLong"), topFn(bridgeSrc, "attachmentsFor"), topFn(bridgeSrc, "nowSecs")].join("\n");
  const names = ["org_targets", "send_to_org", "sheet_pick", "sheet_use", "sheet_request", "create_app"];
  const make = new Function(
    "window", "location", "sessionStorage", "document", "hub", "fetch", "refusal", "refuse", "goSignIn", "fileFetchError", "currentWebApp",
    "bridge", "attached", "HUB", "BUILDER", "builder", "webMode", "localSessions", "deviceId", "watchJob", "sessionOver", "dropUnbuilt", "btoa", "setTimeout", "MAX_REQUEST_CHARS",
    `${REGION}\n${extra}\nconst COMMANDS = {\n${names.map(method).join(",\n")},\n  async session_save() {}\n};\n` +
      // The real loader appends a <script>; here the script "loads" at once.
      "return { COMMANDS, ORG_SENDERS, loadSiteKit, orgSendAnswer };",
  );
  const api = make(
    win, location, sessionStorage, document, hub, fetch, refusal, refuse, goSignIn, fileFetchError, currentWebApp,
    bridge, attached, HUB, BUILDER, builder, webMode, localSessions, deviceId, watchJob, sessionOver, dropUnbuilt, btoa, setTimeout, 2000,
  );
  for (const [f, g] of Object.entries(loadKit)) if (win[g]) calls.kit.push(f);
  return { ...api, win, calls, bridge, attached, session, signIns: () => signIns };
}
const ORGS = [
  { slug: "acme", name: "Acme", role: "developer" },
  { slug: "beta", name: "Beta Ltd", role: "owner" },
  { slug: "gamma", name: "Gamma", role: "admin" },
  { slug: "delta", name: "Delta", role: "reviewer" },
  { slug: "eps", name: "Eps", role: "recipient" },
];

/* ---- who is offered Send to <org> --------------------------------------- */
{
  const w = bridgeWorld({ orgs: ORGS });
  const list = await w.COMMANDS.org_targets();
  check(JSON.stringify(list.map((o) => o.slug)) === '["acme","beta"]', "only the organisations where this person is an owner or a developer: " + list.map((o) => o.slug).join(","));
  check(list[0].name === "Acme" && list[0].role === "developer", "each with its name and the person's role");
  // The list is the hub's role matrix, in all three places.
  const kit = readFileSync("docs/landing/kit/org.js", "utf8");
  const hubMatrix = readFileSync("cloud/worker/src/org.js", "utf8");
  const row = (src) => { const m = /"candidate\.publish":\s*\{([^}]*)\}/.exec(src); return m ? [...m[1].matchAll(/(\w+):\s*"yes"/g)].map((x) => x[1]).sort().join(",") : ""; };
  check(row(kit) === [...w.ORG_SENDERS].sort().join(",") && row(hubMatrix) === row(kit), `the bridge sends for ${w.ORG_SENDERS.join(" and ")}: the hub's and the site's candidate.publish row`);
  const out = bridgeWorld({ orgs: ORGS, token: null });
  check((await out.COMMANDS.org_targets()).length === 0 && out.calls.hub.length === 0, "signed out, none, and the hub is not asked");
  const broken = bridgeWorld({ orgs: null });
  check(Array.isArray(await broken.COMMANDS.org_targets()), "a hub that answers oddly is an empty list, not an error");
}

/* ---- sending ------------------------------------------------------------- */
{
  const w = bridgeWorld({ orgs: ORGS, device: () => ({ state: "waiting-for-approval", digest: "d".repeat(64), approvers: ["alice", "bob"] }) });
  const sent = await w.COMMANDS.send_to_org({ path: "https://build.krate.tech/build/abc/file", slug: "acme", name: "Stock v2", note: "  Imports the sheet  " });
  check(w.calls.fetch[0].url === "https://build.krate.tech/build/abc/file" && w.calls.fetch[0].headers.authorization === "Bearer tok-carol", "the app's bytes come from the build service, with the sign-in");
  const p = w.calls.publish[0];
  check(p && p.slug === "acme" && p.bytes instanceof Uint8Array && p.bytes.length === 5, "the bytes go to this browser's signing for that organisation");
  check(p && p.opts.name === "Stock" && p.opts.note === "Imports the sheet" && p.opts.role === "developer", "with the app's name, the release note and the person's role");
  check(w.calls.configure.length === 1 && w.calls.configure[0].hub === "https://hub.krate.tech" && w.calls.configure[0].token() === "tok-carol", "the signing talks to the hub with this tab's sign-in");
  check(sent.state === "waiting-for-approval" && sent.words === "It waits for a second person to approve it", "the answer: it waits for a second person");
  check(sent.waiting_url === "https://krate.tech/org/?o=acme&tab=myapps" && sent.digest === "d".repeat(64), "with the developer's My apps in the console");
  await w.COMMANDS.send_to_org({ path: "x", slug: "acme" });
  check(w.calls.configure.length === 1, "the signing is set up once per tab");
  const owner = bridgeWorld({ orgs: ORGS, device: () => ({ state: "waiting-for-approval", digest: "e".repeat(64) }) });
  check((await owner.COMMANDS.send_to_org({ path: "x", slug: "beta" })).waiting_url === "https://krate.tech/org/?o=beta&tab=waiting", "an owner's link is Approvals");
  const solo = bridgeWorld({ orgs: ORGS, device: () => ({ state: "released", digest: "f".repeat(64), url: "https://hub.krate.tech/a/fff", solo: true }) });
  const rel = await solo.COMMANDS.send_to_org({ path: "x", slug: "beta" });
  check(rel.state === "released" && rel.words === "Released. Everyone in Beta Ltd can use it now" && rel.url === "https://hub.krate.tech/a/fff", "alone in the organisation, an owner's app is released, and said so");

  // Not yet a publisher: who can approve, and how.
  const wait = (publisher) => bridgeWorld({ orgs: ORGS, device: () => ({ state: "waiting-for-publisher", publisher }) }).COMMANDS.send_to_org({ path: "x", slug: "acme" });
  const a = await wait({ state: "waiting", can_sign: ["alice"], by_hand: false });
  check(a.state === "waiting-for-publisher" && a.words === "Waiting for alice to approve you as a publisher. It happens by itself when one of them opens Krate or the Acme console." && a.waiting_url === "https://krate.tech/org/?o=acme&tab=home", "waiting to be a publisher: who, and that it happens by itself");
  const b = await wait({ state: "waiting", can_sign: ["alice"], by_hand: true });
  check(/Acme approves new publishers by hand: an owner or admin says yes once, in People\.$/.test(b.words), "approved by hand: the one click it needs, and where");
  const c = await wait({ state: "waiting", can_sign: [], by_hand: false });
  check(/Waiting for an owner of Acme to open Krate or the Acme console/.test(c.words), "nobody holds the key yet: an owner opening Krate or the console");

  // Refused before anything is fetched or signed.
  for (const [slug, why] of [["delta", "a reviewer"], ["gamma", "an admin"], ["eps", "a member"], ["nowhere", "not a member"]]) {
    const r = bridgeWorld({ orgs: ORGS, device: () => ({}) });
    let err = null;
    try { await r.COMMANDS.send_to_org({ path: "x", slug }); } catch (e) { err = e; }
    check(err && err.refusal && /You cannot send apps to/.test(err.message) && r.calls.fetch.length === 0 && r.calls.publish.length === 0, `${why} cannot send: refused with what to do, nothing fetched or signed`);
  }
  const long = bridgeWorld({ orgs: ORGS, device: () => ({}) });
  let le = null;
  try { await long.COMMANDS.send_to_org({ path: "x", slug: "acme", note: "x".repeat(1001) }); } catch (e) { le = e; }
  check(le && /up to 1,000 characters/.test(le.message) && long.calls.publish.length === 0, "a note past 1,000 characters is refused before anything is sent");
  const signedOut = bridgeWorld({ orgs: ORGS, token: null, device: () => ({}) });
  signedOut.COMMANDS.send_to_org({ path: "x", slug: "acme" });
  await tick();
  check(signedOut.signIns() === 1 && signedOut.calls.publish.length === 0, "signed out, it asks for a sign-in");

  // The signing's refusals, in words.
  const fail = async (e) => {
    const r = bridgeWorld({ orgs: ORGS, device: () => { throw e; } });
    try { await r.COMMANDS.send_to_org({ path: "x", slug: "acme" }); return null; } catch (err) { return err.message; }
  };
  check(/Could not reach Krate, so nothing was sent/.test(await fail(Object.assign(new Error("x"), { reason: "offline" }))), "offline: nothing was sent, try again");
  check(/An owner of Acme can make you a developer in People/.test(await fail(Object.assign(new Error("a reviewer cannot publish"), { reason: "role-cannot", status: 403 }))), "the hub refusing the role: who can change it");
  check(/larger than Acme takes from a browser\. Send it from Krate Studio on your computer/.test(await fail(Object.assign(new Error(""), { reason: "too-large", status: 413 }))), "too large for the browser door: the desktop path");
  check(await fail(Object.assign(new Error("this browser's earlier permission to publish was withdrawn, so nothing was sent -- send it again to ask afresh"), { reason: "publisher-withdrawn" })) === "This browser's earlier permission to publish was withdrawn, so nothing was sent -- send it again to ask afresh.", "anything else: the signing's own sentence");
}

/* ---- opened for one organisation ----------------------------------------- */
{
  const w = bridgeWorld({ url: "https://krate.tech/app/?org=Acme&start=sheet" });
  check(w.win.krOrgPick() === "acme", "?org= names the organisation Studio was opened for");
  check(w.win.krStartSheet() === true && w.win.krStartSheet() === false, "?start=sheet opens the spreadsheet start once");
  const bad1 = bridgeWorld({ url: "https://krate.tech/app/?org=../../x" });
  check(bad1.win.krOrgPick() === null, "a slug that is not one is ignored");
  check(!/send_to_org|sheet_request|create_app/.test(between(REGION, "try {", "window.krOrgPick")), "reading the link sends, signs and builds nothing");
}

/* ---- a spreadsheet's build ------------------------------------------------ */
const sheetCsv = () => {
  let t = "Item,Qty,Received,Supplier\n";
  for (let i = 1; i <= 1200; i++) t += `"Part ${i}, M${i % 9}",${i},2026-03-${String((i % 28) + 1).padStart(2, "0")},s${i}@example.com\n`;
  return t;
};
const fileOf = (name, text) => () => {
  const bytes = typeof text === "string" ? new TextEncoder().encode(text) : text;
  return { name, size: bytes.length, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length) };
};
{
  const w = bridgeWorld({ orgs: ORGS, file: fileOf("Stock list.csv", sheetCsv()) });
  const got = await w.COMMANDS.sheet_pick();
  check(got.file === "Stock list.csv" && got.rows === 1200 && got.problem === null && got.name === "Stock list", "a .csv is read in the tab: its rows, and a name from the file");
  check(JSON.stringify(got.columns) === JSON.stringify([{ name: "Item", type: "text" }, { name: "Qty", type: "number" }, { name: "Received", type: "date" }, { name: "Supplier", type: "email" }]), "the columns with their kinds");
  check(got.sample.length === 5 && got.sample[0][0] === "Part 1, M1", "and the first rows");
  const made = await w.COMMANDS.sheet_request({ name: "Stock", purpose: "Know what the warehouse holds.", org: "acme" });
  check(made.attachment === "stock-list.csv" && made.rows === 1200, "the whole sheet waits as an attachment named after the file");
  // The build itself: what goes to the build service.
  const started = await w.COMMANDS.create_app({ request: made.request, session: "s1", attachments: [made.attachment] });
  const body = w.calls.build[0] && w.calls.build[0].body;
  check(started === "watching job-1" && body && body.request === made.request, "Make starts a normal build from that request");
  check(body.request.length <= 2000 && /1,200 rows and 4 columns: Item \(text\), Qty \(number\), Received \(date\), Supplier \(email\)\./.test(body.request), "the request carries the columns and their kinds");
  check(body.request.includes("Part 1, M1 | 1 | 2026-03-02 | s1@example.com") && body.request.includes("Part 5, M5"), "and a sample of rows");
  check(!body.request.includes("Part 6, M6") && !body.request.includes("Part 1200"), "and not the rest of the sheet");
  check(/Acme's team data with store\.org/.test(body.request), "made for Acme: kept as its team data");
  const att = body.attachments || [];
  const csv = att.length === 1 ? Buffer.from(att[0].bytes, "base64").toString("utf8") : "";
  check(att.length === 1 && att[0].name === "stock-list.csv" && csv.trim().split("\n").length === 1201 && csv.includes('"Part 1200, M3",1200'), "the whole sheet goes once, as the app's asset");
  const notMine = bridgeWorld({ orgs: ORGS, file: fileOf("a.csv", "x,y\n1,2\n") });
  await notMine.COMMANDS.sheet_pick();
  let ne = null;
  try { await notMine.COMMANDS.sheet_request({ name: "A", org: "delta" }); } catch (e) { ne = e; }
  check(ne && /You cannot make apps for delta/.test(ne.message) && notMine.attached.size === 0, "records cannot be kept for an organisation this person does not make apps for");
  const local = await notMine.COMMANDS.sheet_request({ name: "A", org: "" });
  check(/store\.sql/.test(local.request), "or kept on the person's own computer");
}
{
  // Refused by size before a byte is read.
  let read = false;
  const w = bridgeWorld({ file: () => ({ name: "Huge.xlsx", size: 6 * 1024 * 1024, arrayBuffer: async () => { read = true; return new ArrayBuffer(0); } }) });
  let e = null;
  try { await w.COMMANDS.sheet_pick(); } catch (err) { e = err; }
  check(e && e.refusal && /Huge\.xlsx is 6 MB\. Krate takes a spreadsheet up to 5 MB/.test(e.message) && !read, "over 5 MB: refused by name and size, the file never read");
  let rows = "n\n";
  for (let i = 0; i < 20001; i++) rows += `${i}\n`;
  const many = bridgeWorld({ file: fileOf("many.csv", rows) });
  const got = await many.COMMANDS.sheet_pick();
  check(/many\.csv has 20,001 rows\. Krate takes up to 20,000/.test(got.problem || ""), "over 20,000 rows: the summary says so");
  let me = null;
  try { await many.COMMANDS.sheet_request({ name: "Many" }); } catch (err) { me = err; }
  check(me && /20,000/.test(me.message) && many.attached.size === 0, "and no build can start from it");
  const cancelled = bridgeWorld({ file: null });
  check((await cancelled.COMMANDS.sheet_pick()) === null, "a cancelled picker is nothing");
  // A workbook with an empty first sheet opens on the first one with rows.
  const xlsx = new Uint8Array(readFileSync("docs/landing/studio/sheet-fixture.xlsx"));
  const book = bridgeWorld({ file: fileOf("Stock.xlsx", xlsx) });
  const first = await book.COMMANDS.sheet_pick();
  check(first.sheets.join() === "Stock,Empty,People" && first.sheet === 0 && first.rows === 3, "an .xlsx: every sheet listed, the first with rows chosen");
  const other = await book.COMMANDS.sheet_use({ sheet: 1 });
  check(other.sheet === 1 && /The sheet "Empty" has no rows to read/.test(other.problem || ""), "an empty sheet says so, by name");
  check((await book.COMMANDS.sheet_use({ sheet: 2 })).rows === 1, "and another sheet can be chosen");
}

/* ---- Studio's Send menu (app.js) ----------------------------------------- */
function el(tag = "div") {
  const n = {
    tag, children: [], dataset: {}, attrs: {}, listeners: {}, className: "", textContent: "", value: "", disabled: false, hidden: false, after: [],
    classList: { _s: new Set(), add(c) { this._s.add(c); }, remove(c) { this._s.delete(c); }, toggle(c, on) { (on === undefined ? !this._s.has(c) : on) ? this._s.add(c) : this._s.delete(c); }, contains(c) { return this._s.has(c); } },
    appendChild(c) { this.children.push(c); c.parent = this; return c; },
    setAttribute(k, v) { this.attrs[k] = String(v); },
    removeAttribute(k) { delete this.attrs[k]; },
    addEventListener(k, fn) { this.listeners[k] = fn; },
    insertAdjacentElement(_where, c) { this.after.push(c); c.parent = this; return c; },
    querySelector(sel) { return sel === ".pop-val" ? this.val : null; },
    remove() { this.removed = true; },
    focus() { this.focused = true; },
    set innerHTML(v) { if (!v) this.children = []; },
  };
  return n;
}
const SEND = between(appSrc, "const orgTargets = { list: null, asked: null };", "/* ---- the company's apps (IC-953)")
  + between(appSrc, "async function openSendSheet(", "/* ---- start from a spreadsheet (G6)");
function sendWorld({ targets, answers = {}, pick = null, tauri = false }) {
  const calls = [];
  const toasts = [];
  let items = null;
  const invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    const a = answers[cmd];
    if (cmd === "org_targets") return targets;
    const v = typeof a === "function" ? a(args) : a;
    if (v instanceof Error) throw v;
    return v;
  };
  const window = { __KRATE_BRIDGE__: () => {}, krOrgPick: () => pick };
  const state = { session: { result: { path: "https://build.krate.tech/build/abc/file", name: "Stock" } } };
  const run = new Function(
    "window", "tauri", "invoke", "state", "currentApp", "popMenu", "$", "toastish", "navigator", "document", "sendCard", "openPublishSheet", "makeWrap", "setTimeout",
    `${SEND}\nreturn { openSendSheet, refreshOrgTargets, orgTargets };`,
  );
  const api = run(window, tauri, invoke, state, () => state.session.result, (_a, list) => { items = list; }, () => el("button"), (t) => toasts.push(String(t)), { clipboard: { writeText: async () => {} } },
    { createElement: (t) => el(t) }, () => {}, () => {}, () => {}, (fn) => setTimeout(fn, 0));
  return { ...api, calls, toasts, items: () => items, state };
}
const rowsOf = (items) => items.filter((i) => /for approval$/.test(i.label || ""));
{
  const w = sendWorld({ targets: [{ slug: "acme", name: "Acme", role: "developer" }, { slug: "beta", name: "Beta Ltd", role: "owner" }], pick: "beta" });
  await w.openSendSheet(null, null, el());
  check(w.calls[0][0] === "org_targets", "in a tab, the menu asks the bridge which organisations it may send to");
  const rows = rowsOf(w.items());
  check(rows.map((r) => r.label).join(" | ") === "Send to Beta Ltd for approval | Send to Acme for approval", "one row per organisation, the one Studio was opened for first");
  // Pressing the row opens a line for the release note; Send sends it.
  const row = el("button");
  row.val = el("span");
  const label = el("span");
  label.textContent = rows[1].label;
  w.calls.length = 0;
  rows[1].run(row, label);
  const box = row.after[0];
  check(box && box.className === "pop-note" && box.children[0].placeholder === "What changed (optional)" && box.children[0].maxLength === 1000, "a line for a note opens under the row");
  check(!w.calls.some(([c]) => c === "send_to_org"), "nothing is sent until Send");
  box.children[0].value = " Now imports the sheet ";
}
{
  const w = sendWorld({
    targets: [{ slug: "acme", name: "Acme", role: "developer" }],
    answers: { send_to_org: (a) => ({ org: a.slug, state: "waiting-for-approval", words: "It waits for a second person to approve it", digest: "d", waiting_url: "https://krate.tech/org/?o=acme&tab=myapps" }) },
  });
  await w.openSendSheet(null, null, el());
  const row = el("button");
  row.val = el("span");
  const label = el("span");
  rowsOf(w.items())[0].run(row, label);
  const box = row.after[0];
  box.children[0].value = " Now imports the sheet ";
  await box.children[1].listeners.click();
  const sent = w.calls.find(([c]) => c === "send_to_org");
  check(sent && sent[1].slug === "acme" && sent[1].path === "https://build.krate.tech/build/abc/file" && sent[1].name === "Stock" && sent[1].note === "Now imports the sheet", "Send sends the built app with its name and the note");
  check(label.textContent === "Sent. It waits for a second person to approve it." && row.val.textContent === "see it in the dashboard", "the row says: Sent. It waits for a second person to approve it.");
  check(w.toasts.includes("Sent to Acme. It waits for a second person to approve it."), "and so does the toast");
  check(box.removed === true && row.dataset.waiting === "https://krate.tech/org/?o=acme&tab=myapps", "the note line closes, and the row now links to the console");
  rowsOf(w.items())[0].run(row, label);
  await tick();
  check(w.calls.some(([c, a]) => c === "open_external" && a.url === "https://krate.tech/org/?o=acme&tab=myapps"), "pressing it again opens the console");
}
{
  // Nobody to send to: no row. (Sabotage: offer a reviewer in the bridge,
  // and the bridge's own test above fails; this is the menu's half.)
  const w = sendWorld({ targets: [] });
  await w.openSendSheet(null, null, el());
  check(rowsOf(w.items()).length === 0 && !w.items().some((i) => /Publish to|for approval/.test(i.label || "")), "a person who may send nowhere is not offered Send to an organisation");
  // On a computer the row sends at once, as before, with no note line.
  const d = sendWorld({ targets: [{ slug: "acme", name: "Acme", role: "owner" }], tauri: true, answers: { send_to_org: { state: "waiting-for-approval", words: "Waiting for bob or another reviewer to approve it", waiting_url: "u" } } });
  await d.openSendSheet(null, null, el());
  const row = el("button");
  row.val = el("span");
  const label = el("span");
  rowsOf(d.items())[0].run(row, label);
  await tick();
  const sent = d.calls.find(([c]) => c === "send_to_org");
  check(sent && JSON.stringify(Object.keys(sent[1])) === '["path","slug"]' && row.after.length === 0, "on a computer the engine's command gets the file and the organisation, as before");
}

/* ---- the redesigned Share sheet (redesign.js) ------------------------------
 * It replaces app.js's menu, so it must offer the same list and run the
 * same send. (The page itself was driven in a browser for the report:
 * a reviewer-only account sees no fourth way.) */
{
  const rd = readFileSync("studio/ui/redesign.js", "utf8");
  const paint = between(rd, "function paintOrg() {", "\n  }\n");
  check(/data-w="org" role="tab"[^>]*hidden>/.test(rd), "Share has a fourth way, Send for approval, hidden by default");
  check(/orgTargets && orgTargets\.list/.test(rd) && /way\.hidden = !orgs\.length/.test(paint), "shown only for the organisations the bridge or engine lists");
  check(/sendToOrg\(shareApp, org, go, st, note \? note\.value\.trim\(\) : ""\)/.test(paint), "and it runs app.js's own send, with the note");
  check(/desktopApp\(\) \? "" : '<label>What changed/.test(paint), "the note is offered only where it is carried (a tab)");
  check(/paintOrg\(\);/.test(between(rd, "function openShare(", "\n  }\n")), "every Share paints it");
}

/* ---- Studio's spreadsheet start (app.js) ---------------------------------- */
{
  check(/id="homeSheetLine" hidden><a href="#" id="homeSheet">Start from a spreadsheet<\/a>/.test(html), "Home has \"Start from a spreadsheet\" under the box, hidden until the bridge is there");
  check(/id="sheetSheet"[\s\S]*Start from a spreadsheet[\s\S]*id="sheetChoose"[\s\S]*id="sheetMake"[^>]*disabled>Make the app</.test(html), "and its sheet: choose a file, then Make the app");
  const SHEET = between(appSrc, "/* ---- start from a spreadsheet (G6)", "/* Opened for one organisation");
  const ids = {};
  const $ = (id) => (ids[id] ||= el(id === "sheetFor" || id === "sheetTab" ? "select" : "div"));
  const calls = [];
  const summary = { file: "Stock.csv", sheets: ["Stock"], sheet: 0, rows: 1200, columns: [{ name: "Item", type: "text" }, { name: "Qty", type: "number" }], sample: [["Bolts", "4"], ["Nuts", "9"]], problem: null, name: "Stock" };
  let started = 0;
  const invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === "sheet_pick") return summary;
    if (cmd === "sheet_request") return { request: "Stock: the request", attachment: "stock.csv", rows: 1200 };
    return null;
  };
  const state = { attachments: [] };
  const window = { __KRATE_BRIDGE__: () => {}, krOrgPick: () => "acme", krStartSheet: () => false };
  const run = new Function("window", "tauri", "invoke", "$", "state", "document", "orgTargets", "refreshOrgTargets", "webSignInFirst", "clip", "renderAttachments", "startFromHome", "syncSendReady", "setTimeout",
    `${SHEET}\nreturn { openSheetStart, chooseSheetFile, makeFromSheet, sheetStart };`);
  const S = run(window, false, invoke, $, state, { createElement: (t) => el(t) }, { list: [{ slug: "acme", name: "Acme", role: "developer" }] }, async () => [], () => false, (s, n) => String(s).slice(0, n), () => {}, async () => { started++; }, () => {}, () => {});
  check(ids.homeSheetLine && ids.homeSheetLine.attrs.hidden === undefined, "in a tab the line is shown");
  await S.openSheetStart();
  const opts = ids.sheetFor.children;
  check(opts.length === 2 && opts[0].textContent === "Just me, on my computer" && opts[1].textContent === "Acme, as team data" && opts[1].selected, "who keeps the records: the person, or Acme as team data, chosen for Acme");
  check(ids.sheetMake.disabled === true, "Make waits for a file");
  await S.chooseSheetFile();
  check(ids.sheetFile.textContent === "Stock.csv · 1,200 rows · 2 columns" && ids.sheetName.value === "Stock", "the file, its rows and columns, and a name to start from");
  const table = ids.sheetCols.children[0];
  const heads = table.children[0].children[0].children.map((th) => `${th.children[0].textContent}/${th.children[1].textContent}`);
  check(heads.join(",") === "Item/text,Qty/number", "each column's name over its kind");
  check(table.children[1].children.length === 2 && table.children[1].children[0].children[0].textContent === "Bolts", "and the first rows under them");
  check(/and 1,198 more\. Every row goes into the app; only these first ones go to the AI\./.test(ids.sheetCols.children[1].textContent), "it says the rest go into the app, not to the AI");
  check(ids.sheetMake.disabled === false, "Make is ready");
  $("sheetPurpose").value = "Know what the warehouse holds.";
  ids.sheetFor.value = "acme";
  await S.makeFromSheet();
  const asked = calls.find(([c]) => c === "sheet_request");
  check(asked && asked[1].name === "Stock" && asked[1].purpose === "Know what the warehouse holds." && asked[1].org === "acme", "Make asks for the request with the name, the sentence and the organisation");
  check(state.attachments.join() === "stock.csv" && ids.homePrompt.value === "Stock: the request" && started === 1, "and starts a normal build from Home with the sheet attached");
  check(ids.sheetSheet.classList.contains("hidden"), "the sheet closes");
  // A sheet Krate will not start from: said, and Make stays off.
  summary.problem = "many.csv has 20,001 rows. Krate takes up to 20,000: keep the rows you need, save it, and choose it again.";
  await S.chooseSheetFile();
  check(ids.sheetProblem.textContent === summary.problem && !ids.sheetProblem.classList.contains("hidden") && ids.sheetMake.disabled === true, "too many rows: the words, and no Make");
}

/* ---- the console's New app page ------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=build", DASH, { token: "tok-carol", routes: orgRoutes("github:carol") });
  const links = p.document.body.all((n) => n.tagName === "A");
  const describe = links.find((a) => a.textContent === "Describe it");
  const sheet = links.find((a) => a.textContent === "Start from a spreadsheet");
  check(describe && describe.attrs.href === "/app/?org=acme", "New app: Describe it opens Studio in the browser for this organisation");
  check(sheet && sheet.attrs.href === "/app/?org=acme&start=sheet", "and Start from a spreadsheet opens its spreadsheet start");
  check(/Four ways to start/.test(p.el("pane").shown), "four ways to start");
}

/* ---- an owner's browser approves waiting publishers ------------------------ */
async function consoleWith(publishers, duties) {
  const asked = [];
  const KrateOrgDevice = { duties: async (slug) => { asked.push(slug); return duties; } };
  const p = await page("https://krate.tech/org/?o=acme&tab=home", DASH, {
    token: "tok-alice",
    routes: { ...orgRoutes("github:alice"), "GET /orgs/acme/publishers": () => [200, publishers] },
    extra: { KrateOrgDevice },
  });
  await settle();
  return { p, asked };
}
{
  const { p, asked } = await consoleWith({ by_hand: false, approvers: ["github:alice"], waiting: [{ principal: "github:carol", asked_at: 1760000000 }], publishers: [] }, { approved: ["github:carol"], keyGrants: 0, ringGrants: 0, withdrawn: 0 });
  check(asked.join() === "acme", "an owner's console asks this browser to do its share when someone is waiting to publish");
  check(/carol can publish to Acme now\./.test(p.el("toast").shown), "and says who can publish now");
  const hand = await consoleWith({ by_hand: true, approvers: ["github:alice"], waiting: [{ principal: "github:carol", asked_at: 1760000000 }], publishers: [] }, { approved: [] });
  check(hand.asked.length === 0, "approving by hand: nothing is signed before an owner says yes");
  const none = await consoleWith({ by_hand: false, approvers: ["github:alice"], waiting: [], publishers: [] }, { approved: [] });
  check(none.asked.length === 0, "nobody waiting: nothing is done");
}

if (bad) {
  console.log(`\n${bad} check(s) failed`);
  process.exit(1);
}
console.log("\nall send-to-organisation checks passed");
// The console pages leave their timers running; the checks are done.
process.exit(0);
