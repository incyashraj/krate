// One app, many versions, in Studio (IC-976): the shipped studio/ui/app.js
// and docs/landing/studio/bridge.js, run against stand-ins for the hub and
// the engine.
//
// - An app this person already published says "Publish update (v3)" before
//   anything goes up, and the build already at its link says so instead;
//   a first publish reads as it always did.
// - After a publish, the link Studio keeps and copies is the APP's link, and
//   an update says that everyone with the link now gets the new version.
// - Details lists the versions newest first: number, when, size, the
//   current one marked, Open for each, and Make current with one line that
//   says what it does before it does it; the hub's refusal is said in words.
// - In a tab the bridge finds the app by the id in its manifest and names
//   which version this very file is, by its sha256, the way the desktop
//   does; it hands back the app's link, never the 64-hex build address.
//
//   node docs/landing/studio/versions-test.mjs
import { readFileSync } from "node:fs";
import { createHash, webcrypto } from "node:crypto";

const appSrc = readFileSync("studio/ui/app.js", "utf8");
const bridgeSrc = readFileSync("docs/landing/studio/bridge.js", "utf8");
const redesignSrc = readFileSync("studio/ui/redesign.js", "utf8");
const css = readFileSync("studio/ui/redesign.css", "utf8");

let bad = 0;
function check(ok, name) {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
}
const tick = async () => { for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 0)); };
function between(src, from, to) {
  const a = src.indexOf(from);
  const b = src.indexOf(to, a + 1);
  if (a < 0 || b < 0) throw new Error(`cannot find ${from} .. ${to}`);
  return src.slice(a, b);
}
function topFn(src, name) {
  const m = new RegExp(`\\n(async )?function ${name}\\(`).exec(src);
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

/* ---- a stand-in DOM, enough for the versions list -------------------- */
function element(tag) {
  const el = {
    tagName: tag.toUpperCase(), children: [], parent: null, listeners: {}, dataset: {}, attrs: {},
    hidden: false, disabled: false, className: "", type: "", title: "", _text: "",
    get textContent() { return this._text + this.children.map((c) => c.textContent).join(""); },
    set textContent(v) { this._text = String(v); this.children = []; },
    appendChild(c) { c.parent = this; this.children.push(c); return c; },
    append(...cs) { cs.forEach((c) => this.appendChild(c)); },
    replaceChildren() { this.children = []; this._text = ""; },
    remove() { if (this.parent) this.parent.children = this.parent.children.filter((x) => x !== this); this.parent = null; },
    setAttribute(k, v) { this.attrs[k] = String(v); },
    addEventListener(k, fn) { this.listeners[k] = fn; },
    click() { return this.listeners.click && this.listeners.click(); },
    get isConnected() { return true; },
    all(pred) { const out = []; const walk = (n) => n.children.forEach((c) => { if (pred(c)) out.push(c); walk(c); }); walk(this); return out; },
    querySelector(sel) { return this.all((n) => n.className.split(/\s+/).includes(sel.slice(1)))[0] || null; },
  };
  return el;
}
const byClass = (root, cls) => root.all((n) => n.className.split(/\s+/).includes(cls));
const button = (root, text) => root.all((n) => n.tagName === "BUTTON" && n.textContent === text)[0];

/* ---- app.js: the versions block, as shipped -------------------------- */
const BLOCK = between(appSrc, "/* ---- an app's versions (IC-976)", "/* ---- end of an app's versions ---- */");
function studio({ answers = {}, tauri = true } = {}) {
  const calls = [];
  const toasts = [];
  const invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    const a = answers[cmd];
    const v = typeof a === "function" ? a(args) : a;
    if (v instanceof Error) throw v;
    return v;
  };
  const api = new Function(
    "invoke", "tauri", "window", "document", "toastish", "clip",
    `${BLOCK}\nreturn { appVersions, versionsOf, forgetVersions, publishPlan, publishAnswer, publishedWords, renderVersions, versionWhen, versionSize };`,
  )(invoke, tauri ? {} : null, {}, { createElement: element }, (t) => toasts.push(String(t)), (s, n) => String(s).slice(0, n));
  return { calls, toasts, ...api };
}

const LINE = (extra = {}) => ({
  id: "v0123456789a", name: "Notes", url: "https://hub.krate.tech/a/v0123456789a", current: 2, versions: 2,
  history: [
    { n: 1, version: "0.1.0", at: 1761900000, size: 23 * 1024, hash: "a".repeat(64), url: "https://hub.krate.tech/a/aaaaaaaaaaaa", current: false },
    { n: 2, version: "0.2.0", at: 1762000000, size: 24 * 1024, hash: "b".repeat(64), url: "https://hub.krate.tech/a/bbbbbbbbbbbb", current: true },
  ],
  this_version: null, next: 3, ...extra,
});

/* What the publish control says. */
{
  const s = studio();
  const first = s.publishPlan(null);
  check(first.label === "Publish" && !first.update, "an app never published: Publish, as today");
  const upd = s.publishPlan(LINE());
  check(upd.update && upd.label === "Publish update (v3)", "published before: Publish update (v3)");
  check(upd.sub === "Your link stays the same. Everyone who has it gets v3.", "and says the link stays and who gets it");
  check(upd.url === "https://hub.krate.tech/a/v0123456789a", "with the app's own link");
  const same = s.publishPlan(LINE({ this_version: 2 }));
  check(!same.update && same.same && /This build is v2/.test(same.sub), "this very build already published: no update offered");
  check(!/\b(line|hash)\b/i.test([upd.label, upd.sub, same.sub].join(" ")), "no machinery words on screen");
}

/* The hub's answer, from either engine, and the words after it. */
{
  const s = studio();
  check(s.publishAnswer("https://hub.krate.tech/a/abc").url === "https://hub.krate.tech/a/abc", "an engine that answers with the link alone still works");
  const out = s.publishAnswer({ url: "https://hub.krate.tech/a/v0123456789a", version: 3, updated: true, version_url: "https://hub.krate.tech/a/cccc" });
  check(out.version === 3 && out.updated === true, "an update's number comes through");
  check(s.publishedWords(out) === "Updated to v3. Everyone with the link now gets it. The link is copied.", "an update says everyone with the link now gets it");
  check(s.publishedWords(s.publishAnswer({ url: "u", version: 1, updated: false })) === "Link copied. It's under Share whenever you need it.", "a first publish reads as today");
}

/* Asked once per file, quietly null when the hub has nothing. */
{
  let n = 0;
  const s = studio({ answers: { app_versions: () => { n += 1; return LINE(); } } });
  const a = await s.versionsOf({ path: "/apps/notes.krate" });
  const b = await s.versionsOf({ path: "/apps/notes.krate" });
  check(a && a.id === "v0123456789a" && b === a && n === 1, "asked of the engine once, by the file's path");
  check(s.calls[0][0] === "app_versions" && s.calls[0][1].path === "/apps/notes.krate", "the door is app_versions with the path");
  s.forgetVersions({ path: "/apps/notes.krate" });
  await s.versionsOf({ path: "/apps/notes.krate" });
  check(n === 2, "forgotten after a publish, asked again");
  const off = studio({ answers: { app_versions: new Error("could not reach krate.tech") } });
  check((await off.versionsOf({ path: "/x.krate" })) === null, "offline: no update offered, nothing thrown");
  const none = studio({ answers: { app_versions: null } });
  check((await none.versionsOf({ path: "/x.krate" })) === null, "not published yet: null");
  const bare = studio({ tauri: false });
  check((await bare.versionsOf({ path: "/x.krate" })) === null && bare.calls.length === 0, "no engine and no bridge: nothing is asked");
}

/* The versions list in Details. */
{
  const viewAfter = (args) => ({ ...LINE(), current: args.version, history: LINE().history.map((v) => ({ ...v, current: v.n === args.version })) });
  const s = studio({ answers: { app_make_current: viewAfter, open_external: undefined } });
  const host = element("section");
  const changed = [];
  s.renderVersions(host, LINE({ this_version: 2 }), (next) => changed.push(next));
  check(host.hidden === false, "shown for the publisher's app");
  check(/Versions/.test(byClass(host, "vers-title")[0].textContent) && /Your link opens v2\. Only you see this list\./.test(byClass(host, "vers-sub")[0].textContent), "headed Versions, saying what the link opens and who sees it");
  const rows = byClass(host, "vers-row");
  check(rows.map((r) => r.dataset.n).join(",") === "2,1", "newest first");
  check(/Current/.test(rows[0].textContent) && !/Current/.test(rows[1].textContent), "the current one is marked");
  check(/This build/.test(rows[0].textContent), "and so is the build on screen");
  check(/24 KB/.test(rows[0].textContent) && /23 KB/.test(rows[1].textContent), "each says its size");
  check(byClass(rows[1], "vers-meta")[0].textContent.includes(s.versionWhen(1761900000)) && s.versionWhen(1761900000).length > 4, "and when");
  check(button(rows[0], "Make current") === undefined && button(rows[1], "Make current"), "Make current only on versions that are not current");
  await button(rows[1], "Open").click();
  check(s.calls.some(([c, a]) => c === "open_external" && a.url === "https://hub.krate.tech/a/aaaaaaaaaaaa"), "Open opens that version's own link");

  // Make current: the line first, then the press.
  button(rows[1], "Make current").click();
  const ask = byClass(rows[1], "vers-ask")[0];
  check(ask && ask.textContent.startsWith("Everyone with the link gets v1 again."), "one line says what it does before it does it");
  check(!s.calls.some(([c]) => c === "app_make_current"), "nothing changes until it is pressed");
  await button(ask, "Make v1 current").click();
  await tick();
  const asked = s.calls.find(([c]) => c === "app_make_current");
  check(asked && asked[1].line === "v0123456789a" && asked[1].version === 1, "the hub is asked by the app and the version number");
  check(changed.length === 1 && changed[0].current === 1 && changed[0].this_version === 2, "the list hears the new current version, and still knows this build");
  check(s.toasts.includes("v1 is current. Everyone with the link gets it."), "and says so");

  // Cancel takes the line away; a refusal is said in the hub's words.
  const r2 = studio({ answers: { app_make_current: new Error("only the app's publisher can choose its current version") } });
  const host2 = element("section");
  r2.renderVersions(host2, LINE(), () => {});
  const old = byClass(host2, "vers-row")[1];
  button(old, "Make current").click();
  button(byClass(old, "vers-ask")[0], "Cancel").click();
  check(!byClass(old, "vers-ask").length, "Cancel takes it back");
  button(old, "Make current").click();
  await button(byClass(old, "vers-ask")[0], "Make v1 current").click();
  await tick();
  check(/only the app's publisher/.test(byClass(old, "vers-err")[0].textContent), "a refusal is said in its own words");
  check(button(byClass(old, "vers-ask")[0], "Make v1 current").disabled === false, "and the press is ready again");

  const empty = element("section");
  s.renderVersions(empty, null);
  check(empty.hidden === true, "nobody else's app, nothing published: no list");
}

/* ---- app.js: the publish sheet and the publish itself ---------------- */
{
  const els = {};
  const $ = (id) => (els[id] ||= Object.assign(element("div"), { id, value: "", checked: false, classList: { add() {}, remove() {}, contains: () => false } }));
  const heads = { "#publishSheet h2": element("h2"), "#pubForm .pub-sub": element("p") };
  const app = { path: "/apps/notes.krate", name: "notes.krate" };
  const fns = ["pubGoLabel", "paintPublishPlan", "publishFromSheet"].map((n) => topFn(appSrc, n)).join("\n");
  const toasts = [];
  const copied = [];
  const persisted = [];
  const state = { account: { signed_in: true }, session: { result: { ...app } } };
  const publishOut = { url: "https://hub.krate.tech/a/v0123456789a", version: 3, updated: true, version_url: "https://hub.krate.tech/a/cccccccccccc" };
  const world = new Function(
    "BLOCK", "$", "document", "state", "currentApp", "invoke", "tauri", "window", "navigator", "toastish", "persist", "clip", "publishWords", "showPubSignin",
    `const pubState = { shotPath: null, iconPath: null, plan: null };\n${BLOCK}\n${fns}\nreturn { pubState, paintPublishPlan, publishFromSheet, appVersions };`,
  );
  const sent = [];
  const api = world(
    BLOCK, $, { querySelector: (sel) => heads[sel] || null, createElement: element }, state, () => state.session.result,
    async (cmd, args) => { sent.push([cmd, args]); if (cmd === "app_versions") return LINE(); if (cmd === "publish") return publishOut; return null; },
    {}, {}, { clipboard: { writeText: async (t) => copied.push(t) } }, (t) => toasts.push(t), () => persisted.push(1),
    (s, n) => String(s).slice(0, n), (e) => String(e), () => {},
  );
  await api.paintPublishPlan(app);
  check(heads["#publishSheet h2"].textContent === "Publish update", "the publish sheet is headed Publish update");
  check($("pubGo").textContent === "Publish update (v3)", "its button says the version it will become");
  check(heads["#pubForm .pub-sub"].textContent === "Your link stays the same. Everyone who has it gets v3.", "and what happens to the link");
  await api.publishFromSheet();
  check(state.session.result.share_url === publishOut.url, "the link Studio keeps is the app's own link");
  check(copied[0] === publishOut.url, "and that is the one copied");
  check(toasts[0] === "Updated to v3. Everyone with the link now gets it. The link is copied.", "an update says everyone with the link now gets it");
  check(state.lastPublish && state.lastPublish.version === 3 && state.session.result.published_version === 3, "the version it became is kept for Share to say");
  check(!api.appVersions.byPath.has(app.path), "and the versions are asked again next time");

  // No versions: a first publish reads as today. The sheet opens the way
  // openPublishSheet opens it: the button ready, saying Publish.
  heads["#publishSheet h2"].textContent = "x";
  $("pubGo").disabled = false;
  $("pubGo").textContent = "Publish";
  const first = world(
    BLOCK, $, { querySelector: (sel) => heads[sel] || null, createElement: element }, state, () => state.session.result,
    async (cmd) => (cmd === "app_versions" ? null : { url: "https://hub.krate.tech/a/v0123456789a", version: 1, updated: false }),
    {}, {}, { clipboard: { writeText: async () => {} } }, (t) => toasts.push(t), () => {}, (s, n) => String(s).slice(0, n), (e) => String(e), () => {},
  );
  await first.paintPublishPlan(app);
  check(heads["#publishSheet h2"].textContent === "Publish" && $("pubGo").textContent === "Publish", "a first publish says Publish");
  check(/Publishing again later updates it/.test(heads["#pubForm .pub-sub"].textContent), "and that publishing again updates it");
}

/* ---- redesign.js: the Share sheet says it too ------------------------ */
// The sheet's wiring is drawn and pressed in a browser for the pictures;
// here, that every label it shows comes from app.js's one plan, so the
// two can never say different things.
check(/const makeLabel = \(\) => \{ const p = planOf\(\); return p && p\.update \? p\.label : "Make a link"; \}/.test(redesignSrc), "Share's link button takes the plan's words");
check(/const pubLabel = \(\) => \{ const p = planOf\(\); return p && p\.update \? p\.label : "Publish"; \}/.test(redesignSrc), "Share's Publish button takes the plan's words");
check(!/textContent = "Make a link"/.test(redesignSrc) && !/textContent = "Publish";/.test(redesignSrc), "no reset puts the old words back over an update");
check(/versionsOf\(asked\)\.then/.test(redesignSrc), "Share asks for the versions when it opens");

/* ---- the stylesheet --------------------------------------------------- */
{
  const rules = between(css, "/* An app's versions (IC-976)", "@media (max-width: 560px) { .dt2-rows");
  check(!/#[0-9a-f]{3,8}\b|rgba?\(/i.test(rules), "the versions list uses tokens only");
  check(/\.vers-acts \.dt2-link \{ min-height: 32px; \}/.test(rules) && /\.vers-btns \.btn \{ height: 34px;/.test(rules), "its controls are big enough to press");
}

/* ---- bridge.js: the same in a tab ------------------------------------- */
{
  const fixture = new Uint8Array(readFileSync("cloud/worker/test/fixtures/notes-0.1.0.krate"));
  const sha = createHash("sha256").update(fixture).digest("hex");
  const pieces = ["zipU16", "zipU32", "zipEntries", "zipRead", "publishWebApp", "manifestAppId", "withThisVersion", "webAppVersions"].map((n) => topFn(bridgeSrc, n)).join("\n");
  const cmds = ["publish", "app_versions", "app_make_current"].map(method).join(",\n");
  function tab({ token = "tok", lines = null, publishAnswer = null, current = null } = {}) {
    const sent = [];
    const bridge = { token, jobResult: null };
    const fetchStub = async (url, init = {}) => {
      sent.push({ url: String(url), init });
      if (String(url).startsWith("https://builder.example/")) return { ok: true, arrayBuffer: async () => fixture.buffer.slice(0) };
      if (String(url).endsWith("/publish")) return { ok: true, json: async () => publishAnswer };
      return { ok: false, status: 404, text: async () => "" };
    };
    const hub = async (path, opts = {}) => {
      sent.push({ hub: path, opts });
      if (path.startsWith("/lines?app=")) {
        if (!lines) { const e = new Error('{"error":"not published yet"}'); e.status = 404; throw e; }
        return lines;
      }
      if (/\/current$/.test(path)) {
        if (current instanceof Error) throw current;
        return current;
      }
      throw new Error("no route");
    };
    const refuse = (m) => { const e = new Error(m); e.refusal = true; return Promise.reject(e); };
    const run = new Function(
      "bridge", "fetch", "hub", "refuse", "crypto", "TextDecoder", "currentWebApp", "fileFetchError", "DecompressionStream", "Blob", "Response", "HUB",
      `${pieces}\nconst C = {\n${cmds}\n};\nreturn { C, manifestAppId };`,
    );
    const api = run(bridge, fetchStub, hub, refuse, webcrypto, TextDecoder, () => ({ url: "https://builder.example/build/abc/file", name: "notes.krate" }),
      async () => new Error("no"), DecompressionStream, Blob, Response, "https://hub.example");
    return { ...api, sent };
  }
  const t = tab();
  check(t.manifestAppId('[package]\nid = "nope"\n[app]\nname = "N"\nid = "dev.krate.notes.k3x9qa"\n') === "dev.krate.notes.k3x9qa", "the [app] table's id, not another table's");
  check((await t.C.app_versions({ path: "x" })) === null, "not published yet: null, not an error");
  check(t.sent.some((s) => s.hub === "/lines?app=dev.krate.sql-check"), "the hub is asked by the id inside the file");
  const known = tab({ lines: { ...LINE(), history: [{ n: 1, hash: sha, url: "u1", current: true }] } });
  const v = await known.C.app_versions({ path: "x" });
  check(v && v.this_version === 1 && v.next === 2, "this very file is named by its sha256, and the next number is right");
  const out = await tab({ publishAnswer: { url: "https://hub.example/a/v0123456789a", full_url: "https://hub.example/a/" + "f".repeat(64), version: 2, updated: true, version_url: "https://hub.example/a/ffffffffffff" } }).C.publish({ path: "x", name: "Notes" });
  check(out.url === "https://hub.example/a/v0123456789a", "a tab's publish hands back the app's link, not the build's 64-hex address");
  check(out.version === 2 && out.updated === true && out.version_url === "https://hub.example/a/ffffffffffff", "with the version it became");
  const signedOut = tab({ token: null });
  check((await signedOut.C.app_versions({ path: "x" })) === null && signedOut.sent.length === 0, "signed out: nothing is fetched");
  const made = await tab({ current: { id: "v0123456789a", current: 1 } }).C.app_make_current({ line: "v0123456789a", version: 1 });
  check(made.current === 1, "Make current goes to the hub");
  let said = "";
  try { await tab({ current: new Error('{"error":"only the app\'s publisher can choose its current version"}') }).C.app_make_current({ line: "v0123456789a", version: 1 }); } catch (e) { said = e.message; }
  check(said === "only the app's publisher can choose its current version", "and its refusal comes back as a sentence");
  const odd = tab();
  try { await odd.C.app_make_current({ line: "../admin", version: 1 }); } catch (e) { said = e.message; }
  check(said === "That is not one of your apps." && !odd.sent.length, "anything that is not an app's id never reaches the hub");
}

/* ---- the receive page: an older version says a newer one exists ------ */
{
  const OPEN = readFileSync("docs/open/index.html", "utf8");
  const { page } = await import("./org-test-kit.mjs");
  const at = OPEN.indexOf("var hash;");
  const code = OPEN.slice(OPEN.lastIndexOf("(function () {", at), OPEN.indexOf("\n})();", OPEN.indexOf("function privateApp", at)) + "\n})();".length);
  check(/<p class="ta-newer" id="taNewer" hidden><\/p>/.test(OPEN), "the line starts hidden on the page");
  const H = "1".repeat(64);
  const meta = (version) => ({ [`GET /meta/${H}`]: () => [200, { id: H, url: `https://hub.krate.tech/a/${H}`, shot: null, meta: { name: "Notes", size: 24000 }, version }] });
  const old = await page(`https://krate.tech/open/?a=${H}`, code, { routes: meta({ n: 1, current: 3, newer: true, versions: 3, app_url: "https://hub.krate.tech/a/v0123456789a" }) });
  const line = old.el("taNewer");
  check(line.hidden === false && /^This is v1\. There is a newer version, v3\./.test(line.textContent), "an older version's page says which it is and that v3 is out");
  const to = line.all((n) => n.tagName === "A")[0];
  check(to && to.href === "https://hub.krate.tech/a/v0123456789a" && to.textContent === "Open v3", "with the app's own link, which opens v3");
  const cur = await page(`https://krate.tech/open/?a=${H}`, code, { routes: meta({ n: 3, current: 3, newer: false, versions: 3, app_url: "https://hub.krate.tech/a/v0123456789a" }) });
  // The page's own markup starts the line hidden; this DOM makes it on first
  // ask, so "nothing said" is the line never filled in.
  const silent = (p) => p.el("taNewer").textContent === "" && !p.el("taNewer").all((n) => n.tagName === "A").length;
  check(silent(cur), "the current version says nothing");
  const none = await page(`https://krate.tech/open/?a=${H}`, code, { routes: meta(null) });
  check(silent(none) && /Notes/.test(none.el("taName").textContent), "a build that is no version of anything says nothing, and the page still works");
  const odd = await page(`https://krate.tech/open/?a=${H}`, code, { routes: meta({ n: 1, current: 3, newer: true, app_url: "javascript:alert(1)" }) });
  check(silent(odd), "a link that is not the hub's app link is never shown");
}

console.log(bad ? `${bad} check(s) failed` : "ok  one app, many versions, in Studio and on the receive page");
process.exit(bad ? 1 : 0);
