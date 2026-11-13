// The organisation dashboard's release control and groups (IC-949,
// FLEET-CONTRACT.md R, proof item 7), run from the shipped scripts against
// a stand-in hub in the hub's shapes: Releases by app with the version that
// opens now and its history, Withdraw and what its sheet says about offline
// access already given out, Make current, who can open a release, the
// Groups card, and the words for each new refusal and audit event.
//
//   node docs/landing/studio/org-releases-test.mjs
import { DASH, OPEN, check, page, settle, ACME, member, button, failures, orgRoutes } from "./org-test-kit.mjs";

const ALICE = "github:alice";
const APP = "dev.krate.bounce";
const D0 = "0".repeat(63) + "a";
const D1 = "1".repeat(64);
const D2 = "2".repeat(64);
const UNTIL = Date.UTC(2026, 9, 21, 9, 30) / 1000;
const t = (s) => String(s || "").replace(/\s+/g, " ");
const rel = (digest, version, extra = {}) => ({
  app_id: APP, name: "Bounce", version, digest, url: `https://hub.krate.tech/a/${digest}`, state: "released",
  uploaded_by: "github:carol", approved_by: "github:bob", at: 1760000300, audience: null, offline: "days-7", ...extra,
});
const APPS = {
  apps: [{
    app_id: APP, name: "Bounce", current: D2,
    releases: [
      rel(D2, "1.0.2", { leases: { active: 2, until: UNTIL } }),
      rel(D1, "1.0.1", { audience: ["finance"], leases: { active: 0, until: null } }),
      rel(D0, "1.0.0", { state: "withdrawn", withdrawn_at: 1760100000, withdrawn_by: "github:bob", withdraw_reason: "loses data on save" }),
    ],
  }],
};
const GROUPS = { groups: [{ name: "finance", created_at: 1760000000, created_by: ALICE, members: ["github:dan"] }, { name: "ops", created_at: 1760000000, created_by: ALICE, members: [] }] };
const people = [member(ALICE, "owner"), member("github:bob", "reviewer"), member("github:carol", "developer"), member("github:dan", "recipient"), member("github:erin", "admin")];
const as = (me, extra = {}) => ({
  ...orgRoutes(me),
  "GET /orgs/acme": () => [200, { org: { ...ACME, state: "active", offline_default: "days-7" }, me: people.find((m) => m.principal === me), members: me === "github:dan" ? [people[3]] : people }],
  "GET /orgs/acme/apps": () => [200, APPS],
  "GET /orgs/acme/groups": () => [200, me === "github:dan" ? { groups: [{ ...GROUPS.groups[0], members: ["github:dan"] }] } : GROUPS],
  ...extra,
});
const rowOf = (p, text) => p.el("pane").querySelectorAll(".rw").find((r) => r.querySelector("b") && r.querySelector("b").textContent.startsWith(text));

/* ---- Releases by app ---------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t", routes: as(ALICE, { [`POST /orgs/acme/releases/${D2}/withdraw`]: () => [200, { release: rel(D2, "1.0.2", { state: "withdrawn" }), current: { app_id: APP, digest: D1 }, leases: { active: 2, until: UNTIL } }] }),
  });
  const pane = t(p.el("pane").shown);
  check(/Bounce/.test(pane) && !/dev\.krate\.bounce/.test(pane), "each app is a card named by its name; its app id waits under Technical details");
  const cur = rowOf(p, "Bounce 1.0.2");
  check(cur && /Opens now/.test(cur.shown) && p.el("pane").querySelectorAll(".rw")[0] === cur, "the version that opens now comes first, marked Opens now");
  check(/Other versions/.test(pane), "then the other versions");
  const old = rowOf(p, "Bounce 1.0.1");
  check(/Only for the finance group/.test(old.shown) && /For everyone in Acme/.test(cur.shown), "each says who it is for");
  const gone = rowOf(p, "Bounce 1.0.0");
  check(/Withdrawn/.test(gone.shown) && /Withdrawn by @bob on .*: “loses data on save”/.test(t(gone.shown)) && !button(gone, "Open in Krate") && !button(gone, "Withdraw"), "a withdrawn version shows who, when and why, and offers nothing");
  check(button(cur, "Open in Krate") && button(cur, "Copy link") && button(cur, "Withdraw") && button(cur, "Who can open it") && !button(cur, "Make current"), "the current one: open, copy, withdraw, audience, and no Make current");
  check(button(old, "Make current"), "an older approved one can be made current");
  check(p.el("pane").querySelectorAll("select").length === 1, "the offline class is chosen once per app, not per version");

  // Withdraw: the sheet says what stops at once and what offline access holds.
  button(cur, "Withdraw").click();
  const body = t(p.el("sheetP").textContent + " " + p.el("sheetPts").shown);
  check(/Withdraw Bounce 1\.0\.2\?/.test(p.el("sheetH").textContent) && p.el("sheetDigest").shown.includes(D2), "the withdraw sheet names the version and its exact digest");
  check(/nobody can download it or renew offline access to it, and a computer that has it refuses it the next time it connects/.test(body), "it says downloads and renewals stop now, and devices refuse at their next check-in");
  check(/Offline access already given out keeps working without a connection until it runs out: 2 people hold it, the last until 21 Oct 2026/.test(body), "it says exactly how long offline access already given out still works");
  check(p.el("sheetPts").all((n) => n.tagName === "LI" && n.classList.contains("key")).some((li) => /2 people hold it/.test(li.textContent)), "and that line stands out");
  check(/It is the version that opens now; Bounce 1\.0\.1 opens instead\./.test(body), "and which version opens instead");
  check(/This is not undone/.test(body), "and that it is not undone");
  check(p.el("sheetIn").hidden === false, "a reason is asked for");
  p.el("sheetInput").value = "   ";
  p.el("sheetYes").onclick();
  await settle();
  check(p.sent.every((s) => !s.key.endsWith("/withdraw")) && /Say why it is withdrawn/.test(p.el("sheetWhy").shown) && p.el("sheetYes").hidden === false, "without a reason nothing is sent, and the sheet stays to take one");
  p.el("sheetInput").value = " crashes on open ";
  p.el("sheetYes").onclick();
  await settle();
  const w = p.sent.find((s) => s.key === `POST /orgs/acme/releases/${D2}/withdraw`);
  check(w && w.body.reason === "crashes on open" && w.headers["idempotency-key"], "Withdraw sends the reason, once, with an Idempotency-Key");
  check(/Bounce 1\.0\.2 is withdrawn\. Bounce 1\.0\.1 opens now\./.test(p.el("toast").shown), "and says what opens now");

  // An online-only release, nobody holding offline access.
  const online = { apps: [{ ...APPS.apps[0], releases: [rel(D2, "1.0.2", { offline: "online", leases: { active: 0, until: null } })] }] };
  const o = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: as(ALICE, { "GET /orgs/acme/apps": () => [200, online] }) });
  button(o.el("pane"), "Withdraw").click();
  const pts = o.el("sheetPts").shown;
  check(/It is online only, so it stops everywhere at once\./.test(pts) && /It is the only version, so nothing of Bounce opens after this\./.test(pts), "online only: it stops everywhere at once, and nothing opens after");

  // A withdraw the hub refuses stays on the sheet, in words.
  const late = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t", routes: as(ALICE, { [`POST /orgs/acme/releases/${D2}/withdraw`]: () => [410, { error: "release-withdrawn", message: "", withdrawn: { at: 1760100000, reason: "done already" } }] }),
  });
  button(rowOf(late, "Bounce 1.0.2"), "Withdraw").click();
  late.el("sheetInput").value = "again";
  late.el("sheetYes").onclick();
  await settle();
  check(/This version was withdrawn/.test(late.el("sheetWhy").shown) && /“done already”/.test(late.el("sheetWhy").shown) && !/release-withdrawn/.test(late.el("sheetWhy").shown), "a refused withdraw says so in words, with the reason it was withdrawn");
}

/* ---- Make current (the rollback) --------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t", routes: as(ALICE, { [`PUT /orgs/acme/apps/${APP}/current`]: (_h, b) => [200, { app_id: APP, current: rel(b.digest, "1.0.1") }] }),
  });
  button(rowOf(p, "Bounce 1.0.1"), "Make current").click();
  check(/Make Bounce 1\.0\.1 current\?/.test(p.el("sheetH").textContent) && /gets 1\.0\.1 instead of 1\.0\.2, which stays released, so you can switch back\./.test(t(p.el("sheetP").textContent)), "the sheet says which version opens instead of which, and that it can switch back");
  check(p.sent.every((s) => !s.key.endsWith("/current")), "nothing changes before the person confirms");
  p.el("sheetYes").onclick();
  await settle();
  const put = p.sent.find((s) => s.key === `PUT /orgs/acme/apps/${APP}/current`);
  check(put && put.body.digest === D1 && put.headers["idempotency-key"], "Make current sends that digest for the app id");
}

/* ---- Who can open it --------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t", routes: as("github:erin", { [`PUT /orgs/acme/releases/${D2}/audience`]: (_h, b) => [200, { digest: D2, audience: b.groups || null }] }),
  });
  const cur = rowOf(p, "Bounce 1.0.2");
  button(cur, "Who can open it").click();
  await settle();
  const box = cur.querySelector(".audbox");
  check(box && !box.hidden && /Owners, admins and reviewers always open it/.test(box.shown), "the picker opens in place, saying who always passes");
  check(!/finance \(/.test(box.shown), "for everyone, no groups are offered");
  button(box, "Only some groups").click();
  check(/finance \(1 person\)/.test(box.shown) && /ops \(0 people\)/.test(box.shown), "only some groups lists them, with how many are in each");
  button(box, "Save").click();
  await settle();
  check(p.sent.every((s) => !s.key.endsWith("/audience")) && /Pick at least one group/.test(box.shown), "only some groups with none picked sends nothing, and says so");
  const fin = box.all((n) => n.tagName === "INPUT" && n.attrs.type === "checkbox")[0];
  fin.checked = true;
  fin.dispatch("change");
  button(box, "Save").click();
  await settle();
  const put = p.sent.find((s) => s.key === `PUT /orgs/acme/releases/${D2}/audience`);
  check(put && JSON.stringify(put.body) === '{"groups":["finance"]}' && put.headers["idempotency-key"], "Save sends exactly the groups picked");
  check(/only for the finance group/.test(p.el("toast").shown), "and says who it is for now");

  const p2 = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: as(ALICE, { [`PUT /orgs/acme/releases/${D1}/audience`]: (_h, b) => [200, { digest: D1, audience: null }] }) });
  const old = rowOf(p2, "Bounce 1.0.1");
  button(old, "Who can open it").click();
  await settle();
  const box2 = old.querySelector(".audbox");
  check(box2.all((n) => n.tagName === "INPUT" && n.attrs.type === "checkbox")[0].checked === true, "a limited release opens the picker with its groups ticked");
  button(box2, "Everyone in Acme").click();
  button(box2, "Save").click();
  await settle();
  check(JSON.stringify(p2.sent.find((s) => s.key === `PUT /orgs/acme/releases/${D1}/audience`).body) === '{"everyone":true}', "Everyone sends everyone: true");
}

/* ---- each role sees only what it may use ------------------------------- */
{
  const bob = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: as("github:bob") });
  const b = rowOf(bob, "Bounce 1.0.1");
  check(button(b, "Withdraw") && !button(b, "Make current") && !button(b, "Who can open it"), "a reviewer withdraws, and does not choose current or audience");
  const carol = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: as("github:carol") });
  const c = rowOf(carol, "Bounce 1.0.2");
  check(button(c, "Open in Krate") && !button(c, "Withdraw") && !button(c, "Make current") && !button(c, "Who can open it"), "a developer opens and copies, nothing else");
  // A member lands on their apps, each ready to open, and nothing to manage.
  const dan = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: as("github:dan") });
  const card = dan.el("pane").querySelectorAll(".lcard").find((n) => n.querySelector("b") && n.querySelector("b").textContent === "Bounce");
  check(card && button(card, "Open in Krate") && button(card, "Copy link") && !button(dan.el("pane"), "Withdraw") && !button(dan.el("pane"), "Make current"), "a recipient opens and copies from their apps, nothing else");
  check(button(dan.el("pane"), "Ask for it"), "and can ask for an app that is not there");
  // A hub without release control: the flat list, as before.
  const flat = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: { ...as(ALICE), "GET /orgs/acme/apps": () => [404, { error: "hub-error", message: "" }] } });
  check(button(flat.el("pane"), "Open in Krate") && !button(flat.el("pane"), "Withdraw") && /Sent by @carol/.test(flat.el("pane").shown), "a hub without /apps still shows the flat releases list");
}

/* ---- Groups on People -------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=people", DASH, {
    token: "t", routes: as(ALICE, {
      "POST /orgs/acme/groups": (_h, b) => [201, { group: { name: b.name, members: [] } }],
      "PUT /orgs/acme/groups/ops/members/github%3Acarol": () => [200, { group: "ops", principal: "github:carol", member: true }],
      "DELETE /orgs/acme/groups/finance/members/github%3Adan": () => [204, null],
    }),
  });
  const card = p.el("groupsCard");
  check(card && /Groups/.test(card.shown) && /finance/.test(card.shown) && /@dan/.test(card.shown) && /ops/.test(card.shown) && /Nobody yet/.test(card.shown), "People has a Groups card: each group and who is in it");
  p.el("groupName").value = "Finance Team";
  button(card, "Make group").click();
  await settle();
  check(p.sent.every((s) => s.key !== "POST /orgs/acme/groups") && /That group name will not work/.test(card.shown), "a name that cannot be a group is caught in the page");
  p.el("groupName").value = "Legal";
  button(card, "Make group").click();
  await settle();
  const made = p.sent.find((s) => s.key === "POST /orgs/acme/groups");
  check(made && made.body.name === "legal" && made.headers["idempotency-key"], "Make group sends the lower-cased name");
  const ops = card.querySelectorAll(".rw").find((r) => r.querySelector("b").textContent === "ops");
  const sel = ops.querySelector("select");
  check(sel && sel.all((n) => n.tagName === "OPTION").map((o) => o.value).includes("github:carol"), "each group offers the members who are not in it");
  sel.value = "github:carol";
  button(ops, "Add").click();
  await settle();
  check(p.sent.some((s) => s.key === "PUT /orgs/acme/groups/ops/members/github%3Acarol" && s.headers["idempotency-key"]), "Add puts that member in the group");
  const fin = card.querySelectorAll(".rw").find((r) => r.querySelector("b").textContent === "finance");
  fin.all((n) => n.tagName === "BUTTON" && n.attrs["aria-label"] === "Take @dan out of finance")[0].click();
  await settle();
  check(p.sent.some((s) => s.key === "DELETE /orgs/acme/groups/finance/members/github%3Adan"), "× takes them out");

  const carol = await page("https://krate.tech/org/?o=acme&tab=people", DASH, { token: "t", routes: as("github:carol") });
  const cc = carol.el("groupsCard");
  check(/finance/.test(cc.shown) && !carol.el("pane").querySelectorAll(".gform").length && !cc.all((n) => n.tagName === "BUTTON").length, "a developer reads the groups and changes nothing");
  const dan = await page("https://krate.tech/org/?o=acme&tab=help", DASH, { token: "t", routes: as("github:dan") });
  check(/groups of Acme you are in/.test(dan.el("groupsCard").shown) && !/ops/.test(dan.el("groupsCard").shown), "a recipient sees only the groups they are in");
  const old = await page("https://krate.tech/org/?o=acme&tab=people", DASH, { token: "t", routes: { ...as(ALICE), "GET /orgs/acme/groups": () => [404, { error: "hub-error", message: "" }] } });
  check(!old.el("pane").querySelectorAll("#groupsCard").length, "a hub without groups shows no Groups card");
}

/* ---- words ------------------------------------------------------------- */
{
  const p = await page("https://krate.tech/orgs/", "void 0");
  const K = p.win.KrateOrg;
  check(K.MATRIX["release.withdraw"].reviewer === "yes" && !K.MATRIX["release.withdraw"].developer && !K.MATRIX["release.set-current"].reviewer && K.MATRIX["group.manage"].admin === "yes" && K.MATRIX["release.audience"].owner === "yes", "the page's matrix has section R's rows");
  for (const r of ["release-withdrawn", "not-in-audience", "reason-required", "no-such-group", "group-taken", "group-invalid"]) {
    const w = K.refusal(r, { org: "Acme", slug: "acme" });
    const all = w.title + " " + w.body;
    check(w.title && w.body && !all.includes(r) && !/[—–`]|--/.test(all), `"${r}" is words: ${w.title}`);
  }
  check(K.refusal("not-in-audience", { org: "Acme", groups: ["finance", "ops"] }).title === "This app is only for the finance and ops groups in Acme", "outside the audience names the groups");
  const names = { [D1]: "Bounce 1.0.1", [D2]: "Bounce 1.0.2" };
  const say = (e) => K.eventWords({ actor: "github:bob", result: "allowed", ...e }, names);
  check(say({ action: "release.withdraw", target: `release:${D2}`, reason: "crashes on open" }) === "@bob withdrew Bounce 1.0.2: “crashes on open”.", "a withdrawal reads with its reason");
  check(say({ action: "release.current", target: `app:${APP}`, reason: D1 }) === `@bob made Bounce 1.0.1 the version of ${APP} that opens.`, "a rollback reads plainly");
  check(say({ action: "release.current", target: `app:${APP}`, reason: `${D1} after withdrawing ${D2}` }) === "Bounce 1.0.1 became current, after @bob withdrew Bounce 1.0.2.", "and so does what became current after a withdrawal");
  check(say({ action: "release.current", target: `app:${APP}`, reason: `none after withdrawing ${D2}` }) === `Nothing of ${APP} opens any more, after @bob withdrew Bounce 1.0.2.`, "or that nothing does");
  check(say({ action: "release.audience", target: `release:${D2}`, reason: "groups:finance,ops" }) === "@bob limited Bounce 1.0.2 to the finance and ops groups.", "an audience reads as groups");
  check(say({ action: "group.add", target: "group:finance/github:dan" }) === "@bob added @dan to finance.", "a group change reads plainly");
  check(say({ action: "group.remove", target: "group:finance/github:dan", reason: "member removed" }) === "@dan left finance, on being removed from the organisation.", "and a removal's group exits too");
  check(K.eventWords({ actor: "github:carol", action: "release.download", target: `release:${D2}`, result: "denied", reason: "not-in-audience" }, names) === "@carol tried to open Bounce 1.0.2: it is not for their groups.", "a refused open outside the audience reads plainly");
}

/* ---- the private /open/ page ------------------------------------------- */
{
  const at = OPEN.indexOf("var hash;");
  const code = OPEN.slice(OPEN.lastIndexOf("(function () {", at), OPEN.indexOf("\n})();", OPEN.indexOf("function privateApp", at)) + "\n})();".length);
  const url = `https://krate.tech/open/?a=${D2}&private=1`;
  const gone = await page(url, code, { token: "tok", routes: { [`GET /a/${D2}`]: () => [410, { error: "release-withdrawn", message: "", withdrawn: { at: 1760100000, reason: "crashes on open" } }] } });
  check(/This version was withdrawn/.test(gone.el("taName").textContent) && /“crashes on open”/.test(gone.el("taSize").textContent) && /no longer opens/.test(gone.el("pageHead").textContent), "a withdrawn app says so on the open page, with the reason");
  const out = await page(url, code, { token: "tok", routes: { [`GET /a/${D2}`]: () => [403, { error: "not-in-audience", message: "", groups: ["finance"] }] } });
  check(/only for the finance group/.test(out.el("taName").textContent) && !/not-in-audience/.test(out.el("theirApp").shown), "an app for other groups names them, in words");
}

const bad = failures();
console.log(bad ? `${bad} check(s) failed` : "ok  release control and groups on the dashboard behave as section R says");
process.exit(bad ? 1 : 0);
