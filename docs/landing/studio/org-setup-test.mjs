// Setting an organisation up from krate.tech (IC-958 B and C, and the
// one-minute addendum): the name, a first app released, the team invited,
// Getting started ticking itself, the IT pack, joining from the email,
// and the signing ring's places on the dashboard -- each against the
// shapes the contract states, and hidden where the hub has not got them.
//
//   node docs/landing/studio/org-setup-test.mjs     (from the repo root)
import { readFileSync } from "node:fs";
import { ORGS, DASH, JOIN, check, page, settle, P, ACME, member, orgRoutes, button, failures } from "./org-test-kit.mjs";

const STARTERS = JSON.parse(readFileSync("docs/landing/orgs/starters.json", "utf8"));
const TABLE = JSON.parse(readFileSync("docs/landing/deploy/policy-keys.json", "utf8"));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const DIG = "b".repeat(64);
const textOf = (n) => P(n.shown);
const NEW = { slug: "acme", name: "Acme", created_at: 1760000000, created_by: "github:alice", root_key: null };
const ME = member("github:alice", "owner");

/// The routes a brand-new organisation answers, before and after its first app.
function freshRoutes(extra = {}) {
  return {
    "GET /orgs": () => [200, []],
    "GET /orgs/starters.json": () => [200, STARTERS],
    "GET /orgs/starters/team-notes.krate": () => [200, { bytes: [80, 75, 3, 4, 9, 9] }],
    "GET /deploy/policy-keys.json": () => [200, TABLE],
    "GET /orgs/acme/available": () => [200, { slug: "acme", available: true }],
    "POST /orgs": (_h, b) => [201, { org: { ...NEW, slug: b.slug, name: b.name }, membership: ME }],
    "POST /orgs/acme/candidates": () => [201, { digest: DIG, state: "waiting-for-approval", name: "Team Notes", version: "1.0.0" }],
    [`POST /orgs/acme/candidates/${DIG}/approve`]: () => [200, { release: { digest: DIG, url: `https://hub.krate.tech/a/${DIG}`, solo: true } }],
    "POST /orgs/acme/invites": (_h, b) => [201, { code: "c-" + b.email, expires_at: 1760604800, join_url: "https://krate.tech/join/?o=acme&c=x", kind: "email", display: b.email, role: b.role, emailed: b.email !== "lee@acme.test" }],
    ...extra,
  };
}

/// A stand-in for the browser device (kit/org-device.js, not this page's).
function device(log, opts = {}) {
  return {
    ready: async (slug, o) => { log.push(["ready", slug, o && o.name]); if (opts.readyFails) { const e = new Error("x"); e.reason = opts.readyFails; throw e; } return { root_key: "ab".repeat(32) }; },
    sign: async (slug, bytes) => { log.push(["sign", slug, bytes.length]); return new Uint8Array([...bytes, 1]); },
  };
}

async function create(p, name) {
  p.el("cName").value = name;
  p.el("cName").dispatch("input");
  await wait(420);
  await settle();
}

/* ---------------------------------------------- B1: the name, checked live */
{
  const taken = new Set(["acme", "acme-team"]);
  const p = await page("https://krate.tech/orgs/", ORGS, {
    token: "t",
    routes: freshRoutes({ ...Object.fromEntries(["acme", "acme-team", "acme-hq"].map((s) => [`GET /orgs/${s}/available`, () => [200, { slug: s, available: !taken.has(s) }]])) }),
  });
  check(p.el("create").hidden === false && p.el("cSlugBox").hidden === true, "the create form asks for the name only; the address waits behind Change");
  await create(p, "Acme");
  check(p.el("cSlug").value === "acme-hq" && /acme is taken, so Its address: krate\.tech\/org\/acme-hq/.test(p.el("cAddrText").textContent), "a taken address is replaced by a free one at once, and said");
  check(p.el("cGo").textContent === "Create Acme", "the button says what it makes");
  check(p.sent.filter((s) => s.key.endsWith("/available")).every((s) => !s.headers["idempotency-key"]), "checking a name is a plain read");
}

/* ------------------------------------- B (addendum): first app, released */
{
  const log = [];
  const p = await page("https://krate.tech/orgs/", ORGS, { token: "t", routes: freshRoutes(), extra: { KrateOrgDevice: device(log) } });
  await create(p, "Acme");
  p.el("createForm").dispatch("submit");
  await settle();
  const body = p.el("wBody");
  check(/Give Acme its first app/.test(textOf(body)) && STARTERS.starters.every((s) => textOf(body).includes(s.name)) && !/Placeholder/.test(textOf(body)), "after Create comes the first app: every real ready app, none labelled a placeholder");
  const radios = body.all((n) => n.attrs.role === "radio");
  check(radios.length === STARTERS.starters.length && radios[0].attrs["aria-checked"] === "true", "the first ready app is already picked");
  // Fifteen ready apps (gap G7): every one real, with its picture and a
  // kind, and a row of kinds and a search above them.
  check(STARTERS.starters.length === 15 && STARTERS.starters.every((s) => !s.placeholder && s.shot && s.bundle && s.top && s.top.length === 3 && ["Everyday", "Operations", "People", "Finance"].includes(s.category)), "fifteen ready apps, each with a picture, three best things and one of the four kinds");
  const chips = body.all((n) => n.tagName === "BUTTON" && n.classList.contains("chip"));
  check(chips.map((c) => P(c.shown)).join("|") === "All 15|Everyday 4|Operations 5|People 4|Finance 2" && chips[0].attrs["aria-pressed"] === "true", "the kinds are chips with how many in each, All pressed");
  const shownNames = () => radios.filter((r) => !r.hidden).map((r) => P(r.shown).split(" ").slice(0, 2).join(" "));
  chips.find((c) => /^Finance/.test(P(c.shown))).click();
  check(shownNames().length === 2 && /Expenses/.test(shownNames().join()) && /Purchase Orders/.test(shownNames().join()), "Finance shows the two money apps and hides the rest");
  const shownRadios = radios.filter((r) => !r.hidden);
  shownRadios[1].dispatch("keydown", { key: "ArrowRight" });
  check(shownRadios[0].attrs["aria-checked"] === "true", "the arrow keys move only among the apps on show");
  radios[0].click();
  chips[0].click();
  const find = body.all((n) => n.tagName === "INPUT" && n.attrs.type === "search")[0];
  find.value = "swap";
  find.dispatch("input");
  check(shownNames().length === 1 && /Shift Rota/.test(shownNames()[0]), "a search finds an app by what it does, not only its name");
  find.value = "zebra";
  find.dispatch("input");
  check(radios.every((r) => r.hidden) && /No ready app matches “zebra”/.test(textOf(body)), "nothing matching says so, and what to do");
  find.value = "";
  find.dispatch("input");
  check(radios.every((r) => !r.hidden) && radios[0].attrs["aria-checked"] === "true", "clearing the search shows them all again, the pick unchanged");
  const go = button(body, "Release Team Notes to Acme");
  check(!!go && /not even Krate can read/.test(textOf(body)), "one button says exactly what happens, and the line that says why Krate is beside it");
  go.click();
  await settle();
  check(log.map((l) => l[0]).join() === "ready,sign" && log[0][1] === "acme" && log[1][2] === 6, "this browser sets up Acme's signature and signs the app's bytes");
  const made = p.sent.find((s) => s.key === "POST /orgs");
  check(made && made.body.time_zone === Intl.DateTimeFormat().resolvedOptions().timeZone, "the organisation is made in this browser's time zone, so its days start at the owner's midnight (F-319)");
  const up = p.sent.find((s) => s.key === "POST /orgs/acme/candidates");
  check(up && up.headers["content-type"] === "application/octet-stream" && up.headers["idempotency-key"] && up.body.length === 7, "the signed bytes go to Acme, once, with an Idempotency-Key");
  const ap = p.sent.find((s) => s.key === `POST /orgs/acme/candidates/${DIG}/approve`);
  check(!!ap, "then it is released (solo release while nobody else can approve)");
  check(/Acme can use Team Notes now/.test(textOf(body)) && body.all((n) => n.tagName === "A" && n.className === "lchip")[0].attrs["data-url"] === `https://hub.krate.tech/a/${DIG}` && !textOf(body).includes(DIG), "Acme can use it now, with the link to copy, its address shortened on the page");
  check(/nobody else in Acme can approve yet/.test(textOf(body)), "a solo release says so");
  check(!!body.all((n) => n.tagName === "TEXTAREA")[0] && /Invite your team/.test(textOf(body)), "and Invite your team is right there");
  check(p.location.href.endsWith("/orgs/?o=acme&step=done") && p.el("wSteps").children[2].className === "on", "the address and the steps say where it is");
  const shown = textOf(p.el("wiz"));
  check(!/digest|namespace|delegation|pkcs|root key|slug|capabilit/i.test(shown), "no machinery words on the way (digest, root key, slug ...)");
}

/* ------------------------- without signing in this browser: Krate Studio */
{
  const p = await page("https://krate.tech/orgs/", ORGS, { token: "t", routes: freshRoutes() });
  await create(p, "Acme");
  p.el("createForm").dispatch("submit");
  await settle();
  button(p.el("wBody"), "Release Team Notes to Acme").click();
  await settle();
  const t = textOf(p.el("wBody"));
  const links = p.el("wBody").all((n) => n.tagName === "A").map((a) => a.href);
  check(/not switched on yet/.test(t) && links.includes("krate://org/setup?org=acme") && links.includes("/download/"), "no browser signing yet: Open Krate Studio, or get it");
  check(!p.sent.some((s) => s.key === "POST /orgs/acme/candidates"), "and nothing unsigned is sent");
}

/* --------------------- the two-person rule holds: it waits, and says who */
{
  const p = await page("https://krate.tech/orgs/", ORGS, {
    token: "t", extra: { KrateOrgDevice: device([]) },
    routes: freshRoutes({ [`POST /orgs/acme/candidates/${DIG}/approve`]: () => [403, { error: "self-approval", message: "" }] }),
  });
  await create(p, "Acme");
  p.el("createForm").dispatch("submit");
  await settle();
  button(p.el("wBody"), "Release Team Notes to Acme").click();
  await settle();
  check(p.location.href === "/org/?o=acme&tab=waiting&sent=Team%20Notes", "held for a second person: on to Waiting, which says so");
}

/* ------------------------------ an upload refused: words and the fix */
{
  const p = await page("https://krate.tech/orgs/?o=acme&step=app", ORGS, {
    token: "t", extra: { KrateOrgDevice: device([]) },
    routes: freshRoutes({
      "GET /orgs/acme": () => [200, { org: NEW, me: ME, members: [ME] }],
      "GET /orgs/acme/releases": () => [200, []],
      "POST /orgs/acme/candidates": () => [422, { error: "not-signed-for-this-org", message: "the release key ... delegation ..." }],
    }),
  });
  check(p.el("wiz").hidden === false && /Give Acme its first app/.test(textOf(p.el("wBody"))), "a reload in the middle comes back to the first app");
  button(p.el("wBody"), "Release Team Notes to Acme").click();
  await settle();
  const t = textOf(p.el("wBody"));
  check(/not signed by Acme yet/.test(t) && !/delegat|release key|root/i.test(t) && button(p.el("wBody"), "Release Team Notes to Acme"), "a refusal is said plainly, and the button is back to try again");
}

/* ---------------------------------------------- B3: the team, by email */
{
  const p = await page("https://krate.tech/orgs/?o=acme&step=done", ORGS, {
    token: "t",
    routes: freshRoutes({
      "GET /orgs/acme": () => [200, { org: { ...NEW, root_key: "ab".repeat(32) }, me: ME, members: [ME] }],
      "GET /orgs/acme/releases": () => [200, [{ name: "Team Notes", version: "1.0.0", digest: DIG, url: `https://hub.krate.tech/a/${DIG}`, uploaded_by: "github:alice", approved_by: "github:alice", at: 1760000300, solo: true }]],
    }),
  });
  const body = p.el("wBody");
  check(/Acme can use Team Notes 1\.0\.0 now/.test(textOf(body)), "a reload after the release shows it again");
  const ta = body.all((n) => n.tagName === "TEXTAREA")[0];
  const sel = body.all((n) => n.tagName === "SELECT")[0];
  check(sel.value === "recipient" && sel.all((o) => o.tagName === "OPTION").map((o) => o.textContent).join() === "Can use apps,Can build apps,Can approve apps", "the role is in plain words, and starts at can use apps");
  ta.value = "Sam Lee <Sam@Acme.test>, lee@acme.test\nbob\nsam@acme.test";
  ta.dispatch("input");
  const send = button(body, "Send 2 invitations");
  check(!!send, "pasted addresses are counted once each, names around them ignored");
  send.click();
  await settle();
  const inv = p.sent.filter((s) => s.key === "POST /orgs/acme/invites");
  check(inv.length === 2 && inv[0].body.email === "sam@acme.test" && inv[1].body.role === "recipient" && inv[0].headers["idempotency-key"] !== inv[1].headers["idempotency-key"], "each address is its own invitation, with its own key");
  check(/1 invitation was emailed\. 1 link could not be emailed: copy it/.test(textOf(body)) && /Emailed/.test(textOf(body)) && button(body, "Copy link"), "it says what went, and offers the link where the mail did not go");
  check(ta.value === "bob" && /still in the box/.test(textOf(body)), "what was not an address stays in the box, to fix");
  check(JSON.parse(p.localStorage.getItem("krate-setup:acme")).invited === 2, "and Getting started knows two were invited");
}

/* ------------------------------------------- Getting started, on /org/ */
const dashRoutes = (org, extra = {}) => ({
  "GET /orgs/acme": () => [200, { org, me: ME, members: extra.members || [ME] }],
  "GET /orgs/acme/apps": () => [404, { error: "no-such-route" }],
  "GET /orgs/acme/releases": () => [200, extra.releases || []],
  "GET /orgs/acme/candidates": () => [200, []],
  "GET /orgs/acme/sso": () => [200, extra.sso || null],
  "GET /orgs/acme/saml": () => [200, null],
  "GET /me": () => [200, { user: { login: "alice", email: "alice@acme.test" } }],
  "GET /orgs/starters.json": () => [200, STARTERS],
  "GET /deploy/policy-keys.json": () => [200, TABLE],
  ...extra.routes,
});
{
  const p = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: dashRoutes(NEW) });
  const start = p.el("start");
  check(start.hidden === false && /Getting started/.test(textOf(start)) && /0 of 6 done/.test(textOf(start)), "a new organisation's dashboard opens on Getting started");
  const rows = start.all((n) => n.classList.contains("cl-row"));
  check(rows[0].attrs["aria-expanded"] === "true" && /Release Team Notes to Acme/.test(textOf(start)), "the first thing not done is open: the first app");
  check(/Company sign-in for acme\.test/.test(textOf(start)), "company sign-in names the owner's own domain");
  rows[3].click();
  check(/Ready once Acme has released its first app/.test(textOf(start)), "the IT pack says in one line why it waits");
  check(!/krate (org|publish|sign|delegate)/.test(textOf(p.el("pane")) + textOf(start)), "no command in the default path");
  button(start, "Hide").click();
  check(/Show Getting started/.test(textOf(start)) && JSON.parse(p.localStorage.getItem("krate-setup:acme")).hidden === true, "Getting started hides, and remembers");
  button(start, "Show Getting started").click();
  await settle();
  check(/Getting started/.test(textOf(start)), "and comes back with one click");
}
{
  const org = { ...NEW, root_key: "ab".repeat(32) };
  const rel = [{ name: "Team Notes", version: "1.0.0", digest: DIG, url: "u", uploaded_by: "github:alice", approved_by: "github:alice", at: 1760000300, solo: true }];
  const p = await page("https://krate.tech/org/?o=acme", DASH, {
    token: "t",
    routes: dashRoutes(org, {
      releases: rel, members: [ME, member("email:sam@acme.test", "recipient")],
      sso: { issuer: "https://accounts.google.com", domain: "acme.test" },
      routes: { "GET /orgs/acme/keys": () => [200, { devices: [{ device_id: "1".repeat(16), name: "Chrome (browser)", kind: "browser" }, { device_id: "2".repeat(16), name: "Alice's Mac" }] }] },
    }),
  });
  const start = p.el("start");
  const ticks = start.all((n) => n.classList.contains("cl-tick")).map((n) => n.classList.contains("on"));
  check(ticks.slice(0, 3).every(Boolean) && ticks[4] === true && /4 of 6 done/.test(textOf(start)), "it ticks itself from what the hub says: app out, someone joined, company sign-in, a computer");
  const rp = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t",
    routes: dashRoutes(org, { releases: rel, members: [ME, member("email:sam@acme.test", "recipient")] }),
  });
  check(/Released by its sender, @alice \(no second approver yet\)/.test(textOf(rp.el("pane"))), "a solo release is marked as such in Releases");
  // The IT pack, made here. On the Dashboard the next step is marked, one
  // line like the rest, and opens on a click.
  const rows = start.all((n) => n.classList.contains("cl-row"));
  check(rows[3].attrs["aria-expanded"] === "false" && rows[3].parent.className === "next", "the next item is the first not done, the IT pack, marked Next");
  rows[3].click();
  await settle();
  check(rows[3].attrs["aria-expanded"] === "true", "and it opens when clicked");
  button(start, "Download for Mac").click();
  await settle();
  const blob = p.win.saved[0];
  const xml = blob ? Buffer.from(blob.parts[0]).toString("utf8") : "";
  check(xml.includes("<string>acme</string>") && xml.includes("ab".repeat(32)) && xml.includes("<string>https://hub.krate.tech</string>"), "Download for Mac hands over Acme's profile: its short name, its root, its hub");
  check(/Saved krate-policy-acme\.mobileconfig/.test(textOf(start)), "and says what it saved");
}

/* ---------------------------- Settings: the click path, commands folded */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: { ...orgRoutes("github:alice"), "GET /orgs/acme/sso": () => [200, null], "GET /orgs/acme/exports": () => [200, []] } });
  const pane = p.el("pane");
  const cmds = pane.all((n) => n.tagName === "PRE" && /^krate /.test(n.textContent));
  const folded = (n) => { let x = n.parent; while (x) { if (x.tagName === "DETAILS") return true; x = x.parent; } return false; };
  check(cmds.length >= 4 && cmds.every(folded), "every command in Settings is under For developers who use the terminal");
  check(/Acme's signature/.test(textOf(pane)) && button(pane, "Release an app"), "the signature card and Send an app lead with the click path");
}

/* ------------------------------------- C: joining from the email */
{
  const base = "https://krate.tech/join/?o=acme&c=abc123def";
  const co = await page(base, JOIN, { routes: { "GET /orgs/acme/invites/abc123def/door": () => [200, { name: "Acme Labs", company_sign_in: true, kind: "email" }] } });
  check(co.el("jH").textContent === "You have an invitation to Acme Labs", "an open invitation names the organisation as the email did");
  const links = co.el("jBody").all((n) => n.tagName === "A").map((a) => a.href);
  const order = co.el("jBody").all((n) => n.tagName === "FORM" || n.tagName === "A").map((n) => n.tagName === "FORM" ? "email" : n.href);
  check(order[0] === "/login/?org=acme&next=%2Fjoin%2F%3Fo%3Dacme%26c%3Dabc123def" && order[1] === "email", "an organisation with company sign-in offers it first, then email for an email invitation");
  check(!co.sent[0].headers.authorization, "asking which way in carries no session");
  const sum = co.el("jBody").all((n) => n.tagName === "SUMMARY")[0];
  const trouble = sum && sum.parent;
  check(trouble && trouble.tagName === "DETAILS" && /krate org join acme abc123def/.test(trouble.textContent) && !/abc123def/.test(co.el("jP").textContent + co.el("jH").textContent), "the code is only under Having trouble?");
  const gh = await page(base, JOIN, { routes: { "GET /orgs/acme/invites/abc123def/door": () => [200, { company_sign_in: false, kind: "github" }] } });
  check(gh.el("jBody").all((n) => n.tagName === "A")[0].href.endsWith("#github"), "a GitHub invitation offers GitHub first");
}

/* ------------------- A: publishers, against the shapes the contract states */
{
  // A hub without the signing ring: nothing of it is drawn.
  const none = await page("https://krate.tech/org/?o=acme&tab=people", DASH, { token: "t", routes: orgRoutes("github:alice") });
  check(!none.el("pane").querySelector("#pubCard") && none.el("pubNote").hidden === true && !/waiting to publish/.test(none.el("tabs").shown), "a hub that answers 404 shows no publisher card, notice or badge");

  const pubs = { by_hand: true, waiting: [{ principal: "github:carol", asked_at: 1760000500 }], approvers: ["github:alice", "github:xadmin"] };
  const own = await page("https://krate.tech/org/?o=acme&tab=people", DASH, {
    token: "t",
    routes: { ...orgRoutes("github:alice"), "GET /orgs/acme/publishers": () => [200, pubs], "POST /orgs/acme/publishers/github%3Acarol/approve": () => [200, { state: "approved" }] },
  });
  await settle();
  check(own.el("tab-people").shown.includes("1"), "owners see a count of who waits to publish, on People");
  const ap = button(own.el("pane"), "Approve @carol");
  check(!!ap, "approving by hand is one click");
  ap.click();
  await settle();
  check(own.sent.some((s) => s.key === "POST /orgs/acme/publishers/github%3Acarol/approve" && s.headers["idempotency-key"]) && /signs it the next time it runs Krate/.test(own.el("toast").shown), "and says the owner's computer signs it next time");

  const dev = await page("https://krate.tech/org/?o=acme", DASH, {
    token: "t",
    routes: { ...orgRoutes("github:carol"), "GET /orgs/acme/publishers": () => [200, { by_hand: false, me: { state: "waiting", asked_at: 1760000500 }, approvers: ["github:alice", "github:bob", "github:xadmin"] }] },
  });
  await settle();
  check(/Waiting for an owner's computer to approve you as a publisher/.test(dev.el("pubNote").shown) && /@alice and 2 others can approve you/.test(dev.el("pubNote").shown) && /by itself/.test(dev.el("pubNote").shown), "a developer waiting to publish is told who can approve them");

  const set = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t",
    routes: { ...orgRoutes("github:alice"), "GET /orgs/acme": () => [200, { org: { ...ACME, release_rule: "solo" }, me: member("github:alice", "owner"), members: [member("github:alice", "owner")] }], "GET /orgs/acme/publishers": () => [200, pubs], "GET /orgs/acme/sso": () => [200, null], "GET /orgs/acme/exports": () => [200, []] },
  });
  await settle();
  check(/Approve new publishers by hand/.test(set.el("pane").shown) && /Require a second person from now on/.test(set.el("pane").shown), "Settings has Approve new publishers by hand, and the release rule when the hub has them");
}

process.exit(failures() ? 1 : 0);
