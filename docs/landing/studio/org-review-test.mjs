// What an approver sees on the Waiting tab (IC-950), run from the shipped
// scripts against a stand-in hub in the hub's shapes: what the app can do in
// plain words, what changed since the version that opens now, a tick for
// each new capability a person is asked for, Approve held until every one is
// ticked, and the ticks sent with the approval.
//
//   node docs/landing/studio/org-review-test.mjs
import { DASH, check, page, settle, ACME, member, button, failures, orgRoutes } from "./org-test-kit.mjs";

const ALICE = "github:alice";
const D1 = "1".repeat(64);
const D2 = "2".repeat(64);
const D3 = "3".repeat(64);
const t = (s) => String(s || "").replace(/\s+/g, " ");
const cap = (c, words, asks, extra = {}) => ({ cap: c, words, required: true, rationale: "", asks, ...extra });
const WINDOW = cap("ui.window:create", "open a window on your screen", false);
const NET = cap("net.connect:api.acme.example:443", "connect to api.acme.example:443 over the network", true, { rationale: "send expenses to finance" });
const CAMERA = cap("camera.capture", "use your camera", true, { required: false, rationale: "scan receipts" });
const cand = (digest, version, extra) => ({
  digest, name: "Expenses", version, state: "waiting-for-approval", uploaded_by: "github:carol", uploaded_at: 1760000300,
  approved_by: null, approved_at: null, app_id: "com.acme.expenses", audience: null, signer: "ab".repeat(32), ...extra,
});
const CANDIDATES = [
  // A first release: nothing to compare with.
  cand(D1, "1.0.0", { name: "Receipts", capabilities: [WINDOW, NET], changes: { against: null, added: [WINDOW, NET], removed: [], kept: 0 } }),
  // An update that adds the camera.
  cand(D2, "1.1.0", { capabilities: [WINDOW, NET, CAMERA], changes: { against: { digest: "9".repeat(64), version: "1.0.3" }, added: [CAMERA], removed: [], kept: 2 } }),
  // An update that changes nothing it asks for.
  cand(D3, "1.0.4", { name: "Timesheet", capabilities: [WINDOW], changes: { against: { digest: "8".repeat(64), version: "1.0.3" }, added: [], removed: [], kept: 1 } }),
];
const people = [member(ALICE, "owner"), member("github:bob", "reviewer"), member("github:carol", "developer")];
const as = (me, extra = {}) => ({
  ...orgRoutes(me),
  "GET /orgs/acme": () => [200, { org: { ...ACME, state: "active" }, me: people.find((m) => m.principal === me), members: people }],
  "GET /orgs/acme/candidates": () => [200, CANDIDATES],
  ...extra,
});
// A waiting version is a card of its own (.wcard); anything else a row.
const rowOf = (p, text) => p.el("pane").querySelectorAll(".wcard").concat(p.el("pane").querySelectorAll(".rw")).find((r) => r.querySelector("b") && r.querySelector("b").textContent.startsWith(text));
const ticksIn = (row) => row.all((n) => n.tagName === "INPUT" && n.getAttribute("type") === "checkbox");

{
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, {
    token: "t", routes: as("github:bob", { [`POST /orgs/acme/candidates/${D2}/approve`]: () => [200, { release: { digest: D2 } }] }),
  });
  const first = rowOf(p, "Receipts");
  check(/What it can do once someone allows it: .*connect to api\.acme\.example:443 over the network/.test(t(first.shown)), "a first release lists what it can do, in plain words");
  check(/Its maker says: send expenses to finance/.test(t(first.shown)), "with its maker's reason, shown as theirs");
  check(/And what every app may do: open a window on your screen/.test(t(first.shown)), "what every app may do is said apart, not mixed in");
  check(ticksIn(first).length === 0 && button(first, "Approve") && !button(first, "Approve").disabled, "a first release needs no ticks: approving it is approving that list");

  const same = rowOf(p, "Timesheet");
  check(/It asks for exactly the same as before\./.test(same.shown) && ticksIn(same).length === 0, "an update that asks for nothing new says so and needs no ticks");

  const upd = rowOf(p, "Expenses");
  check(/Compared with 1\.0\.3, the version that opens now:/.test(t(upd.shown)), "an update says what it is compared with");
  check(/New, check each before approving: .*use your camera .*\(optional\) .*Its maker says: scan receipts/.test(t(upd.shown)), "the new capability stands out, with optional and its reason");
  check(/Everything else is as before \(2 permissions\)\./.test(upd.shown), "and how much is unchanged");
  const ticks = ticksIn(upd);
  const approve = button(upd, "Approve");
  check(ticks.length === 1 && approve.disabled === true, "Approve waits until each new capability is ticked");
  ticks[0].checked = true;
  ticks[0].dispatch("change");
  check(approve.disabled === false, "ticking it enables Approve");
  approve.click();
  await settle();
  check(/New: it can use your camera/.test(p.el("sheetPts").shown), "the approval sheet repeats what is new");
  p.el("sheetYes").onclick();
  await settle();
  const sent = p.sent.find((s) => s.key === `POST /orgs/acme/candidates/${D2}/approve`);
  check(sent && JSON.stringify(sent.body.accept) === JSON.stringify(["camera.capture"]), "the ticks go with the approval, so the hub can check them");

}

{
  // A reviewer declines with a reason; an empty reason is not sent.
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, {
    token: "t", routes: as("github:bob", { [`POST /orgs/acme/candidates/${D2}/decline`]: (h, b) => [200, { digest: D2, state: "declined", decline_reason: b.reason }] }),
  });
  const upd = rowOf(p, "Expenses");
  button(upd, "Decline").click();
  const go = button(upd, "Decline it");
  go.click();
  await settle();
  check(!p.sent.some((s) => s.key.endsWith("/decline")) && /Say why in a few words/.test(upd.shown), "declining needs a reason");
  upd.all((n) => n.tagName === "TEXTAREA")[0].value = "Scan receipts without the camera first";
  go.click();
  await settle();
  const sent = p.sent.find((s) => s.key === `POST /orgs/acme/candidates/${D2}/decline`);
  check(sent && sent.body.reason === "Scan receipts without the camera first", "a reviewer declines with a reason the sender reads");
}

{
  // The person who sent it reads what changed on its app's page, and cannot tick or approve.
  const p = await page("https://krate.tech/org/?o=acme&tab=apps&app=com.acme.expenses", DASH, { token: "t", routes: as("github:carol") });
  const upd = rowOf(p, "Expenses");
  check(/use your camera/.test(upd.shown) && !button(upd, "Approve") && ticksIn(upd).every((x) => x.disabled), "the sender sees what changed, and nothing to tick or approve");
}

{
  // A hub from before IC-950 sends no capabilities: the row still works.
  const old = CANDIDATES.map((c) => { const { capabilities, changes, ...rest } = c; return rest; });
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: as("github:bob", { "GET /orgs/acme/candidates": () => [200, old] }) });
  const row = rowOf(p, "Expenses");
  check(row && button(row, "Approve") && !button(row, "Approve").disabled && /It asks for nothing beyond what every app may do\./.test(row.shown), "an older hub: no list, Approve as before");
}

if (failures()) process.exit(1);
console.log("ok  the Waiting tab shows what an app can do and what changed, and approval waits for each new capability");
