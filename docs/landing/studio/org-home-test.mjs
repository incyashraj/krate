// The organisation console's own parts, run from the shipped scripts
// against a stand-in hub: the Dashboard every role starts on (org/home.js)
// with its four numbers, the bars to point at and what needs this person;
// the right-hand panel each of them opens; Settings in its window, with
// its sections down the side; and Activity narrowed by kind and by words.
//
//   node docs/landing/studio/org-home-test.mjs
import { DASH, check, page, settle, ACME, member, button, failures } from "./org-test-kit.mjs";

const t = (s) => String(s || "").replace(/\s+/g, " ");
const NOW = 1792000000;
const DAY = 86400;
const iso = (k) => new Date((NOW - k * DAY) * 1000).toISOString().slice(0, 10);
const days = (known) => Array.from({ length: 30 }, (_, i) => ({ day: iso(29 - i), people: known[29 - i] ?? 0 }));
const weeks = (known) => Array.from({ length: 12 }, (_, i) => ({ from: iso(7 * (11 - i) + 6), to: iso(7 * (11 - i)), people: known[11 - i] ?? 0 }));
const people = [member("github:alice", "owner"), member("github:bob", "reviewer"), member("github:carol", "developer"), member("github:dan", "recipient")];

const insights = {
  v: 1, now: NOW, today: iso(0), time_zone: "Asia/Singapore", role: "owner", names: true, sections: ["org", "developer", "reviewer", "yours"],
  org: {
    adoption: { members: 4, active_week: 3, active_month: 4, daily: days({ 0: 2, 1: 3, 5: 1 }), trend: weeks({ 0: 3, 1: 1 }), apps: 2, apps_used: 1, top: [{ app_id: "com.acme.notes", name: "Team Notes", people: 3 }], unused: [{ app_id: "com.acme.rota", name: "Rota", version: "1.0.0", digest: "r".repeat(64), released_at: NOW - 40 * DAY }], joined_not_opened: 0 },
    people: { by_role: { owner: 1, reviewer: 1, developer: 1, recipient: 1 }, inactive: [{ principal: "github:dan", display: "dan", role: "recipient", last_active: iso(38) }], invites: [{ principal: "email:kim@acme.test" }] },
    security: { devices: [], publishers: [], approvers: 2, two_person: true, solo_releases: 0, refusals: { week: [{ reason: "not-a-member", n: 5 }], week_total: 5, weekly_average_before: 1, spike: true } },
    releases: { approved_month: 1, waiting: 1, oldest_waiting_since: NOW - 2 * DAY },
  },
  reviewer: { queue: [{ name: "Rota", version: "1.1.0", waited_seconds: 2 * DAY }], approved_by_you_month: 1, approved_month: 1 },
  yours: { devices: [], apps: [] },
};
const audit = { chain_ok: true, events: [
  { seq: 1, at: NOW - 3 * DAY, actor: "github:alice", action: "org.create", target: "org:acme", result: "allowed" },
  { seq: 2, at: NOW - 2 * DAY, actor: "github:alice", action: "key.init", target: "key:v1", result: "allowed" },
  { seq: 3, at: NOW - DAY, actor: "github:mallory", action: "release.download", target: "release:" + "d".repeat(64), result: "denied", reason: "not-a-member" },
  { seq: 4, at: NOW - 3600, actor: "github:alice", action: "invite.create", target: "invite:email:kim@acme.test/developer", result: "allowed" },
] };
function routes(role) {
  const me = people.find((m) => m.role === role);
  return {
    "GET /orgs/acme": () => [200, { org: { ...ACME, created_at: NOW - 120 * DAY, state: "active", time_zone: "Asia/Singapore", time_zone_chosen: true }, me, members: people }],
    "GET /orgs/acme/insights": () => [200, insights],
    "GET /orgs/acme/audit": () => [200, audit],
    "GET /orgs/acme/candidates": () => [200, []],
    "GET /orgs/acme/releases": () => [200, []],
  };
}
const shown = (n) => t(n.shown);

/* ---- the Dashboard is where everyone starts ---- */
{
  const p = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: routes("owner") });
  await settle();
  const pane = p.el("pane");
  check(p.location.href.endsWith("tab=home") && p.el("tab-home").attrs["aria-selected"] === "true", "an organisation opens on its Dashboard");
  check(/Active this week 3 of 4 ▲ 2 vs last week/.test(shown(pane)), "the week's active people, with how it moved against last week");
  check(/Apps in use 1 of 2/.test(shown(pane)) && /Waiting for approval 1 the oldest for 2 days/.test(shown(pane)), "apps in use, and what is waiting with how long");
  check(/Refused this week 5 ▲ 4 vs a usual week/.test(shown(pane)), "refusals against a usual week; up is the bad direction");
  const bars = pane.all((n) => n.tagName === "BUTTON" && /^hc-b( |$)/.test(String(n.className)));
  check(bars.length === 30 && /: 2 people$/.test(bars[29].attrs["aria-label"]) && /: 3 people$/.test(bars[28].attrs["aria-label"]), "thirty bars, each saying its day and its number");
  check(/Needs you/.test(shown(pane)) && /1 version is waiting for approval Review/.test(shown(pane)) && /More refusals than usual this week See why/.test(shown(pane)), "what needs the owner, each with its click");
  check(/Days run midnight to midnight, Singapore time\./.test(shown(pane)), "and which day it counts by");

  // A number opens what is behind it, on the right.
  button(pane, "Active this week3of 4▲ 2 vs last week") || pane.all((n) => n.tagName === "BUTTON" && /^Active this week/.test(n.attrs["aria-label"] || ""))[0].click();
  const stat = pane.all((n) => n.tagName === "BUTTON" && /^Active this week/.test(n.attrs["aria-label"] || ""))[0];
  stat.click();
  await settle();
  check(p.el("crTitle").textContent === "People" && /Not seen for 30 days @dan last on/.test(shown(p.el("crBody"))), "a number opens its people in the right-hand panel");
  check(p.el("in").classList.contains("inspect") && p.el("crBack").hidden === false, "and the panel says how to go back");
  p.el("crBack").click();
  await settle();
  check(p.el("crTitle").textContent === "Acme" && /Quick actions/.test(shown(p.el("crBody"))), "back to the overview: quick actions and what changed lately");
  bars[29].click();
  await settle();
  check(p.el("crTitle").textContent === bars[29].attrs["aria-label"].split(":")[0] && /2 people opened one of Acme's apps that day/.test(shown(p.el("crBody"))), "a bar opens its day");
}

/* ---- the right-hand panel's overview speaks in words ---- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: routes("owner") });
  await settle();
  const feed = shown(p.el("crBody"));
  check(/turned on end-to-end encryption for the team's data/.test(feed) && /invited kim@acme\.test as developer/.test(feed), "what changed lately is said as what a person did");
  check(!/key\.init|key:v1|signing\.|ring:/.test(feed), "no event name or key ever reaches the panel");
}

/* ---- Settings is a window with its sections down the side ---- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: { ...routes("owner"), "GET /orgs/acme/sso": () => [200, null], "GET /orgs/acme/exports": () => [200, []], "GET /orgs/acme/connections": () => [200, []] } });
  await settle();
  p.el("tab-settings").click();
  await settle();
  const dlg = p.el("setDlg");
  check(dlg.attrs.open !== undefined && p.el("pane").parent === p.el("setSlot"), "Settings opens in its window, holding the page's own pane");
  const nav = p.el("setNav").all((n) => n.tagName === "BUTTON").map((b) => b.textContent);
  check(nav[0] === "General" && nav.includes("Signature") && nav.includes("Billing"), "its sections down the side, General first and Billing last: " + nav.join(", "));
  check(/Time zone/.test(shown(p.el("pane"))), "General holds the time zone");
  p.el("setClose").click();
  await settle();
  check(dlg.attrs.open === undefined && p.el("pane").parent !== p.el("setSlot") && p.location.href.endsWith("tab=releases"), "closing it puts the pane back and returns to where the person was");
}

/* ---- Activity, narrowed down ---- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=activity", DASH, { token: "t", routes: routes("owner") });
  await settle();
  const pane = p.el("pane");
  const entries = () => pane.all((n) => n.tagName === "BUTTON" && String(n.className).startsWith("ev")).length;
  check(entries() === 4, "every entry is listed");
  button(pane, "Refused 1").click();
  check(entries() === 1 && /Refused @mallory tried to open/.test(shown(pane)), "Refused shows only what the hub said no to");
  button(pane, "All").click();
  const find = pane.all((n) => n.tagName === "INPUT" && n.attrs.type === "search")[0];
  find.value = "kim";
  find.dispatch("input");
  check(entries() === 1 && /invited kim@acme\.test/.test(shown(pane)), "words find the entries that say them");
  pane.all((n) => n.tagName === "BUTTON" && String(n.className).startsWith("ev"))[0].click();
  await settle();
  check(p.el("crTitle").textContent === "What happened" && /Who @alice/.test(shown(p.el("crBody"))), "an entry opens in full on the right");
}

/* ---- Waiting: what is for you first, oldest first, how long, and what was decided ---- */
{
  const T = Math.floor(Date.now() / 1000);
  const cand = (name, who, at, extra = {}) => ({ digest: name[0].repeat(64), name, version: "1.0.0", app_id: "com.acme." + name.toLowerCase(), state: "waiting-for-approval", uploaded_by: who, uploaded_at: at, capabilities: [], changes: { against: null, added: [], removed: [], kept: 0 }, ...extra });
  const list = [
    cand("Rota", "github:carol", T - 3600),
    cand("Mine", "github:bob", T - 600),
    cand("Payroll", "github:carol", T - 3 * DAY, { changes: { against: "0.9.0", added: [], removed: [], kept: 2 } }),
    cand("Notes", "github:carol", T - 9 * DAY, { state: "released", approved_by: "github:bob", approved_at: T - 8 * DAY }),
  ];
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: { ...routes("reviewer"), "GET /orgs/acme/candidates": () => [200, list] } });
  await settle();
  const pane = p.el("pane");
  const s = shown(pane);
  check(/3 versions are waiting · the oldest for 3 days · 2 for you/.test(s), "how many wait, the oldest, and how many are this person's to decide");
  check(s.indexOf("Waiting for you") < s.indexOf("Payroll") && s.indexOf("Payroll") < s.indexOf("Rota") && s.indexOf("Rota") < s.indexOf("Waiting for someone else") && s.indexOf("Waiting for someone else") < s.indexOf("Mine"), "for you first, oldest first; your own under someone else");
  check(/Payroll 1\.0\.0 Update from 0\.9\.0/.test(s) && /Rota 1\.0\.0 New app/.test(s), "each says whether it is new or an update, and from what");
  const tags = pane.all((n) => n.tagName === "SPAN" && /^Waiting \d/.test(n.textContent));
  check(tags.length === 3 && tags[0].className.includes("warn") && !tags[1].className.includes("warn"), "how long each has waited, marked once it is past two days");
  check(/Recently decided Notes 1\.0\.0 Approved by @bob/.test(s), "and what was decided lately");
}

/* ---- Apps: narrowed by what needs doing ---- */
{
  const app = (name, extra) => ({ app_id: "com.acme." + name.toLowerCase(), name, state: "released", version: "1.0.0", audience: null, owner: { principal: "github:carol" }, needs_owner: false, last_sent_by: "github:carol", last_sent_at: NOW, capabilities: [], people: { month: 2 }, ...extra });
  const inv = { apps: [app("Rota", { needs_owner: true, owner: { principal: "github:gone" } }), app("Expenses", {}), app("Old", { state: "withdrawn" })] };
  const p = await page("https://krate.tech/org/?o=acme&tab=apps", DASH, { token: "t", routes: { ...routes("owner"), "GET /orgs/acme/inventory": () => [200, inv] } });
  await settle();
  const pane = p.el("pane");
  const cards = () => pane.all((n) => n.tagName === "DIV" && /\bappc\b/.test(String(n.className)) && !n.hidden).length;
  check(cards() === 3 && button(pane, "Needs an owner 1") && button(pane, "Withdrawn 1"), "every app, and a filter for each kind there is");
  button(pane, "Needs an owner 1").click();
  check(cards() === 1 && /Rota/.test(shown(pane)), "Needs an owner shows only the app that needs one");
  button(pane, "All 3").click();
  pane.all((n) => n.tagName === "DIV" && /\bapph\b/.test(String(n.className)))[1].click();
  await settle();
  check(/tab=apps&app=/.test(p.location.href) && /Expenses/.test(shown(p.el("pane"))) && button(p.el("pane"), "Overview"), "an app's head opens its own page");
}


/* ---- each role's own home ---- */
{
  // A developer: three ways to start, their apps, and what the team asked for.
  const dev = { ...insights, role: "developer", org: undefined, sections: ["developer", "yours"], reviewer: undefined,
    developer: { apps: [{ app_id: "com.acme.notes", name: "Team Notes", released: true, people_week: 3, daily: days({}), refusals: [{ reason: "not-in-audience", n: 2 }] }] } };
  const asked = [{ id: "r1", text: "A way to book the meeting rooms", by: "github:dan", at: NOW - DAY, state: "open", reply: "" }];
  const p = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: { ...routes("developer"), "GET /orgs/acme/insights": () => [200, dev], "GET /orgs/acme/requests": () => [200, { requests: asked }] } });
  await settle();
  const pane = shown(p.el("pane"));
  check(p.el("tab-home").shown.includes("Home") && /Your apps 1 1 released/.test(pane) && /People this week 3/.test(pane) && /Refused, 30 days 2/.test(pane), "a developer's home: their apps, who used them, what was refused");
  check(/Describe it/.test(pane) && /Start from a ready one/.test(pane) && /Send your own/.test(pane), "and the three ways to start one");
  check(/A way to book the meeting rooms/.test(pane) && /@dan/.test(pane), "and what the team asked for");
}
{
  // A member asks for an app; a developer answers it.
  const asked = [];
  const p = await page("https://krate.tech/org/?o=acme&tab=requests", DASH, { token: "t", routes: { ...routes("recipient"),
    "GET /orgs/acme/requests": () => [200, { requests: asked }],
    "POST /orgs/acme/requests": (h, b) => { asked.push({ id: "r1", text: b.text, by: "github:dan", at: NOW, state: "open", reply: "" }); return [201, { request: asked[0] }]; } } });
  await settle();
  const pane = p.el("pane");
  const box = pane.all((n) => n.tagName === "TEXTAREA")[0];
  box.value = "Hi";
  button(pane, "Ask for it").click();
  await settle();
  check(!p.sent.some((s) => s.key === "POST /orgs/acme/requests") && /Say a little more/.test(shown(pane)), "a request too short to act on is not sent");
  box.value = "A way to book the meeting rooms";
  button(pane, "Ask for it").click();
  await settle();
  const sent = p.sent.find((s) => s.key === "POST /orgs/acme/requests");
  check(sent && sent.body.text === "A way to book the meeting rooms", "a member asks for an app in a sentence");
  check(/A way to book the meeting rooms/.test(shown(p.el("pane"))) && /Asked/.test(shown(p.el("pane"))), "and sees it, asked and not yet answered");

  const d = await page("https://krate.tech/org/?o=acme&tab=requests", DASH, { token: "t", routes: { ...routes("developer"),
    "GET /orgs/acme/requests": () => [200, { requests: asked }],
    "PUT /orgs/acme/requests/r1": () => [200, { request: { ...asked[0], state: "planned" } }] } });
  await settle();
  d.el("pane").all((n) => n.tagName === "BUTTON" && /\bopens\b/.test(String(n.className)))[0].click();
  await settle();
  const ta = d.el("crBody").all((n) => n.tagName === "TEXTAREA")[0];
  ta.value = "Next week";
  button(d.el("crBody"), "Planned").click();
  await settle();
  const put = d.sent.find((s) => s.key === "PUT /orgs/acme/requests/r1");
  check(put && put.body.state === "planned" && put.body.reply === "Next week", "a developer answers it with planned and a line");

  // A hub from before requests says so, and offers nothing that can only fail.
  const old = await page("https://krate.tech/org/?o=acme&tab=requests", DASH, { token: "t", routes: { ...routes("recipient"), "GET /orgs/acme/requests": () => [404, { error: "hub-error", message: "" }] } });
  await settle();
  check(/Not on this hub yet/.test(shown(old.el("pane"))) && !button(old.el("pane"), "Ask for it"), "a hub without requests says so");
}


/* ---- search and the bell ---- */
{
  const inv = { apps: [{ app_id: "com.acme.expenses", name: "Expenses", releases: [], current: null }] };
  const items = [{ id: "waiting:x", kind: "approval-waiting", at: NOW - 60, text: "Rota 1.1.0 waits for your approval", tab: "waiting", target: "x" }];
  const p = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: { ...routes("owner"),
    "GET /orgs/acme/apps": () => [200, inv],
    "GET /orgs/acme/notifications": () => [200, { items, counts: { unread_hint: 1 } }] } });
  await settle();
  const bell = p.el("cbell");
  check(bell.classList.contains("has") && /1 new/.test(bell.attrs["aria-label"]), "the bell shows something is new");
  bell.click();
  await settle();
  check(p.el("crTitle").textContent === "Notifications" && /Rota 1\.1\.0 waits for your approval/.test(shown(p.el("crBody"))) && !bell.classList.contains("has"), "it opens on the right, and looking clears the dot");
  check(Number(p.localStorage.getItem("krate-org-seen-acme")) > 0, "the page remembers when it last looked");

  p.el("cfind").click();
  await settle();
  const dlg = p.el("findDlg");
  const input = dlg.all((n) => n.tagName === "INPUT")[0];
  input.value = "expen";
  input.dispatch("input");
  const rows = dlg.all((n) => n.tagName === "BUTTON" && /\bfd-row\b/.test(String(n.className)));
  check(rows.length === 1 && /Expenses/.test(shown(rows[0])), "search finds an app by part of its name");
  input.value = "people";
  input.dispatch("input");
  check(/Page People and groups/.test(shown(dlg)) && /Do Invite people/.test(shown(dlg)), "and pages and things to do by their words");
  input.value = "expen";
  input.dispatch("input");
  input.dispatch("keydown", { key: "Enter" });
  await settle();
  check(/tab=apps&app=com\.acme\.expenses/.test(p.location.href), "Enter opens what is picked");
}

console.log(failures() ? `${failures()} check(s) failed` : "ok  the console: Dashboard, panel, Settings, Activity, Waiting, Apps, role homes, Requests, search, bell");
process.exit(failures() ? 1 : 0);
