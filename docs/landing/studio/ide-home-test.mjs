/* The IDE door on Studio's Home leads with doing, and never scolds on arrival.
 *
 * The founder opened the IDE tab and met a search box, an arrow, and a red
 * "No project by that name" before he had typed a thing. The door now leads
 * with starters made from Krate's own reference apps (a game, a dashboard, a
 * team board, a habit tracker -- each with its real first frame, light and
 * dark) and the person's recent projects as cards. This pins what a stranger
 * meets:
 *   - starters.json is what scripts/build-ide-starters.py makes from apps/
 *     (never hand-edited), each starter rides on a kind ide_new accepts, and
 *     each has its light and dark picture on disk;
 *   - arriving draws no error and puts no cursor in a field;
 *   - a project card opens the workbench through openProject (its first call
 *     is ide_resolve for that path, then the tree);
 *   - a starter card asks for a name inline, prefilled with a free one; Create
 *     calls ide_new with the starter's kind, writes the app's manifest (the
 *     project's own [app] table, the app's capabilities) and every source file
 *     with ide_write, then opens the project;
 *   - a taken name is said on that card, with the click that opens it.
 *
 * Runs the SHIPPED studio/ui/ide.js against a stubbed desktop shell and a
 * small fake document (every element the script does not care about absorbs
 * whatever is done to it). That each starter really builds on the released
 * engine is proved by `scripts/build-ide-starters.py --prove <krate>`.
 *
 *   node docs/landing/studio/ide-home-test.mjs
 */
import { readFileSync, existsSync, readdirSync } from "node:fs";
import assert from "node:assert/strict";
import vm from "node:vm";

const html = readFileSync("studio/ui/index.html", "utf8");
const ideSrc = readFileSync("studio/ui/ide.js", "utf8");
const redesign = readFileSync("studio/ui/redesign.js", "utf8");
const rust = readFileSync("studio/src/ide.rs", "utf8");
const DATA = JSON.parse(readFileSync("studio/ui/starters/starters.json", "utf8"));
const STARTERS = DATA.starters;

// 1. The starters: made from the reference apps, each on a kind ide_new takes.
const kinds = JSON.parse(rust.match(/const STARTERS: &\[&str\] = &(\[[^\]]*\]);/)[1]);
assert.ok(kinds.length >= 3, "the engine's built-in kinds were read from ide.rs");
assert.equal(DATA.generated_by, "scripts/build-ide-starters.py", "starters.json says which script makes it");
assert.ok(STARTERS.length >= 3, "at least three starters");
const capsOf = (m) => m.slice(m.indexOf("[[capabilities]]"));
for (const s of STARTERS) {
  assert.ok(kinds.includes(s.kind), `${s.id} rides on ${s.kind}, a kind ide_new accepts`);
  assert.ok(s.title && s.blurb && s.name, `${s.id} has words for its card`);
  const app = `apps/${s.app}`;
  const src = readdirSync(`${app}/src`).filter((f) => f.endsWith(".rs") && f !== "bindings.rs").map((f) => `src/${f}`).sort();
  assert.deepEqual(Object.keys(s.files).sort(), src, `${s.id} carries every source file of ${s.app}, and no bindings`);
  for (const rel of src) assert.equal(s.files[rel], readFileSync(`${app}/${rel}`, "utf8"), `${s.id}'s ${rel} is ${s.app}'s, as it is now: rerun scripts/build-ide-starters.py`);
  assert.equal(s.caps, capsOf(readFileSync(`${app}/manifest.toml`, "utf8")), `${s.id} asks for what ${s.app} asks for`);
  for (const look of ["light", "dark"]) assert.ok(existsSync(`studio/ui/starters/${s.id}-${look}.jpg`), `${s.id} has its ${look} picture`);
}
const pane = html.slice(html.indexOf('id="homeIdePane"'), html.indexOf('id="homeAttachRow"'));
assert.doesNotMatch(pane, /<input/, "the door's hero is not a field to type in");
assert.doesNotMatch(ideSrc, /No project by that name/, "the old arrival error is gone");
assert.doesNotMatch(redesign, /homeIdeFind[^;]*focus\(/, "choosing the IDE tab puts the cursor in no field");
console.log(`ok  ${STARTERS.length} starters from the reference apps: ${STARTERS.map((s) => `${s.id} (${s.app} on ${s.kind})`).join(", ")}`);

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

// What ide_new writes for a checklist project on 0.6.1 (the [app] table is
// this project's own: its id, its entry, the name the person typed).
const BASE_MANIFEST = `[app]
id = "dev.krate.my_game.focbns"
name = "My game"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/my_game.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "ui.window:create"
rationale = "Open the checklist window"
required = true

[[capabilities]]
cap = "store.kv"
rationale = "Save your My checklist items"
required = true
`;

function boot({ projects = [], fail = {} } = {}) {
  const calls = [];
  const els = {};
  const byId = (id) => (els[id] ||= fake({ id }));
  // The starter cards, as ide.js paints them from starters.json.
  const cards = STARTERS.map((st) => {
    const forms = [];
    const card = fake({ dataset: { id: st.id, kind: st.kind, name: st.name }, appendChild: (f) => { forms.push(f); return f; } });
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
    if (cmd === "ide_read") return args.rel === "manifest.toml" ? BASE_MANIFEST : "fn main() {}\n";
    return null;
  };
  const ctx = {
    document: doc, window: win, navigator: { platform: "MacIntel", userAgent: "" },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    tauri: { event: { listen: async () => () => {} } }, invoke, state: {}, showView(v) { calls.push(["showView", v]); },
    agentLabel: () => "Claude", aiLogo: () => "", requestAnimationFrame: (f) => f(), setTimeout: (f) => { f(); return 0; }, clearTimeout() {},
    Image: function () {}, CustomEvent: function (type, o) { this.type = type; this.detail = o && o.detail; },
    getComputedStyle: () => ({}), console,
    fetch: async (url) => (url === "starters/starters.json" ? { ok: true, json: async () => JSON.parse(JSON.stringify(DATA)) } : { ok: false, status: 404 }),
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
  const painted = t.byId("homeIdeStarters").innerHTML;
  assert.equal((painted.match(/class="kr-starter"/g) || []).length, STARTERS.length, "one card per starter");
  for (const st of STARTERS) {
    assert.match(painted, new RegExp(`data-id="${st.id}" data-kind="${st.kind}"`), `${st.id} is drawn on its kind`);
    assert.ok(painted.includes(`starters/${st.id}-light.jpg`) && painted.includes(`starters/${st.id}-dark.jpg`), `${st.id} shows its light and dark pictures`);
  }
  console.log("ok  arriving draws the starters, no error, and takes no focus; the empty state is said");
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

// 4. A starter: name inline, prefilled with a free name; Create makes the
//    kind, writes the app over it, and opens it.
{
  const st = STARTERS[0];
  const taken = st.name.toLowerCase().replace(/[^a-z0-9]+/g, "-");
  const t = boot({ projects: [{ name: taken, path: `/u/Krate Apps/Projects/${taken}`, updated: 1 }] });
  await t.arrive(); await settle();
  const card = t.cards[0];
  await fire(t.byId("homeIdeStarters"), "click", { target: { closest: (s) => (s === ".kr-st-go" ? card.go : null) } });
  assert.ok(card.classList.contains("naming"), "the card asks for a name");
  const form = card.forms[0];
  const input = form.querySelector("input");
  assert.equal(input.value, `${st.name} 2`, "prefilled with a name not taken yet");
  t.calls.length = 0;
  await fire(form, "submit"); await settle();
  const order = t.calls.map((c) => c[0]);
  assert.deepEqual(t.calls.find((c) => c[0] === "ide_new"), ["ide_new", { name: `${st.name} 2`, kind: st.kind }], "Create makes the starter's kind under that name");
  const writes = t.calls.filter((c) => c[0] === "ide_write").map((c) => c[1]);
  const man = writes.find((w) => w.rel === "manifest.toml");
  assert.ok(man, "the manifest is written");
  assert.ok(man.text.startsWith(BASE_MANIFEST.slice(0, BASE_MANIFEST.indexOf("[[capabilities]]")).trimEnd()), "it keeps the project's own [app] table");
  assert.ok(man.text.endsWith(st.caps) && !man.text.includes("Save your My checklist items"), "and asks for the app's capabilities, not the checklist's");
  for (const [rel, text] of Object.entries(st.files)) {
    assert.ok(writes.some((w) => w.rel === rel && w.text === text && w.path === "/u/Krate Apps/Projects/my-checklist"), `${rel} is written into the new project`);
  }
  const opened = order.indexOf("ide_resolve");
  assert.ok(opened > order.lastIndexOf("ide_write"), "the project opens after every file is written");
  console.log(`ok  a starter asks for a name, makes ${st.kind}, writes ${st.app}'s ${Object.keys(st.files).length} file(s) and manifest, then opens it`);
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
