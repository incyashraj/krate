// The organisation dashboard's Insights tab (IC-959), run from the shipped
// scripts against a stand-in hub in the hub's shapes (GET
// /orgs/<slug>/insights, cloud/worker/src/org-insights.js): every role's
// sections, every number with what it means and where to act, every chart
// with its numbers as text, "fewer than 3" for a count the hub masked,
// names only where the hub sent them, and every state -- loading, empty,
// one app, partial, refused, offline, and 500 people drawn well inside a
// second.
//
//   node docs/landing/studio/org-insights-test.mjs
import { DASH, check, page, settle, ACME, member, button, failures } from "./org-test-kit.mjs";

const t = (s) => String(s || "").replace(/\s+/g, " ");
const NOW = 1792000000;
const DAY = 86400;
const iso = (k) => new Date((NOW - k * DAY) * 1000).toISOString().slice(0, 10);
const days = (known) => Array.from({ length: 30 }, (_, i) => ({ day: iso(29 - i), people: known[29 - i] ?? 0 }));
const weeks = (known) => Array.from({ length: 12 }, (_, i) => ({ from: iso(7 * (11 - i) + 6), to: iso(7 * (11 - i)), people: known[11 - i] ?? 0 }));
const FEWER = "<3";

const people = [
  member("github:alice", "owner"), member("github:erin", "admin"), member("github:bob", "reviewer"),
  member("github:carol", "developer"), member("github:dan", "recipient"), member("email:hal@acme.test", "recipient"),
];

function orgPart({ apps = 2, inactive = null, devices = null } = {}) {
  return {
    adoption: {
      members: 9, active_week: 3, active_month: 4,
      daily: days({ 9: 3, 8: 1, 6: 2, 1: 2, 0: 1 }), trend: weeks({ 5: 4, 1: 3, 0: 3 }),
      apps, apps_used: apps ? 1 : 0,
      top: apps ? [{ app_id: "com.acme.expenses", name: "Expenses", people: 4 }] : [],
      unused: apps > 1 ? [{ app_id: "com.acme.rota", name: "Rota", version: "1.0.0", digest: "r".repeat(64), released_at: NOW - 3 * DAY, owner: { principal: "email:frank@acme.test", display: "frank@acme.test", active: true } }] : [],
    },
    people: {
      by_role: { owner: 1, admin: 1, developer: 2, reviewer: 1, recipient: 4 },
      by_sign_in: { github: 6, email: 3, company: 0 },
      inactive: inactive || [{ principal: "email:hal@acme.test", display: "hal@acme.test", role: "recipient", last_active: iso(38), since: NOW - 80 * DAY }],
      company_sign_in: null, without_company_sign_in: [],
      invites: [{ principal: "email:kim@acme.test", display: "kim@acme.test", role: "developer", invited_by: { principal: "github:alice" }, issued_at: NOW - 2 * DAY, expires_at: NOW + 5 * DAY }],
    },
    security: {
      key: { version: 1, state: "stale", created_at: NOW - 75 * DAY },
      devices: devices || [
        { device_id: "a".repeat(16), name: "Alice's MacBook", principal: "github:alice", display: "alice", added_at: NOW - 75 * DAY, last_seen: NOW - 3600, holds_current: true, silent: false },
        { device_id: "b".repeat(16), name: "Erin's laptop", principal: "github:erin", display: "erin", added_at: NOW - 74 * DAY, last_seen: null, holds_current: true, silent: true },
      ],
      publishers: [{ principal: "github:carol", display: "carol", role: "developer", since: NOW - 80 * DAY, last_sent_at: NOW - 10 * DAY }],
      approvers: 2, two_person: true,
      refusals: { week: [{ reason: "not-a-member", n: 2 }, { reason: "release-withdrawn", n: 1 }], week_total: 3, weekly_average_before: 0.25, spike: false },
      withdrawn_offline: [{ app_id: "com.acme.expenses", name: "Expenses", version: "1.0.0", digest: "d".repeat(64), withdrawn_at: NOW - 2 * DAY, people: 1, until: NOW + 4 * DAY }],
      sign_in: { company: null, domain_pending: false },
      directory: { tokens: 1, last_used_at: NOW - 7200, week: { ok: 2, failed: 1, refused: 0 } },
      audit_export: null,
    },
    releases: { approved_month: 2, median_wait_seconds: 7200, approvals_counted: 3, waiting: 1, oldest_waiting_since: NOW - 3 * DAY, withdrawn_month: 1, rollbacks_month: 1 },
    cost: { plan: "stripe", state: "paid", seats_used: 5, seats_paid: 10, free_seats: 3, active_month: 4 },
  };
}
const devPart = (masked) => ({
  apps: [{
    app_id: "com.acme.expenses", name: "Expenses", version: "1.0.1", released: true,
    people_week: 4,
    daily: days(masked ? { 9: 3, 8: FEWER, 6: FEWER, 1: FEWER, 0: FEWER } : { 9: 3, 8: 1, 6: 2, 1: 2, 0: 1 }),
    trend: weeks({ 5: 4, 1: 3, 0: 4 }),
    versions: [{ version: "1.0.1", digest: "e".repeat(64), current: true, people: 3 }, { version: "1.0.0", digest: "d".repeat(64), current: false, people: masked ? FEWER : 1 }],
    refusals: [{ reason: "not-a-member", n: 3 }, { reason: "release-withdrawn", n: 1 }],
    data: { bytes: 1258291, quota: 52428800, writes_week: 140 },
    connections: [{ name: "finance-api", uses_week: 12 }],
    waits: [{ version: "1.0.1", state: "released", sent_at: NOW - 10 * DAY, approved_at: NOW - 10 * DAY + 7200, waited_seconds: 7200 }, { version: "1.0.0", state: "withdrawn", sent_at: NOW - 40 * DAY, approved_at: NOW - 39 * DAY, waited_seconds: DAY }],
  }],
});
const revPart = (names) => ({
  queue: [{ app_id: "com.acme.rota", name: "Rota", version: "1.1.0", digest: "f".repeat(64), sent_at: NOW - 2 * DAY, waited_seconds: 2 * DAY, yours: false, first: false, replaces: "1.0.0", added: [{ words: "use the camera", asks: true }], removed: [], kept: 2, ...(names ? { sent_by: { principal: "github:frank", display: "frank" } } : {}) }],
  approved_by_you_month: 2, approved_month: 2, median_wait_seconds_month: 5400,
});
const yoursPart = { devices: [{ device_id: "c".repeat(16), name: "Dan's ThinkPad", added_at: NOW - 30 * DAY, last_seen: NOW - 600, holds_current: true }], apps: [{ app_id: "com.acme.expenses", name: "Expenses", days: 5, last: iso(0) }] };

function body(role, extra = {}) {
  const high = role === "owner" || role === "admin";
  const sections = { owner: ["org", "developer", "reviewer", "yours"], admin: ["org", "yours"], developer: ["developer", "yours"], reviewer: ["reviewer", "yours"], recipient: ["yours"] }[role];
  const out = { v: 1, now: NOW, today: iso(0), role, names: high, since: NOW - 100 * DAY, sections };
  if (sections.includes("org")) out.org = orgPart();
  if (sections.includes("developer")) out.developer = devPart(!high);
  if (sections.includes("reviewer")) out.reviewer = revPart(high);
  out.yours = yoursPart;
  return { ...out, ...extra };
}

const who = { owner: "github:alice", admin: "github:erin", developer: "github:carol", reviewer: "github:bob", recipient: "github:dan" };
function routes(role, insights, extra = {}) {
  const me = people.find((m) => m.principal === who[role]);
  return {
    "GET /orgs/acme": () => [200, { org: { ...ACME, created_at: NOW - 120 * DAY, state: "active" }, me, members: role === "recipient" ? [me] : people }],
    "GET /orgs/acme/insights": () => (typeof insights === "function" ? insights() : [200, insights]),
    "GET /orgs/acme/billing": () => [200, { plan: "stripe", invoices: [{ id: "in_1", at: NOW - 5 * DAY, status: "paid", amount: 5000, currency: "usd" }] }],
    ...extra,
  };
}
const open = (role, insights, extra) => page("https://krate.tech/org/?o=acme&tab=insights", DASH, { token: "t", routes: routes(role, insights, extra) });
const pane = (p) => p.el("pane");
const shown = (p) => t(pane(p).shown);
/// What a person reads without opening any Details.
function outsideDetails(node) {
  if (node.hidden || node.tagName === "DETAILS") return "";
  if (node.tagName === "#TEXT") return node._text;
  return node.children.map(outsideDetails).join(" ");
}
const MACHINERY = /digest|sha256|hash|namespace|delegation|pkcs|manifest|capabilit|\blease|\bscim\b|\boidc\b|\bslug\b|principal|github:|email:|<3|\bkey\b/i;
const cardsOf = (p) => pane(p).all((n) => n.tagName === "SECTION" && n.className.includes("card ins"));
const linkTo = (root, tab) => root.all((n) => n.tagName === "A" && n.className === "act" && n.attrs.href.endsWith("&tab=" + tab));

/* ---- an owner: the organisation, then their own apps, approvals, yours ---- */
{
  const p = await open("owner", body("owner"));
  const s = shown(p);
  check(/Insights Is Acme getting value from its apps, and is it safe\? Counts only: Krate never sees what anyone does inside an app\./.test(s), "the tab says what it is for, and what Krate never sees");
  check(cardsOf(p).map((c) => c.attrs.id).join(",") === "ins-adoption,ins-people,ins-security,ins-releases,ins-cost,ins-app-0,ins-g-reviewer,ins-g-yours", "the owner reads the organisation's five cards, their app, approvals and their own");
  check(/Active in the last 7 days 3 of 9 people in Acme who opened one of its apps Invite your team/.test(s), "a number, what it means, where to act");
  check(/Active in the last 30 days 4 of 9 5 people have not opened one in a month See who has not/.test(s), "and the month, with who has not");
  check(/Apps in use 1 of 2 apps someone opened in the last 30 days See every app/.test(s), "apps in use");
  // Every tile has a meaning line; every tile but a few that have nothing to act on links to where to act.
  const tiles = pane(p).all((n) => n.tagName === "DIV" && (n.className === "tile" || n.className === "tile warn"));
  check(tiles.length >= 20 && tiles.every((x) => x.children[2] && x.children[2].className === "means" && x.children[2].textContent.length > 8), `every one of ${tiles.length} numbers says what it means`);
  const unlinked = tiles.filter((x) => !x.children[3]).map((x) => x.children[0].textContent);
  check(unlinked.join("|") === "Typical wait, last 90 days|Team data|Approved by you, last 30 days|Typical wait, last 30 days", "and all but the waits and stored data, which have nothing to click, link to where to act: " + unlinked.join(", "));
  // Charts: the numbers as text beside each, all of them in a table.
  const figs = pane(p).all((n) => n.tagName === "FIGURE");
  check(figs.length === 4, "two charts for the organisation, two for the app");
  const daily = figs[0];
  check(/People who opened an app, each day last 30 days/.test(t(daily.shown)) && /Most on one day: 3 · Today: 1/.test(t(daily.shown)), "a chart says its highest day and today in words");
  const rowsIn = (f) => f.all((n) => n.tagName === "TR").length - 1;
  check(rowsIn(daily) === 30 && rowsIn(figs[1]) === 12, "and carries every day and every week as a table (30 and 12 rows)");
  const bars = daily.all((n) => n.tagName === "SPAN" && (n.className === "b" || n.className.startsWith("b ")));
  check(bars.length === 30 && bars[20].attrs.title.endsWith(": 3 people") && daily.all((n) => n.attrs && n.attrs.role === "img")[0].attrs["aria-label"].startsWith("People who opened an app each day, last 30 days: most on one day 3"), "each bar has its number on hover, and the chart a sentence for a screen reader");
  check(/Most used, last 30 days Expenses 4 people/.test(s), "the most used app");
  check(/Nobody opened these in 30 days/.test(s) && button(pane(p), "Withdraw") === undefined && pane(p).all((n) => n.tagName === "A" && n.textContent === "Ask the owner" && n.attrs.href.startsWith("mailto:frank@acme.test")).length === 1, "an unused app offers Ask the owner (a mail) and Withdraw");
  check(linkTo(pane(p), "releases").some((a) => a.textContent === "Withdraw"), "Withdraw goes to Releases, where the confirmation is");
  // People.
  check(/Not seen for 30 days 1 no app opened and no visit here/.test(s) && /hal@acme\.test Member · last opened an app on \d+ Sep/.test(s), "who has stopped coming, by name, for an owner");
  check(/Company sign-in Off .* Set up company sign-in/.test(s) && /Invitations waiting 1 sent and not yet accepted Manage invitations/.test(s), "company sign-in off, and invitations waiting");
  check(/By role: 1 owner, 1 admin, 2 developers, 1 reviewer, 4 members\. Signing in: 6 with GitHub, 3 with an email address\./.test(s), "members by role and by sign-in");
  // Security.
  check(/Every release of Acme is signed by Acme, and a second person approves it before anyone can open it\./.test(s), "why Krate, where it is felt");
  check(/Computers with access 2 computers that can open Acme's encrypted team data; 1 has been silent for 60 days/.test(s), "computers with access, the silent one counted");
  const devRows = pane(p).all((n) => n.id === "ins-devices")[0];
  check(/^Erin's laptop Silent 60\+ days/.test(t(devRows.all((n) => n.className === "rw flat")[0].shown)), "a silent computer is listed first, flagged");
  check(/Refused in the last 7 days 3 attempts the hub said no to; about 0 a week before/.test(s) && /Not a member 2 times/.test(s) && /It was withdrawn 1 time/.test(s), "refusals this week, by reason, in words");
  check(/Expenses 1\.0\.0 Withdrawn .* 1 person can still open it offline until/.test(s), "a withdrawn release still held offline, until when");
  check(/Company directory Needs a look Last synced 2 hours ago · this week 2 worked, 1 failed/.test(s), "directory sync health");
  // Releases and cost.
  check(/Typical wait, last 90 days 2 hours the middle of the last 3 approvals/.test(s) && /Waiting now 1 the oldest for 3 days Review them/.test(s), "release waits");
  check(/Taken back, last 30 days 1 withdrawn and 1 rollback to an older version/.test(s), "withdrawals and rollbacks");
  check(/Seats in use 5 of 10 paid/.test(s) && /Cost per active person \$12\.50 the invoice of .*, \$50\.00, over the 4 people who opened an app in 30 days/.test(s), "seats, and the last invoice per active person");
  // An owner reads exact counts in their own app.
  check(/Expenses Version 1\.0\.1 opens now\. How is it doing\?/.test(s) && /1\.0\.0 1 person still on it/.test(s), "the owner's own app, exact counts");
  check(/Rota 1\.1\.0 Asks for more Waiting 2 days · sent by @frank · New: it can use the camera/.test(s), "the queue, who sent it (an owner sees names) and what it newly asks for");
  // The seamless bar, rule 3: no machinery words outside Details.
  const plain = outsideDetails(pane(p));
  const m = plain.match(MACHINERY);
  check(!m, "no machinery word outside Details" + (m ? `: "${m[0]}"` : ""));
  // Rule 1: one primary (dark) button at most per card.
  check(cardsOf(p).every((c) => c.all((n) => n.tagName === "BUTTON" && n.className.includes("dark")).length <= 1), "at most one primary button per card");
  // The section chips go to their headings.
  check(pane(p).all((n) => n.tagName === "NAV")[0].children.map((a) => a.attrs.href).join(" ") === "#ins-g-org #ins-g-developer #ins-g-reviewer #ins-g-yours", "the section links reach every section");
  check(pane(p).all((n) => n.id === "ins-g-reviewer" || n.id === "ins-g-yours").length === 2 && !pane(p).all((n) => n.tagName === "H3" && /Approvals|Yours/.test(n.textContent)).length, "a one-card section is not headed twice");
}

/* ---- Remove, from the people not seen for 30 days ---- */
{
  let sent = null;
  const p = await open("owner", body("owner"), {
    "DELETE /orgs/acme/members/email%3Ahal%40acme.test": (h) => { sent = h["idempotency-key"]; return [204, null]; },
  });
  const rm = pane(p).all((n) => n.tagName === "BUTTON" && n.textContent === "Remove")[0];
  rm.click();
  await settle();
  check(t(p.el("sheetH").textContent) === "Remove hal@acme.test from Acme?" && /Their access ends the next time they open an app/.test(p.el("sheetP").textContent), "Remove asks once, with the consequence in one line");
  check(p.el("sheetYes").textContent === "Remove hal@acme.test", "the button says what happens");
  p.el("sheetYes").click();
  await settle();
  check(Boolean(sent), "and removes them, once (an Idempotency-Key)");
  check(/hal@acme\.test was removed\. Their next open is refused\./.test(t(p.el("toast").shown)), "and says so");
}

/* ---- a developer: their app, counts masked, no names ---- */
{
  const b = body("developer");
  const p = await open("developer", b);
  const s = shown(p);
  check(cardsOf(p).map((c) => c.attrs.id).join(",") === "ins-app-0,ins-g-yours", "a developer reads their app and their own, nothing of the organisation");
  check(/How your apps are doing\. Counts only, never what anyone does inside them\./.test(s), "in the developer's words");
  check(/People in the last 7 days 4/.test(s) && /Refused, last 30 days 4 times the hub said no/.test(s), "people and refusals for their app");
  check(/Team data 1\.2 MB of 50 MB 140 changes in the last 7 days; encrypted, so not even Krate can read it/.test(s), "team data against its quota, and why Krate");
  const daily = pane(p).all((n) => n.tagName === "FIGURE")[0];
  check(/Most on one day: 3 · Today: fewer than 3 · Pale bars are fewer than 3 people, not shown exactly\./.test(t(daily.shown)), "a masked count reads fewer than 3, and the chart says what its pale bars are");
  check(daily.all((n) => n.tagName === "TD" && n.textContent === "fewer than 3").length === 4, "in the numbers too");
  check(daily.all((n) => n.className === "b few").length === 4, "drawn pale, never as an exact height");
  check(/1\.0\.0 fewer than 3 people still on it; they get the new one the next time they open it online/.test(s), "versions in use, masked");
  check(/finance-api 12 uses in the last 7 days, by all of Acme's apps; the app never holds the credential/.test(s), "connection use");
  check(/1\.0\.1 Waited 2 hours/.test(s) && /1\.0\.0 Waited 1 day · since withdrawn/.test(s), "how long their versions waited");
  check(!/@|alice|erin|hal/.test(s.replace("Acme's", "")), "no person's name anywhere");
  const copy = button(pane(p), "Copy its link");
  check(Boolean(copy), "the app's link to share, where people are counted");
  check(/^Versions in use, last 7 days 1\.0\.1 Opens now/.test(t(pane(p).all((n) => n.className === "isub")[0].shown)), "the version that opens now is listed first");

  // A first version still waiting: nothing to count, and it says why.
  const waitingOnly = body("developer");
  waitingOnly.developer = { apps: [{ ...devPart(true).apps[0], name: "Rota", version: null, released: false, people_week: 0, daily: days({}), trend: weeks({}), versions: [], refusals: [], data: null, connections: [], waits: [{ version: "1.0.0", state: "waiting", sent_at: NOW - 600, approved_at: null, waited_seconds: 600 }] }] };
  const w = await open("developer", waitingOnly);
  const ws = shown(w);
  check(/Nobody can open it until a second person approves it, so there is nothing to count yet\./.test(ws) && !w.el("pane").all((n) => n.tagName === "FIGURE").length, "a version still waiting shows no empty charts, and says why");
  check(/1\.0\.0 Waiting for 10 minutes so far See it/.test(ws), "with how long it has waited, and where to see it");
}

/* ---- a reviewer, an admin and a recipient ---- */
{
  const r = await open("reviewer", body("reviewer"));
  const s = shown(r);
  check(cardsOf(r).map((c) => c.attrs.id).join(",") === "ins-g-reviewer,ins-g-yours", "a reviewer reads what needs them, and their own");
  check(/Waiting for approval 1 the oldest for 2 days Review them/.test(s) && /Rota 1\.1\.0 Asks for more Waiting 2 days · New: it can use the camera Review/.test(s), "the queue, how long each waited, what changed, with Review");
  check(!/sent by/.test(s), "and not who sent it");
  check(/Approved by you, last 30 days 2 of 2 approvals in Acme/.test(s), "what they approved this month");
  const a = await open("admin", body("admin"));
  check(cardsOf(a).map((c) => c.attrs.id).join(",") === "ins-adoption,ins-people,ins-security,ins-releases,ins-cost,ins-g-yours", "an admin reads the organisation and their own");
  // A member's console is their apps: no Insights section (their own
  // computers and apps are on My apps).
  const d = await open("recipient", body("recipient"));
  check(d.location.href.endsWith("tab=home"), "a member asked for Insights lands on My apps");
  check(!d.el("pane").all((n) => n.tagName === "FIGURE").length, "no analytics for a recipient");
}

/* ---- empty, one app, partial ---- */
{
  const empty = body("owner");
  empty.org = orgPart({ apps: 0, inactive: [], devices: [] });
  empty.developer = { apps: [] };
  empty.reviewer = { queue: [], approved_by_you_month: 0, approved_month: 0, median_wait_seconds_month: null };
  empty.yours = { devices: [], apps: [] };
  const p = await open("owner", empty);
  const s = shown(p);
  check(/Acme has no apps yet Release one, a ready app or a \.krate you have, and this shows who opens it every day\. Release an app/.test(s), "an empty organisation teaches the next step");
  check(/You have not sent an app to Acme yet/.test(s) && /Nothing is waiting\. When someone sends a version, it shows here/.test(s), "each empty section says what will be there");
  check(/No computer holds access yet/.test(s) && /Everyone in Acme was here in the last 30 days\./.test(s), "empty lists are sentences, not blanks");
  button(pane(p), "Release an app").click();
  check(p.location.href.includes("tab=releases"), "Release an app goes to Releases");

  const one = body("owner");
  one.org = orgPart({ apps: 1 });
  const o = await open("owner", one);
  check(/Apps in use 1 of 1/.test(shown(o)) && !/Nobody opened these/.test(shown(o)), "one app: no unused list");

  // Solo release (IC-958): nobody else can approve yet, so the card does
  // not claim a second approval it cannot have.
  const solo = body("owner");
  solo.org = orgPart();
  solo.org.security.approvers = 1;
  solo.org.security.two_person = false;
  solo.org.security.solo_releases = 2;
  const so = await open("owner", solo);
  check(/Second approval Not yet nobody else can approve, so 2 versions were released by the person who sent it; it turns on when a reviewer joins Invite a reviewer/.test(shown(so)), "solo: the card says Not yet, and how many went out alone");
  check(/Until a second person can approve, you release your own apps yourself\./.test(shown(so)) && !/a second person approves it before anyone can open it/.test(shown(so)), "solo: the security sentence does not promise a second person");
  check(/Second approval On 2 people can approve a release/.test(shown(o)), "two-person: the card says On");

  // A new organisation: its people joined this month, nobody has been
  // away for 30 days, so nobody is "a month" away and there is no list to
  // send anyone to.
  const young = body("owner");
  young.org = orgPart({ inactive: [] });
  young.org.adoption.members = 3;
  young.org.adoption.active_month = 1;
  young.org.adoption.joined_not_opened = 2;
  const yo = await open("owner", young);
  check(/Active in the last 30 days 1 of 3 2 people joined in the last 30 days and have not opened one yet/.test(shown(yo)) && !/See who has not/.test(shown(yo)), "a new organisation: joined recently, not a month away, and no empty list to open");
  young.org.adoption.joined_not_opened = 1;
  young.org.people.inactive = [{ principal: "email:hal@acme.test", display: "hal@acme.test", role: "recipient", last_active: iso(38), since: NOW - 80 * DAY }];
  const yo2 = await open("owner", young);
  check(/1 person has not opened one in a month; 1 more joined recently See who has not/.test(shown(yo2)), "a mix: the settled one is counted, the new one named apart");

  // Which day the numbers count by (F-319): said to everyone, changed by
  // owners and admins in Settings.
  const zoned = await open("owner", body("owner", { time_zone: "Asia/Singapore", time_zone_chosen: true }));
  check(/Days run midnight to midnight, Singapore time\. Change/.test(shown(zoned)), "the owner reads which day the numbers count by, and can change it");
  const change = linkTo(pane(zoned), "settings")[0];
  check(change && /time zone/.test(change.attrs["aria-label"] || ""), "Change goes to Settings and says what it changes");
  const devZoned = await open("developer", body("developer", { time_zone: "America/Los_Angeles", time_zone_chosen: true }));
  check(/Days run midnight to midnight, Los Angeles time\./.test(shown(devZoned)) && !linkTo(pane(devZoned), "settings").length, "a developer reads it too, without a Change they cannot use");
  check(!/Days run midnight/.test(shown(o)), "a hub without time zones draws no line");

  const fresh = body("owner", { since: NOW - 5 * DAY });
  const f = await open("owner", fresh);
  check(/Counting since .*\. Days before that were not counted, so they show as 0\./.test(shown(f)), "partial: counting began after the organisation did, and it says so");
}

/* ---- loading, refused, offline ---- */
{
  // A hub that has not answered yet.
  const org = routes("owner", body("owner"))["GET /orgs/acme"]()[1];
  const slow = async (url) => {
    const path = new URL(url).pathname;
    if (path === "/orgs/acme/insights") return new Promise(() => {});
    return { ok: true, status: 200, text: async () => JSON.stringify(org), json: async () => org };
  };
  const p = await page("https://krate.tech/org/?o=acme&tab=insights", DASH, { token: "t", extra: { fetch: slow } });
  check(/Counting Acme's numbers/.test(shown(p)) && pane(p).all((n) => n.attrs && n.attrs.role === "status").length >= 1, "loading says what is happening, to a screen reader too");
  const refused = await open("owner", () => [403, { error: "not-a-member", message: "you are not a member of acme" }]);
  check(refused.el("pane").all((n) => n.className.includes("call")).length === 1 && !/\{"error"/.test(shown(refused)) && !button(refused.el("pane"), "Try again"), "a refusal is words, with no retry that cannot work");
  let calls = 0;
  const off = await page("https://krate.tech/org/?o=acme&tab=insights", DASH, {
    token: "t",
    routes: routes("owner", () => { calls++; return calls === 1 ? [500, null] : [200, body("owner")]; }),
  });
  check(Boolean(button(off.el("pane"), "Try again")), "a failure offers Try again");
  button(off.el("pane"), "Try again").click();
  await settle();
  check(/Adoption/.test(t(off.el("pane").shown)) && calls === 2, "and Try again draws the numbers");
}

/* ---- 500 people: drawn well inside a second ---- */
{
  const big = body("owner");
  const name = (i) => `email:p${String(i).padStart(3, "0")}@acme.test`;
  big.org = orgPart({
    inactive: Array.from({ length: 60 }, (_, i) => ({ principal: name(i), display: name(i).slice(6), role: "recipient", last_active: iso(40 + (i % 30)), since: NOW - 120 * DAY })),
    devices: Array.from({ length: 430 }, (_, i) => ({ device_id: String(i).padStart(16, "0"), name: `Laptop ${i}`, principal: name(i), display: name(i).slice(6), added_at: NOW - 100 * DAY, last_seen: NOW - (i % 90) * DAY, holds_current: true, silent: i % 90 > 60 })),
  });
  big.org.adoption.members = 504;
  big.org.adoption.active_month = 444;
  big.org.adoption.daily = days(Object.fromEntries(Array.from({ length: 30 }, (_, k) => [k, 200 + (k % 7) * 5])));
  big.org.adoption.top = Array.from({ length: 5 }, (_, i) => ({ app_id: `com.acme.a${i}`, name: `App ${i}`, people: 60 - i }));
  big.org.adoption.unused = Array.from({ length: 6 }, (_, i) => ({ app_id: `com.acme.u${i}`, name: `Old ${i}`, version: "1.0.0", released_at: NOW - 60 * DAY, owner: { principal: name(i), active: true } }));
  big.org.security.publishers = Array.from({ length: 42 }, (_, i) => ({ principal: name(i), role: "developer", since: NOW - 100 * DAY, last_sent_at: NOW - i * DAY }));
  big.org.people.invites = Array.from({ length: 12 }, (_, i) => ({ principal: `email:new${i}@acme.test`, role: "recipient", invited_by: { principal: "github:alice" }, issued_at: NOW - DAY, expires_at: NOW + 6 * DAY }));
  const started = performance.now();
  const p = await open("owner", big);
  const ms = Math.round(performance.now() - started);
  console.log(`     500 people, 430 computers, 60 not seen, 42 senders: drawn in ${ms} ms`);
  check(ms < 1000, `500 people are drawn in under a second (${ms} ms)`);
  const devices = p.el("pane").all((n) => n.id === "ins-devices")[0];
  check(devices.all((n) => n.className === "rw flat").length === 8 && Boolean(button(devices, "Show all 430 computers")), "a long list shows its first 8, the ones that need acting on first");
  check(/^Laptop \d+ Silent 60\+ days/.test(t(devices.all((n) => n.className === "rw flat")[0].shown)), "silent computers come first");
  button(devices, "Show all 430 computers").click();
  check(devices.all((n) => n.className === "rw flat").length === 430 && !button(devices, "Show all 430 computers"), "and Show all draws the rest");
  check(Boolean(button(p.el("pane"), "Show all 60 people")) && Boolean(button(p.el("pane"), "Show all 42 people")) && Boolean(button(p.el("pane"), "Show all 12 invitations")), "every long list is capped the same way");
}

/* ---- the page's matrix is the hub's ---- */
{
  const { KIT } = await import("./org-test-kit.mjs");
  const vm = await import("node:vm");
  const ctx = { URL, URLSearchParams };
  ctx.window = ctx;
  vm.createContext(ctx);
  vm.runInContext(KIT, ctx);
  const K = ctx.KrateOrg;
  const row = (a) => ["owner", "admin", "developer", "reviewer", "recipient"].map((r) => (K.MATRIX[a][r] ? "y" : "-")).join(" ");
  check(row("insights.view") === "y y y y y" && row("insights.org") === "y y - - -" && row("insights.developer") === "y - y - -" && row("insights.reviewer") === "y - - y -" && row("insights.people") === "y y - - -", "the page's matrix has IC-959's rows, as the hub's (org.js MATRIX)");
  check(K.tabsFor("recipient").join(",") === "home,requests,help" && ["owner", "admin", "developer", "reviewer"].every((r) => K.tabsFor(r).includes("insights")), "every role but members has the Insights tab");
}

console.log(failures() ? `${failures()} check(s) failed` : "ok  the Insights tab");
process.exit(failures() ? 1 : 0);
