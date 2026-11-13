// Approving with the code in view, approval rules, and the one-tap link
// (G15, G16, G22), run from the shipped scripts against a stand-in hub in
// the hub's shapes (cloud/worker/src/org-approvals.js and code-changes.js).
//
//   node docs/landing/studio/org-approvals-test.mjs
import { DASH, check, page, settle, ACME, member, button, failures, orgRoutes } from "./org-test-kit.mjs";

const D1 = "1".repeat(64);
const D2 = "2".repeat(64);
const D3 = "3".repeat(64);
const D9 = "9".repeat(64);
const t = (s) => String(s || "").replace(/\s+/g, " ");
const people = [member("github:alice", "owner"), member("github:bob", "reviewer"), member("github:frank", "reviewer"), member("github:carol", "developer"), member("github:dan", "recipient"), member("github:erin", "admin")];
const approval = (extra = {}) => ({ rules: true, needed: 2, sensitive: true, approvals: [], groups: [], satisfied: false, words: "Needs 2 approvals", you: { approved: false, can: true, why: null, releases: false, after: "Needs one more approval" }, ...extra });
const cand = (digest, version, extra) => ({
  digest, name: "Expenses", version, state: "waiting-for-approval", uploaded_by: "github:carol", uploaded_at: 1760000300,
  approved_by: null, approved_at: null, app_id: "com.acme.expenses", audience: null, capabilities: [], changes: { against: { digest: D9, version: "1.0.3" }, added: [], removed: [], kept: 0 }, ...extra,
});
const CHANGES = {
  digest: D2, name: "Expenses", version: "1.1.0", against: { digest: D9, version: "1.0.3" }, new_app: false,
  source: { files: 6, old_files: 5 }, unchanged: 3, not_compared: 0, truncated: true, limits: { total: 409600, file: 65536 },
  totals: { files: 3, added_lines: 5, removed_lines: 1 },
  code: { size: 112840, old_size: 30923 },
  files: [
    { path: "src/lib.rs", status: "changed", binary: false, size: 90, old_size: 88, added_lines: 1, removed_lines: 1,
      hunks: [{ old_start: 4, old_lines: 3, new_start: 4, new_lines: 3, lines: ["     let x = 1;", "-    let y = 2;", "+    let y = 20;", "     paint(x, y);"] }] },
    { path: "src/net.rs", status: "added", binary: false, size: 70, old_size: 0, added_lines: 4, removed_lines: 0, truncated: true,
      hunks: [{ old_start: 0, old_lines: 0, new_start: 1, new_lines: 2, lines: ["+pub fn send() {", "+    unsafe { raw_send() }"] }] },
    { path: "src/big.rs", status: "changed", binary: false, size: 900000, old_size: 900000, added_lines: 4000, removed_lines: 4000, omitted: true, hunks: [] },
  ],
  risks: [
    { kind: "host", level: "high", text: "New network host: api.acme.example:443" },
    { kind: "dependency", level: "high", text: "New dependency: serde_json 1.0.128" },
    { kind: "unsafe", level: "high", text: "1 new unsafe block in src/net.rs" },
    { kind: "size", level: "note", text: "The compiled app grew from 30.2 KB to 110.2 KB (+265%)" },
    { kind: "made", level: "note", text: "Made with AI: Krate Studio with Claude (claude-opus-5)" },
  ],
};
const CANDIDATES = [
  cand(D2, "1.1.0", { approval: approval({ approvals: [{ principal: "github:frank", display: "frank", at: 1760000400 }], words: "Needs one more approval from security",
    groups: [{ group: "security", why: "it connects to a new host: api.acme.example:443", met: false }],
    you: { approved: false, can: true, why: null, releases: true, after: null } }) }),
  cand(D3, "2.0.0", { name: "Timesheet", app_id: "com.acme.timesheet", approval: approval() }),
  cand(D1, "1.0.0", { name: "Rota", app_id: "com.acme.rota", state: "released", approved_by: "github:bob", approved_at: 1760000500, approved_with: ["github:frank", "github:bob"] }),
];
const as = (me, extra = {}) => ({
  ...orgRoutes(me),
  "GET /orgs/acme": () => [200, { org: { ...ACME, state: "active", approval_policy: { default: false } }, me: people.find((m) => m.principal === me), members: people }],
  "GET /orgs/acme/candidates": () => [200, CANDIDATES],
  [`GET /orgs/acme/candidates/${D2}/changes`]: () => [200, CHANGES],
  [`GET /orgs/acme/candidates/${D3}/changes`]: () => [200, { ...CHANGES, digest: D3, files: [], risks: [], totals: { files: 0, added_lines: 0, removed_lines: 0 }, truncated: false }],
  ...extra,
});
const cardOf = (p, text) => p.el("pane").querySelectorAll(".wcard").find((r) => r.querySelector("b") && r.querySelector("b").textContent.startsWith(text));
const codeOf = (card) => card.querySelector(".cchg");

{
  // The waiting card shows what changed in the code, and the rules' word.
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: as("github:bob") });
  const card = cardOf(p, "Expenses");
  check(/Update from 1\.0\.3/.test(t(card.shown)), "an update names the version it replaces, not an object");
  const code = codeOf(card);
  check(code && /What changed in the code/.test(code.shown), "the card has What changed in the code");
  check(/Compared with 1\.0\.3: 3 files changed, \+5 −1, 3 files the same\./.test(t(code.shown)), "with a one-line summary of the change");
  const risks = code.querySelector(".crisk").children;
  check(risks.length === 5 && risks.filter((r) => r.classList.contains("high")).map((r) => r.textContent).join("|") === "New network host: api.acme.example:443|New dependency: serde_json 1.0.128|1 new unsafe block in src/net.rs", "the risk rows, the ones to look at marked");
  check(/Made with AI: Krate Studio with Claude \(claude-opus-5\)/.test(code.shown) && /grew from 30\.2 KB to 110\.2 KB/.test(code.shown), "how it was made and how much the app grew");
  const files = code.querySelectorAll(".cfile");
  check(files.length === 3 && /Changed src\/lib\.rs \+1 −1/.test(t(files[0].querySelector("summary").shown)) && /Added src\/net\.rs/.test(t(files[1].shown)), "each file with what happened to it and its line counts");
  check(!files[0].querySelector(".cdiff"), "a file's diff is drawn when it is opened, not before");
  files[0].setAttribute("open", "");
  files[0].dispatch("toggle");
  const rows = files[0].querySelectorAll("tr");
  const cells = (r) => r.children.map((c) => c.textContent).join("|");
  check(rows.length === 5 && /Lines 4 to 6/.test(rows[0].textContent), "opened: a header for each hunk, in plain words");
  check(cells(rows[2]) === "5||−    let y = 2;" && rows[2].classList.contains("del") && cells(rows[3]) === "|5|+    let y = 20;" && rows[3].classList.contains("add") && cells(rows[4]) === "6|6|     paint(x, y);", "each line numbered old and new, removed and added marked");
  check(files[0].querySelector(".cdiff").attrs.tabindex === "0" && files[0].querySelector(".cdiff").attrs.role === "region", "the diff scrolls on its own, and can be reached by keyboard");
  files[1].setAttribute("open", "");
  files[1].dispatch("toggle");
  check(/Cut here: the rest of this file's changes are too long to show/.test(files[1].shown), "a file cut at its bound says so");
  files[2].setAttribute("open", "");
  files[2].dispatch("toggle");
  check(/Not shown: the changes above reached the limit/.test(files[2].shown), "a file past the total bound says so");
  check(/Some changes are too long to show here \(over 400 KB in all\)/.test(code.shown), "and the whole comparison says it was cut");

  // Where it stands against the rules.
  const pol = card.querySelector(".apol");
  check(pol && /Needs one more approval from security/.test(pol.shown) && /Approved by @frank/.test(pol.shown) && /Needs an approver from security: it connects to a new host/.test(pol.shown), "the rules' word: what is still needed, who approved, and why");
  check(button(card, "Approve") && !button(card, "Approve").disabled, "bob can give the approval that releases it");
}

{
  // An approval that does not release yet: the sheet says so, and so does the page after.
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, {
    token: "t", routes: as("github:bob", { [`POST /orgs/acme/candidates/${D3}/approve`]: () => [202, { digest: D3, state: "waiting-for-approval", approval: approval({ words: "Needs one more approval", approvals: [{ principal: "github:bob", display: "bob", at: 1 }] }) }] }),
  });
  const card = cardOf(p, "Timesheet");
  check(/Needs 2 approvals/.test(card.shown), "a version with nobody yet says how many it needs");
  button(card, "Approve").click();
  await settle();
  check(/Your approval is recorded, and it keeps waiting: it needs one more approval\./.test(p.el("sheetP").textContent) && p.el("sheetYes").textContent === "Approve", "the sheet says this approval does not release it yet");
  p.el("sheetYes").onclick();
  await settle();
  check(p.sent.some((s) => s.key === `POST /orgs/acme/candidates/${D3}/approve`), "the approval goes to the hub");
  check(/Your approval is in\. Timesheet 2\.0\.0 needs one more approval\./.test(p.el("toast").shown), "and the page says what is still needed");
}

{
  // Someone the rules do not count, and someone who approved already.
  const cannot = CANDIDATES.map((c) => (c.digest === D2 ? { ...c, approval: { ...c.approval, you: { approved: false, can: false, why: "it needs an approval from security, and you are not in that group" } } } : c));
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: as("github:bob", { "GET /orgs/acme/candidates": () => [200, cannot] }) });
  const card = cardOf(p, "Expenses");
  check(!button(card, "Approve") && button(card, "Decline") && /Your approval would not count: it needs an approval from security, and you are not in that group\./.test(card.shown), "an approval that would not count is not offered, and the page says why");
  check(/Waiting for someone else/.test(p.el("pane").shown), "and it is listed as waiting for someone else");
  const done = CANDIDATES.map((c) => (c.digest === D2 ? { ...c, approval: { ...c.approval, you: { approved: true, can: false, why: "you approved it" } } } : c));
  const q = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: as("github:frank", { "GET /orgs/acme/candidates": () => [200, done] }) });
  check(/You approved this\. Needs one more approval from security\./.test(cardOf(q, "Expenses").shown) && !button(cardOf(q, "Expenses"), "Approve"), "someone who approved sees that, and what is left");
}

{
  // An older hub, or a person refused the code: the card works as before.
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: as("github:bob", { [`GET /orgs/acme/candidates/${D2}/changes`]: () => [404, null] }) });
  const card = cardOf(p, "Expenses");
  check(codeOf(card).hidden === true && button(card, "Approve"), "no code route: the section is not shown, and Approve works");
  const q = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: as("github:erin", { [`GET /orgs/acme/candidates/${D2}/changes`]: () => [403, { error: "role-cannot", message: "" }] }) });
  check(codeOf(cardOf(q, "Expenses")).hidden === true, "refused: nothing about the code is shown");
}

{
  // The approval email's link: one version, its code open, Approve and Decline.
  const p = await page(`https://krate.tech/org/?o=acme&tab=waiting&c=${D2}`, DASH, { token: "t", routes: as("github:bob") });
  const cards = p.el("pane").querySelectorAll(".wcard");
  check(cards.length === 1 && /Expenses/.test(cards[0].shown), "the link opens that version alone");
  const files = codeOf(cards[0]).querySelectorAll(".cfile");
  check(files[0].getAttribute("open") === "" && files[0].querySelector(".cdiff"), "with its changed files open");
  check(button(cards[0], "Approve") && button(cards[0], "Decline") && button(p.el("pane"), "Everything waiting"), "Approve, Decline, and the way back to everything waiting");
  check(/tab=waiting&c=2{64}/.test(p.location.href), "a reload keeps the version");
  button(p.el("pane"), "Everything waiting").click();
  await settle();
  check(p.el("pane").querySelectorAll(".wcard").length === 2 && !/&c=/.test(p.location.href), "and the way back lists everything waiting");
}

{
  // A link to a version already decided says who decided and when.
  const p = await page(`https://krate.tech/org/?o=acme&tab=waiting&c=${D1}`, DASH, { token: "t", routes: as("github:bob") });
  check(/Already approved/.test(p.el("pane").shown) && /@frank and @bob approved Rota 1\.0\.0/.test(t(p.el("pane").shown)) && /It is released/.test(p.el("pane").shown), "approved already: by whom, and that it is out");
  check(!button(p.el("pane"), "Approve"), "and nothing to approve");
  const declined = CANDIDATES.concat([cand("4".repeat(64), "1.2.0", { state: "declined", declined_by: "github:alice", declined_at: 1760000600, decline_reason: "No camera, please" })]);
  const q = await page(`https://krate.tech/org/?o=acme&tab=waiting&c=${"4".repeat(64)}`, DASH, { token: "t", routes: as("github:bob", { "GET /orgs/acme/candidates": () => [200, declined] }) });
  check(/Already declined/.test(q.el("pane").shown) && /@alice declined Expenses 1\.2\.0 .*No camera, please/.test(t(q.el("pane").shown)), "declined already: by whom, and why");
  const r = await page(`https://krate.tech/org/?o=acme&tab=waiting&c=${"5".repeat(64)}`, DASH, { token: "t", routes: as("github:bob") });
  check(/This version is not here/.test(r.el("pane").shown), "a version that is not here says so");
}

{
  // The bell opens a waiting version on its own.
  const items = [{ id: `waiting:${D2}`, kind: "approval-waiting", at: 1760000300, text: "Expenses 1.1.0 waits for your approval", tab: "waiting", target: D2 }];
  const p = await page("https://krate.tech/org/?o=acme&tab=home", DASH, { token: "t", routes: as("github:bob", { "GET /orgs/acme/notifications": () => [200, { items, counts: { unread_hint: 1 } }] }) });
  p.el("cbell").click();
  await settle();
  const row = p.el("crBody").all((n) => n.tagName === "BUTTON" && /\bnrow\b/.test(String(n.className)))[0];
  row.click();
  await settle();
  check(/tab=waiting&c=2{64}/.test(p.location.href) && p.el("pane").querySelectorAll(".wcard").length === 1, "a notification about a waiting version opens it alone");
}

{
  // An app's Versions tab shows the waiting version with its code.
  const p = await page("https://krate.tech/org/?o=acme&tab=apps&app=com.acme.expenses&view=versions", DASH, {
    token: "t", routes: as("github:bob", { "GET /orgs/acme/apps": () => [200, { apps: [] }], "GET /orgs/acme/inventory": () => [200, { apps: [] }], "GET /orgs/acme/insights": () => [404, null] }),
  });
  const card = cardOf(p, "Expenses");
  check(card && codeOf(card) && /New network host/.test(codeOf(card).shown) && button(card, "Approve"), "the app's Versions tab has the waiting card, its code, and Approve");
}

{
  // Settings: an owner sets the rules; anyone else reads them.
  const routes = (me, extra = {}) => as(me, {
    "GET /orgs/acme/settings/approvals": () => [200, { policy: { approvals: 1, sensitive_approvals: 2, reviewer_groups: [{ app: "*", group: "finance" }], security_group: null }, default: false, words: "one approval for every version; two for one that asks for the network, files or the camera, or adds a capability; one approver from finance for every app", warnings: ["Nobody in finance can approve: add a reviewer or an owner to finance, or versions that need finance wait"] }],
    "GET /orgs/acme/groups": () => [200, { groups: [{ name: "finance" }, { name: "security" }] }],
    "GET /orgs/acme/apps": () => [200, { apps: [{ app_id: "com.acme.expenses", name: "Expenses" }] }],
    "GET /orgs/acme/exports": () => [200, []],
    "GET /orgs/acme/sso": () => [200, null],
    ...extra,
  });
  let put = null;
  const p = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: routes("github:alice", { "PUT /orgs/acme/settings/approvals": (h, b) => { put = b; return [200, { policy: b, default: false, words: "two approvals for every version", warnings: [] }]; } }) });
  const card = p.el("rulesCard");
  check(/Approval rules/.test(card.shown) && /Nobody in finance can approve/.test(card.shown), "an owner sees the rules, with the hub's warning");
  const segs = card.querySelectorAll(".seg");
  check(segs.length === 2 && segs[1].children[1].getAttribute("aria-pressed") === "true", "the counts show what is set");
  segs[0].children[1].click();
  const selects = card.querySelectorAll("select");
  check(selects[0].value === "finance", "the group for every app shows what is set");
  selects[selects.length - 1].value = "security";
  button(card, "Add a group for one app").click();
  const appRow = card.querySelector(".rapp").querySelectorAll("select");
  appRow[1].value = "security";
  button(card, "Save the rules").click();
  await settle();
  check(put && put.approvals === 2 && put.sensitive_approvals === 2 && put.security_group === "security" && JSON.stringify(put.reviewer_groups) === JSON.stringify([{ app: "*", group: "finance" }, { app: "com.acme.expenses", group: "security" }]), "saving sends exactly the rules chosen");
  check(/Saved\. From now on each version needs two approvals for every version\./.test(p.el("toast").shown) && !/Nobody in finance/.test(card.shown), "and says what they are now");
  const r = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: routes("github:bob") });
  const rc = r.el("rulesCard");
  check(/Each version needs one approval for every version/.test(rc.shown) && /An owner of Acme sets these/.test(rc.shown) && !button(rc, "Save the rules") && !rc.querySelectorAll("select").length, "a reviewer reads the rules and cannot change them");
}

if (failures()) process.exit(1);
console.log("ok  approvals show the code, follow the rules, and open from one link");
