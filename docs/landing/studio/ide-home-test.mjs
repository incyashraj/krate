/* The IDE door on Studio's Home leads with doing, and never scolds on arrival.
 *
 * The founder opened the IDE tab and met a search box, an arrow, and a red
 * "No project by that name" before he had typed a thing. The door now leads
 * with the engine's own starters (each showing a real first frame) and the
 * person's recent projects as cards. This pins what a stranger meets:
 *   - every starter ide_new makes has a card, and only those, each with its
 *     real picture on disk;
 *   - arriving draws no error and puts no cursor in a field;
 *   - a project card opens the workbench through openProject (its first call
 *     is ide_resolve for that path, then the tree);
 *   - a starter card asks for a name inline, prefilled with a free one, and
 *     Create calls ide_new with that starter's kind, then opens the project;
 *   - a taken name is said on that card, with the click that opens it.
 *
 * Runs the SHIPPED studio/ui/ide.js against a stubbed desktop shell and a
 * small fake document (every element the script does not care about absorbs
 * whatever is done to it).
 *
 *   node docs/landing/studio/ide-home-test.mjs
 */
import { readFileSync, existsSync } from "node:fs";
import assert from "node:assert/strict";
import vm from "node:vm";

const html = readFileSync("studio/ui/index.html", "utf8");
const ideSrc = readFileSync("studio/ui/ide.js", "utf8");
const redesign = readFileSync("studio/ui/redesign.js", "utf8");
const rust = readFileSync("studio/src/ide.rs", "utf8");

// 1. The starters on Home are exactly the ones ide_new accepts.
const starters = JSON.parse(rust.match(/const STARTERS: &\[&str\] = &(\[[^\]]*\]);/)[1]);
assert.ok(starters.length >= 3, "the engine's starters were read from ide.rs");
const pane = html.slice(html.indexOf('id="homeIdePane"'), html.indexOf('id="homeAttachRow"'));
const kinds = [...pane.matchAll(/class="kr-starter" data-kind="([^"]+)"/g)].map((m) => m[1]);
assert.deepEqual(kinds, starters, "one starter card per engine starter, in the engine's order");
for (const src of pane.matchAll(/<img src="(starters\/[^"]+)"/g)) {
  assert.ok(existsSync(`studio/ui/${src[1]}`), `the starter picture ${src[1]} is in the interface`);
}
assert.doesNotMatch(pane, /<input/, "the door's hero is not a field to type in");
assert.doesNotMatch(ideSrc, /No project by that name/, "the old arrival error is gone");
assert.doesNotMatch(redesign, /homeIdeFind[^;]*focus\(/, "choosing the IDE tab puts the cursor in no field");
console.log(`ok  ${kinds.length} starter cards, the engine's own: ${kinds.join(", ")}`);

// ---- a fake document, just enough for ide.js -------------------------------
const listeners = new Map(); // element -> { type: [fn] }
function on(el, type, fn) { if (!listeners.has(el)) listeners.set(el, {}); (listeners.get(el)[type] ||= []).push(fn); }
async function fire(el, type, ev = {}) { for (const fn of (listeners.get(el) || {})[type] || []) await fn({ preventDefault() {}, stopPropagation() {}, ...ev }); }
function fake(props = {}) {
  const kids = new Map();
  const classes = new Set();
  const base = {
    hidden: false, textContent: "", innerHTML: "", value: "", title: "", disabled: false, dataset: {}, style: { setProperty() {} },
    classList: { add: (...c) => c.forEach((x) => classes.add(x)), remove: (...c) => c.forEach((x) => classes.delete(x)), toggle: (c, f) => ((f ?? !classes.has(c)) ? classes.add(c) : classes.delete(c)), contains: (c) => classes.has(c) },
    addEventListener(type, fn) { on(proxy, type, fn); },
    removeEventListener() {},
    querySelector(sel) { if (!kids.has(sel)) kids.set(sel, fake()); return kids.get(sel); },
    querySelectorAll() { return []; },
    closest() { return null; },
    appendChild(c) { return c; }, append() {}, prepend() {}, remove() { base.removed = true; }, insertAdjacentHTML() {}, insertBefore(c) { return c; },
    setAttribute() {}, getAttribute() { return null; }, removeAttribute() {}, toggleAttribute() {}, contains() { return false; }, dispatchEvent() { return true; },
    focus() { focused.push(proxy); }, blur() {}, select() {}, click() {}, scrollIntoView() {}, getBoundingClientRect() { return { top: 0, left: 0, width: 0, height: 0, right: 0, bottom: 0 }; },
    ...props,
  };
  const proxy = new Proxy(base, { get: (t, k) => (k in t || typeof k === "symbol" ? t[k] : (t[k] = fake())) });
  return proxy;
}
const focused = [];

function boot({ projects = [], fail = {} } = {}) {
  const calls = [];
  const els = {};
  const byId = (id) => (els[id] ||= fake({ id }));
  // The starter cards, as the page has them.
  const cards = kinds.map((kind) => {
    const name = pane.match(new RegExp(`data-kind="${kind}" data-name="([^"]+)"`))[1];
    const forms = [];
    const card = fake({ dataset: { kind, name }, appendChild: (f) => { forms.push(f); return f; } });
    card.forms = forms;
    card.go = fake({ closest: (sel) => (sel === ".kr-starter" ? card : null) });
    return card;
  });
  byId("homeIdeStarters").querySelectorAll = (sel) => (sel === ".kr-starter.naming" ? cards.filter((c) => c.classList.contains("naming")) : []);
  const doc = fake({
    getElementById: byId,
    createElement: () => { const f = fake(); const parts = {}; f.querySelector = (s) => (parts[s] ||= fake()); return f; },
    addEventListener(type, fn) { on(doc, type, fn); },
    dispatchEvent(ev) { return fire(doc, ev.type, ev); },
  });
  const win = fake({ addEventListener() {} });
  const invoke = async (cmd, args) => {
    // Copied into this realm: the script runs in its own, and deepEqual
    // compares prototypes.
    calls.push([cmd, args === undefined ? undefined : JSON.parse(JSON.stringify(args))]);
    if (fail[cmd]) throw fail[cmd];
    if (cmd === "ide_projects") return projects;
    if (cmd === "ide_resolve") return { path: args.path, name: args.path.split("/").pop() };
    if (cmd === "ide_new") return { name: "my-checklist", path: "/u/Krate Apps/Projects/my-checklist" };
    if (cmd === "ide_tree") return [{ rel: "src/lib.rs", dir: false, size: 10 }];
    if (cmd === "ide_read") return "fn main() {}\n";
    return null;
  };
  const ctx = {
    document: doc, window: win, navigator: { platform: "MacIntel", userAgent: "" },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    tauri: { event: { listen: async () => () => {} } }, invoke, state: {}, showView(v) { calls.push(["showView", v]); },
    agentLabel: () => "Claude", aiLogo: () => "", requestAnimationFrame: (f) => f(), setTimeout: (f) => { f(); return 0; }, clearTimeout() {},
    Image: function () {}, CustomEvent: function (type, o) { this.type = type; this.detail = o && o.detail; },
    getComputedStyle: () => ({}), console,
  };
  vm.createContext(ctx);
  vm.runInContext(ideSrc, ctx);
  return { calls, byId, cards, doc, arrive: () => fire(doc, "kr-home-mode", { detail: "ide" }) };
}
const tick = () => new Promise((r) => setImmediate(r));
const settle = async () => { for (let i = 0; i < 20; i++) await tick(); };

// 2. Arriving: no error line, no cursor in a field, the empty state says what goes there.
{
  const t = boot();
  focused.length = 0;
  await t.arrive(); await settle();
  assert.equal(t.byId("homeIdeHint").hidden, true, "no error line on arrival");
  assert.equal(t.byId("homeIdeHint").textContent, "", "no error text on arrival");
  assert.equal(focused.length, 0, "arriving focuses nothing");
  assert.match(t.byId("homeIdeRows").innerHTML, /Projects you start or open show up here/, "the empty state says what will appear");
  assert.equal(t.byId("homeIdeFilter").hidden, true, "no filter with no projects");
  console.log("ok  arriving draws no error and takes no focus; the empty state is said");
}

// 3. A project card opens the workbench through openProject.
{
  const projects = Array.from({ length: 12 }, (_, i) => ({ name: `p${i}`, path: `/u/Krate Apps/Projects/p${i}`, updated: 1 }));
  const t = boot({ projects });
  await t.arrive(); await settle();
  const rows = t.byId("homeIdeRows").innerHTML;
  assert.equal((rows.match(/class="kr-pj"/g) || []).length, 8, "eight at a glance");
  assert.match(rows, /Show all 12/, "the rest one click away");
  assert.equal(t.byId("homeIdeFilter").hidden, false, "a filter once there are more than eight");
  assert.equal(t.byId("homeIdeHint").hidden, true, "still no error");
  const card = fake({ dataset: { path: "/u/Krate Apps/Projects/p3", name: "p3" } });
  t.calls.length = 0;
  await fire(t.byId("homeIdeRows"), "click", { target: { closest: (s) => (s === ".kr-pj" ? card : null) } });
  await settle();
  const names = t.calls.map((c) => c[0]);
  assert.deepEqual(t.calls[0], ["ide_resolve", { path: "/u/Krate Apps/Projects/p3" }], "openProject resolves the card's path first");
  assert.ok(names.includes("ide_tree"), "and reads its files: the workbench is opening");
  assert.ok(t.calls.some((c) => c[0] === "showView" && c[1] === "ide"), "and shows the workbench");
  console.log("ok  a project card opens the workbench (ide_resolve, ide_tree, the IDE view)");
}

// 4. A starter: name inline, prefilled with a free name; Create makes it and opens it.
{
  const t = boot({ projects: [{ name: "my-checklist", path: "/u/Krate Apps/Projects/my-checklist", updated: 1 }] });
  await t.arrive(); await settle();
  const card = t.cards[kinds.indexOf("checklist")];
  await fire(t.byId("homeIdeStarters"), "click", { target: { closest: (s) => (s === ".kr-st-go" ? card.go : null) } });
  assert.ok(card.classList.contains("naming"), "the card asks for a name");
  const form = card.forms[0];
  const input = form.querySelector("input");
  assert.equal(input.value, "My checklist 2", "prefilled with a name not taken yet");
  t.calls.length = 0;
  await fire(form, "submit"); await settle();
  const made = t.calls.find((c) => c[0] === "ide_new");
  assert.deepEqual(made, ["ide_new", { name: "My checklist 2", kind: "checklist" }], "Create makes that starter under that name");
  assert.ok(t.calls.some((c) => c[0] === "ide_resolve" && c[1].path.endsWith("/my-checklist")), "then opens the new project");
  console.log("ok  a starter asks for a name inline, makes it with ide_new and opens it");
}

// 5. A taken name is said on the card, with the click that opens that project.
{
  const projects = [{ name: "h", path: "/u/Krate Apps/Projects/h", updated: 1 }];
  const t = boot({ projects, fail: { ide_new: "A project named h already exists." } });
  await t.arrive(); await settle();
  const card = t.cards[0];
  await fire(t.byId("homeIdeStarters"), "click", { target: { closest: (s) => (s === ".kr-st-go" ? card.go : null) } });
  const form = card.forms[0];
  form.querySelector("input").value = "h";
  let offered = null;
  form.querySelector(".kr-st-line").appendChild = (b) => { offered = b; return b; };
  await fire(form, "submit"); await settle();
  assert.match(form.querySelector(".kr-st-line").textContent, /already exists/, "the refusal is said on the card");
  assert.equal(t.byId("homeIdeHint").hidden, true, "and not as a line somewhere else");
  assert.ok(offered && offered.textContent === "Open h", "with the click that opens it");
  console.log("ok  a taken name is said on the card and offers Open h");
}
