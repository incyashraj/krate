// Studio's "Company apps" on Home (IC-953), run from the shipped app.js.
//
// The block is lifted out of studio/ui/app.js and run against a small
// stand-in DOM and a stand-in engine (`invoke`), the way the other page
// tests run the shipped scripts: one card per app the engine lists (its
// name, organisation, version, what it can do, whether it works offline),
// Open asks the engine's door for that app by organisation and app id and
// shows the engine's own words when it does not open, a withdrawn app is
// gone after the next refresh, the list is refreshed every 15 minutes, and
// on a computer whose policy names an organisation with nobody signed in
// the section is the one card that asks for that organisation's sign-in.
//
//   node docs/landing/studio/company-apps-test.mjs
import { readFileSync } from "node:fs";

const app = readFileSync("studio/ui/app.js", "utf8");
const html = readFileSync("studio/ui/index.html", "utf8");
const bridge = readFileSync("docs/landing/studio/bridge.js", "utf8");
const css = readFileSync("studio/ui/redesign.css", "utf8");

let bad = 0;
function check(ok, name) {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
}

const start = app.indexOf("const companyApps = {");
const end = app.indexOf("async function openSendSheet(");
check(start > 0 && end > start, "the company apps block is in app.js");
const block = app.slice(start, end);

/* ---- a stand-in DOM -------------------------------------------------- */
function element(tag) {
  const el = {
    tag, children: [], listeners: {}, hidden: false, disabled: false, textContent: "", className: "",
    attrs: {}, _html: "", said: null,
    set innerHTML(v) { this._html = String(v); if (!v) this.children = []; },
    get innerHTML() { return this._html; },
    appendChild(c) { this.children.push(c); return c; },
    setAttribute(k, v) { this.attrs[k] = String(v); },
    addEventListener(k, fn) { this.listeners[k] = fn; },
    querySelector(sel) { if (sel === ".co-said") return (this.said ||= element("span")); return null; },
    click() { return this.listeners.click && this.listeners.click(); },
  };
  return el;
}
function world(answers, store = new Map()) {
  const els = { companyApps: element("section"), companyAppsList: element("div"), companyAppsNote: element("p") };
  const calls = [];
  const signIns = [];
  const intervals = [];
  let next = 0;
  const invoke = async (cmd, args) => {
    calls.push([cmd, args]);
    const a = answers[cmd];
    const v = typeof a === "function" ? a(args, next++) : a;
    if (v instanceof Error) throw v;
    return v;
  };
  const run = new Function(
    "$", "document", "invoke", "escapeHtml", "clip", "beginBrowserSignIn", "setInterval", "clearInterval", "lsGet", "lsSet",
    `${block}\nreturn { refreshCompanyApps, renderCompanyApps, COMPANY_EVERY };`,
  );
  const api = run(
    (id) => els[id] || null,
    { createElement: (t) => element(t) },
    invoke,
    (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]),
    (s, n) => String(s).slice(0, n),
    (provider) => { signIns.push(provider); return Promise.resolve(); },
    (fn, ms) => { intervals.push([fn, ms]); return 1; },
    () => {},
    (k) => (store.has(k) ? store.get(k) : null),
    (k, v) => store.set(k, v),
  );
  return { els, calls, signIns, intervals, ...api };
}
const settle = () => new Promise((r) => setTimeout(r, 0));
const cards = (w) => w.els.companyAppsList.children;
const open = (card) => card.children.find((c) => c.className === "co-open");

const BOUNCE = (extra = {}) => ({
  app_id: "com.acme.expenses", name: "Expenses", version: "1.3.0", current: "a".repeat(64), digest: "a".repeat(64),
  size: 2_100_000, offline: "days-7", ready_offline_until: 1_790_000_000,
  capabilities: [{ cap: "net.connect:api.acme.com:443", words: "connect to api.acme.com" }, { cap: "fs.read", words: "read files you choose" }],
  ...extra,
});
const LIST = (apps, extra = {}) => ({ v: 1, signed_in: true, policy_org: null, offline: false, orgs: [{ slug: "acme", name: "Acme", role: "recipient", apps }], ...extra });

/* ---- the markup and the stylesheet ----------------------------------- */
check(/<section class="kr-under co-apps" id="companyApps"[^>]*hidden>/.test(html), "the section is on Home, hidden until there is something to show");
check(/id="companyAppsH">Company apps</.test(html), "it is called Company apps");
check(/\.co-open \{[^}]*min-height: 44px/.test(css), "Open is at least 44px tall");
check(/@media \(max-width: 860px\) \{ \.co-list \{ grid-template-columns: minmax\(0, 1fr\); \} \}/.test(css), "one card to a row on a narrow window");
const rules = css.slice(css.indexOf("/* ---------------------------------------------------------------- company apps"));
check(!/#[0-9a-f]{3,8}\b|rgba?\(/i.test(rules), "the section's rules use tokens only");

/* ---- one card per app ------------------------------------------------- */
{
  const w = world({ org_apps: LIST([BOUNCE(), BOUNCE({ app_id: "com.acme.rota", name: "Rota", version: "0.4.0", ready_offline_until: null, offline: "online", capabilities: [] })]) });
  await w.refreshCompanyApps(true);
  check(w.calls[0][0] === "org_apps", "the engine is asked for the list");
  check(w.els.companyApps.hidden === false && cards(w).length === 2, "one card per app");
  const [first, second] = cards(w);
  check(/<b>Expenses<\/b>/.test(first.innerHTML) && /Acme · 1\.3\.0 · works offline/.test(first.innerHTML), "name, organisation, version, and that it works offline");
  check(/connect to api\.acme\.com · read files you choose/.test(first.innerHTML), "what it can do, in one line, in the approver's words");
  check(/Acme · 0\.4\.0</.test(second.innerHTML) && /Asks for nothing on this computer/.test(second.innerHTML), "an app that cannot work offline says nothing about it; one that asks for nothing says so");
  check(open(first).textContent === "Open" && /Open Expenses from Acme/.test(open(first).attrs["aria-label"]), "each has Open, named for a screen reader");

  // Open: the engine's door, by organisation and app id.
  await open(first).click();
  const asked = w.calls.find(([c]) => c === "org_app_open");
  check(asked && asked[1].slug === "acme" && asked[1].appId === "com.acme.expenses", "Open asks the engine to open that app of that organisation");
  check(open(first).disabled === false && open(first).textContent === "Open", "and Open is ready again after");
}

/* ---- refused: the engine's words, and the list is asked again --------- */
{
  let round = 0;
  const w = world({
    org_apps: () => (round++ === 0 ? LIST([BOUNCE()]) : LIST([])),
    org_app_open: new Error("Expenses does not open: Acme withdrew Expenses 1.3.0: \"wrong rates\""),
  });
  await w.refreshCompanyApps(true);
  const card = cards(w)[0];
  await open(card).click();
  check(/Acme withdrew Expenses 1\.3\.0/.test(card.said.textContent), "a refusal shows the engine's words on the card");
  await settle();
  check(w.calls.filter(([c]) => c === "org_apps").length === 2, "and the list is asked again");
  check(w.els.companyApps.hidden === true && cards(w).length === 0, "a withdrawn app is gone from Home");
}

/* ---- refused, still listed: the words stay through the refresh -------- */
{
  const w = world({
    org_apps: LIST([BOUNCE()]),
    org_app_open: new Error("this is Acme's private app, and its offline access ran out on 2026-10-01"),
  });
  await w.refreshCompanyApps(true);
  await open(cards(w)[0]).click();
  await settle();
  check(cards(w).length === 1 && /offline access ran out/.test(cards(w)[0].said.textContent), "a refusal's words stay on the card after the list is drawn again");
}

/* ---- every 15 minutes ------------------------------------------------- */
{
  const w = world({ org_apps: LIST([BOUNCE()]) });
  check(w.COMPANY_EVERY === 15 * 60 * 1000 && w.intervals.some(([, ms]) => ms === 15 * 60 * 1000), "the list is refreshed every 15 minutes while Studio is open");
  await w.refreshCompanyApps(true);
  await w.refreshCompanyApps();
  check(w.calls.filter(([c]) => c === "org_apps").length === 1, "coming back to Home within a minute does not ask again");
  await w.intervals[0][0]();
  await settle();
  check(w.calls.filter(([c]) => c === "org_apps").length === 2, "the clock asks again");
}

/* ---- offline ---------------------------------------------------------- */
{
  const w = world({ org_apps: LIST([BOUNCE()], { offline: true }) });
  await w.refreshCompanyApps(true);
  check(w.els.companyAppsNote.hidden === false && /No connection/.test(w.els.companyAppsNote.textContent), "offline, it says these are the apps as last seen");
}

/* ---- nothing to show -------------------------------------------------- */
for (const answer of [{ orgs: [] }, { v: 1, signed_in: false, policy_org: null, orgs: [] }, null]) {
  const w = world({ org_apps: answer });
  await w.refreshCompanyApps(true);
  check(w.els.companyApps.hidden === true, `no apps, no section (${JSON.stringify(answer)})`);
}

/* ---- a new laptop: the policy's organisation, signed out --------------- */
{
  const w = world({ org_apps: { v: 1, signed_in: false, policy_org: "acme", sign_in: "https://krate.tech/login/?org=acme", orgs: [] } });
  await w.refreshCompanyApps(true);
  check(w.els.companyApps.hidden === false && cards(w).length === 1, "one card asks to sign in");
  const ask = cards(w)[0];
  check(/acme uses Krate/.test(ask.innerHTML) && /Sign in with your work account/.test(ask.innerHTML), "naming the organisation IT set up");
  await open(ask).click();
  check(w.signIns[0] === "org:acme", "Sign in opens that organisation's company sign-in");
  // With the organisation's own name in the policy, that is what it reads.
  const named = world({ org_apps: { v: 1, signed_in: false, policy_org: "acme", policy_name: "Acme Labs", orgs: [] } });
  await named.refreshCompanyApps(true);
  const card = cards(named)[0];
  check(/<b>Acme Labs uses Krate<\/b>/.test(card.innerHTML) && open(card).textContent === "Sign in to Acme Labs", "it says the organisation's name, and the button says where it signs in");
}

/* ---- fast: the last list shows at once, the fresh one replaces it ------ */
{
  const store = new Map();
  const first = world({ org_apps: LIST([BOUNCE()]) }, store);
  await first.refreshCompanyApps(true);
  let answer;
  const gate = new Promise((r) => { answer = r; });
  const next = world({ org_apps: () => gate }, store);
  const pending = next.refreshCompanyApps(true);
  check(cards(next).length === 1 && /Expenses/.test(cards(next)[0].innerHTML), "on launch, the apps last seen are on Home before the engine answers");
  answer(LIST([]));
  await pending;
  check(next.els.companyApps.hidden === true, "and the fresh answer replaces them");
  const out = world({ org_apps: { v: 1, signed_in: false, policy_org: null, orgs: [] } }, store);
  await out.refreshCompanyApps(true);
  const after = world({ org_apps: () => new Promise(() => {}) }, store);
  after.refreshCompanyApps(true);
  check(cards(after).length === 0, "signed out, nothing is kept to show the next person");
}

/* ---- in a tab --------------------------------------------------------- */
check(/async org_apps\(\)/.test(bridge) && /async org_app_open\(/.test(bridge), "the web bridge answers both commands");
{
  // The bridge's own org_app_open, run: a company app opens in a new tab at
  // its own address on the hub (K-1166), never as a file to download.
  const at = bridge.indexOf("  async org_app_open(");
  const body = bridge.slice(at, bridge.indexOf("\n  },", at) + "\n  }".length).replace(/^ {2}async /, "async function ");
  const opened = [];
  const run = (token) => new Function("bridge", "refuse", "HUB", "window", `${body}; return org_app_open;`)(
    { token }, (m) => { throw new Error(m); }, "https://hub.krate.tech", { open: (...a) => opened.push(a) },
  );
  await run("gho_dan")({ slug: "acme", appId: "com.acme.expenses" });
  check(opened.length === 1 && opened[0][0] === "https://hub.krate.tech/run/org/acme/com.acme.expenses" && opened[0][1] === "_blank",
    "in a tab, Open opens the app at its own address in a new tab", opened);
  const refused = await run("gho_dan")({ slug: "acme", appId: "../../evil" }).then(() => null, (e) => e.message);
  check(opened.length === 1 && /not one Krate opens/.test(refused || ""), "an address that is not an app's is refused, nothing opened", refused);
  const out = await run(null)({ slug: "acme", appId: "com.acme.expenses" }).then(() => null, (e) => e.message);
  check(opened.length === 1 && /Sign in first/.test(out || ""), "signed out, it asks for sign-in first", out);
}

console.log(bad ? `${bad} check(s) failed` : "ok  company apps on Studio's Home");
process.exit(bad ? 1 : 0);
