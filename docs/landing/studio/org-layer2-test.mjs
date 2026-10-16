// The organisation pages' second layer (IC-947, IC-948; CONTRACT.md
// Addenda 3 and 4), run from the shipped scripts against a stand-in hub in
// the contract's shapes: invitation mail, replacing the root key, closing
// and its window, export, the suspended and recovery banners, billing,
// company sign-in, offline access, and the /login?org= door.
//
//   node docs/landing/studio/org-layer2-test.mjs
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { webcrypto } from "node:crypto";
import { DASH, OPEN, check, page, settle, ACME, member, DIGEST, orgRoutes, tabNames, button, failures } from "./org-test-kit.mjs";

const ALICE = "github:alice";
const KEY2 = "ab".repeat(32);
const t = (s) => String(s || "").replace(/\s+/g, " ");
const withOrg = (me, org, members) => ({
  "GET /orgs/acme": () => [200, { org: { ...ACME, offline_default: "days-7", state: "active", close: null, suspended: false, billing_state: "pilot", recovery: null, ...org }, me: member(me, (members || []).find((m) => m.principal === me)?.role || "owner"), members: members || [member(ALICE, "owner")] }],
});
const two = [member(ALICE, "owner"), member("github:olga", "owner"), member("github:bob", "reviewer")];

/* ---- A. invitation mail ------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=people", DASH, {
    token: "t",
    routes: orgRoutes(ALICE, {
      "POST /orgs/acme/invites": (_h, b) => [201, b.email
        ? { code: "c1", expires_at: 1760604800, join_url: "https://krate.tech/join/?o=acme&c=c1", emailed: true }
        : { code: "c2", expires_at: 1760604800, join_url: "https://krate.tech/join/?o=acme&c=c2", emailed: false, email_problem: "GitHub invitations are not mailed" }],
    }),
  });
  p.el("invWho").value = "bob";
  button(p.el("pane"), "Create invitation").click();
  await settle();
  check(/Copy this link and send it/.test(p.el("invOut").shown) && /GitHub invitations are not mailed/.test(p.el("invOut").shown), "a GitHub invitation says to copy the link, and why it was not mailed");
  button(p.el("pane"), "Work email").click();
  p.el("invWho").value = "sam@acme.test";
  button(p.el("pane"), "Create invitation").click();
  await settle();
  check(/Emailed to sam@acme\.test/.test(p.el("invOut").shown), "an emailed invitation says where it went");
  const failed = await page("https://krate.tech/org/?o=acme&tab=people", DASH, {
    token: "t",
    routes: orgRoutes(ALICE, { "POST /orgs/acme/invites": () => [201, { code: "c3", expires_at: 1760604800, join_url: "x", emailed: false, email_problem: "This organisation has sent its 20 invitation mails for today" }] }),
  });
  button(failed.el("pane"), "Work email").click();
  failed.el("invWho").value = "sam@acme.test";
  button(failed.el("pane"), "Create invitation").click();
  await settle();
  check(/Copy this link and send it/.test(failed.el("invOut").shown) && /20 invitation mails for today/.test(failed.el("invOut").shown), "a mail that could not go says why, and the link is still there");
}

/* ---- C. replace the root key ------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: { ...orgRoutes(ALICE), ...withOrg(ALICE), "GET /orgs/acme/sso": () => [200, null], "GET /orgs/acme/exports": () => [200, []], "PUT /orgs/acme/root-key": (_h, b) => [200, { key: { public_key: b.public_key } }] },
  });
  button(p.el("pane"), "Replace root key").click();
  p.el("newKey").value = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEI...\n-----END PRIVATE KEY-----";
  button(p.el("pane"), "Replace").click();
  check(/private key/.test(p.el("pane").shown) && p.sent.every((s) => s.key !== "PUT /orgs/acme/root-key"), "a private key is refused in the page and never sent");
  p.el("newKey").value = KEY2.toUpperCase();
  button(p.el("pane"), "Replace").click();
  check(p.el("sheetDigest").shown.includes("abab abab abab abab"), "the confirmation shows the new key's fingerprint");
  p.el("sheetYes").onclick();
  await settle();
  const put = p.sent.find((s) => s.key === "PUT /orgs/acme/root-key");
  check(put && put.body.public_key === KEY2 && Object.keys(put.body).join() === "public_key", "only the 64-hex public key is sent, lower-cased");
}

/* ---- D. closing, its window, cancel, export ---------------------------- */
{
  const closeRoutes = (extra, org, members) => ({ ...orgRoutes(ALICE), ...withOrg(ALICE, org, members), "GET /orgs/acme/sso": () => [200, null], "GET /orgs/acme/exports": () => [200, []], ...extra });
  const sole = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: closeRoutes({ "POST /orgs/acme/close": (_h, b) => (b.confirm_slug === "acme" ? [200, { close: { id: "k1", state: "scheduled" } }] : [400, { error: "close-confirm-mismatch", message: "" }]) }),
  });
  button(sole.el("pane"), "Close Acme").click();
  check(sole.el("sheetIn").hidden === false, "a sole owner is asked to type the short name");
  sole.el("sheetInput").value = "acmee";
  sole.el("sheetYes").onclick();
  await settle();
  check(/not the short name/.test(sole.el("sheetWhy").shown), "a mistyped name closes nothing, and says so");
  const sole2 = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: closeRoutes({ "POST /orgs/acme/close": (_h, b) => [200, { close: { id: "k1", state: "scheduled", body: b } }] }),
  });
  button(sole2.el("pane"), "Close Acme").click();
  sole2.el("sheetInput").value = " acme ";
  sole2.el("sheetYes").onclick();
  await settle();
  check(sole2.sent.find((s) => s.key === "POST /orgs/acme/close").body.confirm_slug === "acme", "the typed name goes as confirm_slug");

  const many = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: closeRoutes({ "POST /orgs/acme/close": () => [200, { close: { id: "k2" } }] }, {}, two) });
  button(many.el("pane"), "Close Acme").click();
  check(many.el("sheetIn").hidden === true && /second owner/.test(many.el("sheetP").textContent), "with two owners nobody types: a second owner approves");
  many.el("sheetYes").onclick();
  await settle();
  check(JSON.stringify(many.sent.find((s) => s.key === "POST /orgs/acme/close").body) === "{}", "and the request carries nothing to type");

  const waiting = { state: "closing", close: { id: "k2", state: "waiting-for-second-owner", requested_by: "github:olga", requested_at: 1760000000, approved_by: null, statement: "Acme closes. Apps open for 30 days.", statement_digest: "d".repeat(64), ends_at: null } };
  const olga2 = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: closeRoutes({ "POST /orgs/acme/close/k2/approve": (_h, b) => [200, { close: { id: "k2", state: "scheduled", body: b } }], "DELETE /orgs/acme/close/k2": () => [204, null] }, waiting, two),
  });
  check(/waits for a second owner/.test(olga2.el("banner").shown), "the banner says the closing waits for a second owner");
  check(/Acme closes\. Apps open for 30 days\./.test(olga2.el("pane").shown), "Settings shows the statement being approved");
  button(olga2.el("pane"), "Approve the closing").click();
  check(olga2.el("sheetDigest").shown.includes("d".repeat(64)), "approving shows the statement's digest");
  olga2.el("sheetYes").onclick();
  await settle();
  check(olga2.sent.find((s) => s.key === "POST /orgs/acme/close/k2/approve").body.statement_digest === "d".repeat(64), "and approves exactly that digest");
  const asker = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: closeRoutes({}, { ...waiting, close: { ...waiting.close, requested_by: ALICE } }, two) });
  check(!button(asker.el("pane"), "Approve the closing") && /another owner approves it/.test(asker.el("pane").shown), "whoever asked is not offered the approval");
  button(asker.el("pane"), "Cancel the closing").click();
  check(/Nothing closes/.test(asker.el("sheetP").textContent), "cancelling asks first");

  // The hub keeps the organisation active while a close waits for its
  // second owner, and the statement is an object with an effect sentence.
  const active = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: closeRoutes({}, { state: "active", close: { ...waiting.close, statement: { org: "acme", window_days: 30, effect: "During the 30-day window members keep opening releases." } } }, two),
  });
  check(/waits for a second owner/.test(active.el("banner").shown) && /During the 30-day window/.test(active.el("pane").shown), "a close waiting on an active organisation still shows, with the statement's words");
  const shut = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, {
    token: "t", routes: closeRoutes({ "GET /orgs/acme/candidates": () => [200, [{ digest: "a".repeat(64), name: "Bounce", version: "1.0.1", state: "waiting-for-approval", uploaded_by: "github:carol", uploaded_at: 1760000400 }]] }, { state: "closing", close: { id: "k3", state: "scheduled", ends_at: 1762592000, requested_by: ALICE, requested_at: 1760000000 } }),
  });
  check(!button(shut.el("pane"), "Approve") && /Approvals stop while Acme is closing/.test(shut.el("pane").shown), "while closing, Approve is not offered and the page says why");
  const sched = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: closeRoutes({}, { state: "closing", close: { id: "k3", state: "scheduled", ends_at: 1762592000, requested_by: ALICE, requested_at: 1760000000 } }) });
  check(/Acme closes on/.test(sched.el("banner").shown) && /still open/.test(sched.el("banner").shown), "a scheduled closing names its date, and that apps still open");

  const closed = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t", routes: closeRoutes({ "GET /orgs/acme/exports": () => [200, [{ id: "x1", state: "ready", created_at: 1760000000, expires_at: 1762592000, size: 204800 }]] }, { state: "closed", closed_at: 1762592000 }),
  });
  check(tabNames(closed) === "settings", "after closing, former owners keep only Settings");
  check(/Acme closed on/.test(closed.el("banner").shown), "and the banner says it closed");
  check(/Ready/.test(closed.el("pane").shown) && button(closed.el("pane"), "Download") && !button(closed.el("pane"), "Close Acme"), "the export is there to download, and nothing else");

  const ex = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: closeRoutes({ "POST /orgs/acme/export": () => [202, { export: { id: "x2", state: "building", created_at: 1760000000 } }], "GET /orgs/acme/exports": () => [200, [{ id: "x2", state: "building", created_at: 1760000000 }]] }),
  });
  button(ex.el("pane"), "Build an export").click();
  await settle();
  check(ex.sent.some((s) => s.key === "POST /orgs/acme/export" && s.headers["idempotency-key"]) && /Building/.test(ex.el("pane").shown), "Build an export asks the hub, and the list shows it building");
}

/* ---- D. suspended and recovery banners --------------------------------- */
{
  const susp = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: { ...orgRoutes(ALICE), ...withOrg(ALICE, { suspended: true, billing_state: "suspended" }) } });
  check(/paused for billing/.test(susp.el("banner").shown) && /already released still open/.test(susp.el("banner").shown), "a suspended organisation says its released apps still open");
  const rec = { id: "r1", for: "github:carol", by: "github:krate-admin", reason: "the only owner lost access to their account", requested_at: 1760000000, effective_at: 1760259200, state: "waiting", not_mailed: ["github:olga"] };
  const ownerView = await page("https://krate.tech/org/?o=acme", DASH, {
    token: "t", routes: { ...orgRoutes(ALICE), ...withOrg(ALICE, { recovery: rec }), "POST /orgs/acme/recover/r1/object": () => [200, { recovery: { ...rec, state: "cancelled" } }] },
  });
  const b = ownerView.el("banner").shown;
  check(/Not mailed: @olga/.test(b) && /restoring ownership/.test(b) && /@krate-admin/.test(b) && /@carol/.test(b) && /lost access/.test(b) && /takes effect on/.test(b), "the recovery banner says who asked, for whom, why and when");
  button(ownerView.el("banner"), "Object").click();
  ownerView.el("sheetYes").onclick();
  await settle();
  check(ownerView.sent.some((s) => s.key === "POST /orgs/acme/recover/r1/object"), "Object sends the objection");
  const dev = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: { ...orgRoutes("github:carol"), ...withOrg("github:carol", { recovery: rec }, [member(ALICE, "owner"), member("github:carol", "developer")]) } });
  check(!/restoring ownership/.test(dev.el("banner").shown), "a developer is not shown the recovery");
}

/* ---- E. billing ---------------------------------------------------------- */
{
  const bill = (body, me = ALICE, role = "owner") => page("https://krate.tech/org/?o=acme&tab=billing", DASH, {
    token: "t", routes: { ...orgRoutes(me), ...withOrg(me, {}, [member(me, role)]), "GET /orgs/acme/billing": () => [200, body], "POST /orgs/acme/billing/checkout": () => [200, { url: "https://checkout.stripe.com/c/pay/cs_test_1" }] },
  });
  const closedB = await bill({ plan: "pilot", state: "pilot", seats_used: 4, seats_paid: null, period_end: null, checkout_open: false, manual: null, invoices: [] });
  check(/Krate invoices pilot organisations directly: write to yashraj@krate\.tech/.test(t(closedB.el("pane").shown)) && !button(closedB.el("pane"), "Pay by card for 4 seats"), "billing not open: the invoicing line, no checkout");
  check(!/[$€£]|USD|EUR/.test(closedB.el("pane").shown), "no price appears that the hub did not send");
  const open = await bill({ plan: "pilot", state: "pilot", seats_used: 4, seats_paid: null, checkout_open: true, manual: null, invoices: [{ id: "i1", at: 1760000000, status: "paid", number: "ACME-0001" }] });
  const pay = button(open.el("pane"), "Pay by card for 4 seats");
  check(!!pay && /ACME-0001/.test(open.el("pane").shown) && !/[$€£]/.test(open.el("pane").shown), "billing open: checkout for the seats in use, and an invoice with no amount shows none");
  pay.click();
  await settle();
  check(open.sent.some((s) => s.key === "POST /orgs/acme/billing/checkout" && s.headers["idempotency-key"]) && open.location.href === "https://checkout.stripe.com/c/pay/cs_test_1", "Pay opens Stripe's checkout, with an Idempotency-Key");
  const admin = await bill({ plan: "invoiced", state: "invoiced", seats_used: 4, seats_paid: 10, period_end: 1767225600, checkout_open: true, manual: { reference: "INV-0042", paid_until: 1767225600 }, invoices: [{ id: "i2", at: 1760000000, status: "paid", amount: 12000, currency: "usd" }] }, "github:ada", "admin");
  check(!button(admin.el("pane"), "Pay by card for 4 seats") && /INV-0042/.test(admin.el("pane").shown) && /4 used · 10 paid/.test(admin.el("pane").shown), "an admin reads billing but is not offered checkout");
  check(/\$120\.00/.test(admin.el("pane").shown), "an amount Stripe sent is shown in its currency");
  const dev = await page("https://krate.tech/org/?o=acme&tab=billing", DASH, { token: "t", routes: { ...orgRoutes("github:carol"), ...withOrg("github:carol", {}, [member("github:carol", "developer")]) } });
  check(!tabNames(dev).includes("billing"), "developers have no Billing tab");
}

/* ---- F. company sign-in -------------------------------------------------- */
{
  const sso = (cur, extra = {}) => page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: { ...orgRoutes(ALICE), ...withOrg(ALICE), "GET /orgs/acme/exports": () => [200, []], "GET /orgs/acme/sso": () => [200, cur], "PUT /orgs/acme/sso": (_h, b) => [200, { ...b, has_secret: true, domain_verified: false, client_secret: undefined }], "GET /orgs/acme/sso/domain-token": () => [200, { name: "_krate-verify.acme.test", value: "krate-verify=abc123" }], ...extra },
  });
  const fresh = await sso(null);
  const card = fresh.el("ssoCard").shown;
  check(/Google Workspace/.test(card) && /Okta/.test(card) && /Microsoft Entra/.test(card) && /https:\/\/login\.microsoftonline\.com\/<tenant-id>\/v2\.0/.test(card), "the three providers are named with their issuer shapes");
  const roles = fresh.el("ssoJoin").all((n) => n.tagName === "OPTION").map((o) => o.value);
  check(roles.join() === ",developer,reviewer,recipient", "auto-join offers nobody, developer, reviewer or recipient: never owner or admin");
  check(fresh.el("ssoEnforce").disabled === true, "enforcing waits for a verified domain");
  fresh.el("ssoIssuer").value = "https://accounts.google.com";
  fresh.el("ssoClient").value = "client-1";
  fresh.el("ssoDomain").value = "acme.test";
  button(fresh.el("ssoCard"), "Turn on company sign-in").click();
  await settle();
  check(fresh.sent.every((s) => s.key !== "PUT /orgs/acme/sso") && /client secret is missing/.test(fresh.el("ssoCard").shown), "a first setup without the secret is caught in the page");
  fresh.el("ssoSecret").value = "s3cret";
  button(fresh.el("ssoCard"), "Turn on company sign-in").click();
  await settle();
  const put = fresh.sent.find((s) => s.key === "PUT /orgs/acme/sso");
  check(put && put.body.client_secret === "s3cret" && put.body.auto_join_role === null && put.body.enforce === false, "the first save sends the secret, with nobody auto-joining");
  check(/_krate-verify\.acme\.test/.test(fresh.el("ssoCard").shown) && /krate-verify=abc123/.test(fresh.el("ssoCard").shown), "then the TXT record to add is shown");

  const set = await sso({ issuer: "https://acme.okta.com", client_id: "c", has_secret: true, domain: "acme.test", domain_verified: true, verified_at: 1760000000, auto_join_role: "recipient", enforce: false });
  check(set.el("ssoEnforce").disabled === false && /Verified/.test(set.el("ssoCard").shown), "a verified domain can be enforced");
  button(set.el("ssoCard"), "Save changes").click();
  await settle();
  const upd = set.sent.find((s) => s.key === "PUT /orgs/acme/sso");
  check(upd && !("client_secret" in upd.body) && upd.body.auto_join_role === "recipient", "saving without a new secret keeps the stored one");
  check(/\/login\/\?org=acme/.test(set.el("ssoCard").shown), "and the company sign-in link to share is shown");

  const unv = await sso({ issuer: "https://acme.okta.com", client_id: "c", has_secret: true, domain: "acme.test", domain_verified: false, auto_join_role: null, enforce: false }, { "POST /orgs/acme/sso/verify-domain": () => [422, { error: "domain-not-verified", message: "no TXT record at _krate-verify.acme.test yet" }] });
  button(unv.el("ssoCard"), "Verify").click();
  await settle();
  check(/not verified yet/.test(unv.el("ssoCard").shown) && /TXT record is not there yet/.test(unv.el("ssoCard").shown) && !/ -- /.test(unv.el("ssoCard").shown), "a domain that is not there yet is said in words");

  const req = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: { "GET /orgs/acme": () => [403, { error: "sso-required", message: "" }] } });
  const a = req.el("stop").all((n) => n.tagName === "A")[0];
  check(/requires company sign-in/.test(req.el("stop").shown) && a && a.href.startsWith("/login/?org=acme&next="), "sso-required offers the company sign-in, coming back here");
}

/* ---- F. the rest of company sign-in ------------------------------------ */
{
  const cur = { issuer: "https://acme.okta.com", client_id: "c", has_secret: true, domain: "acme.test", domain_verified: true, verified_at: 1760000000, auto_join_role: null, enforce: false };
  const p = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
    token: "t", routes: { ...orgRoutes(ALICE), ...withOrg(ALICE), "GET /orgs/acme/exports": () => [200, []], "GET /orgs/acme/sso": () => [200, cur],
      "PUT /orgs/acme/sso": () => [409, { error: "sso-needs-break-glass", message: "" }], "DELETE /orgs/acme/sso": () => [204, null] },
  });
  check(/tenant must issue verified email claims/.test(p.el("ssoCard").shown), "the Entra hint says its tenant must issue verified email claims");
  p.el("ssoEnforce").checked = true;
  button(p.el("ssoCard"), "Save changes").click();
  await settle();
  check(/Keep a way in that does not need company sign-in/.test(p.el("ssoCard").shown) && /at least one owner must sign in another way/.test(p.el("ssoCard").shown), "enforcing without a way back in is explained, with what to do");
  button(p.el("ssoCard"), "Turn off company sign-in").click();
  check(/Turn off company sign-in/.test(p.el("sheetH").textContent) && p.sent.every((x) => x.key !== "DELETE /orgs/acme/sso"), "turning it off asks first");
  p.el("sheetYes").onclick();
  await settle();
  check(p.sent.some((x) => x.key === "DELETE /orgs/acme/sso" && x.headers["idempotency-key"]), "and then sends DELETE /orgs/acme/sso");
  const off = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: { ...orgRoutes(ALICE), ...withOrg(ALICE), "GET /orgs/acme/exports": () => [200, []], "GET /orgs/acme/sso": () => [503, { error: "sso-off", message: "" }] } });
  check(/Company sign-in is not switched on on this hub yet/.test(off.el("ssoCard").shown), "a hub without the key says company sign-in is not switched on yet");

  // What a finished company sign-in did, on the page it returns to.
  const K = off.win.KrateOrg;
  const words = ["joined", "member", "removed", "no-auto-join", "off-domain"].map((s) => K.ssoNotice(s, "Acme"));
  check(/joined Acme/.test(words[0]) && /signed in through Acme/.test(words[1]) && /removed from Acme/.test(words[2]) && /ask one of its owners for an invitation/.test(words[3]) && /not at Acme's verified domain/.test(words[4]), "each company sign-in result reads in words");
  const back = await page("https://krate.tech/org/?o=acme", DASH, { token: "t", routes: orgRoutes(ALICE) });
  back.localStorage.setItem("krate_sso_notice", JSON.stringify({ org: "acme", state: "no-auto-join" }));
  check(/does not let people join by themselves/.test(back.win.KrateOrg.takeSsoNotice(back.localStorage, "Acme")) && back.localStorage.getItem("krate_sso_notice") === null, "the result is shown once, then forgotten");
  check(/not switched on on this hub yet/.test(K.refusal("leases-off").title), "offline access off on the hub says so");
}

/* ---- G. offline access ------------------------------------------------- */
{
  const routes = (me, role) => ({ ...orgRoutes(me), ...withOrg(me, {}, [member(me, role)]),
    "GET /orgs/acme/releases": () => [200, [{ name: "Bounce", version: "1.0.0", digest: DIGEST, url: "u", uploaded_by: "github:carol", approved_by: "github:bob", at: 1760000300, offline: "days-30" }]],
    "PUT /orgs/acme/apps/Bounce/offline": (_h, b) => [200, { name: "Bounce", class: b.class }],
    "PUT /orgs/acme/offline-default": (_h, b) => [200, { class: b.class }],
    "GET /orgs/acme/sso": () => [200, null], "GET /orgs/acme/exports": () => [200, []] });
  const own = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: routes(ALICE, "owner") });
  // The release row's own picker: Getting started sits above the rows with
  // a picker of its own.
  const sel = own.el("pane").all((n) => n.tagName === "SELECT" && n.all((o) => o.tagName === "OPTION").some((o) => o.value === "days-7"))[0];
  check(sel && sel.value === "days-30" && sel.all((n) => n.tagName === "OPTION").map((o) => o.value).join() === "online,days-7,days-30", "an owner sets each release's offline class: online, 7 days, 30 days, nothing longer");
  sel.value = "online";
  sel.dispatch("change");
  await settle();
  check(own.sent.some((s) => s.key === "PUT /orgs/acme/apps/Bounce/offline" && s.body.class === "online"), "and the choice is sent for that app's name");
  const rec = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: routes("github:bob", "reviewer") });
  check(!rec.el("pane").querySelector("select") && /30 days offline/.test(rec.el("pane").shown), "everyone else reads the class in words");
  const set = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "t", routes: routes(ALICE, "owner") });
  const def = set.el("pane").all((n) => n.tagName === "SELECT" && /new apps/i.test(n.attrs["aria-label"] || ""))[0];
  def.value = "days-30";
  def.dispatch("change");
  await settle();
  check(set.sent.some((s) => s.key === "PUT /orgs/acme/offline-default" && s.body.class === "days-30"), "an owner sets the organisation's default");

  // The open page: the exact terms before the first open.
  const at = OPEN.indexOf("var hash;");
  const code = OPEN.slice(OPEN.lastIndexOf("(function () {", at), OPEN.indexOf("\n})();", OPEN.indexOf("function privateApp", at)) + "\n})();".length);
  const until = Date.UTC(2026, 9, 21, 12) / 1000;
  const lease = await page(`https://krate.tech/open/?a=${DIGEST}&private=1`, code, {
    token: "tok", routes: { [`GET /a/${DIGEST}`]: () => [200, { name: "Bounce", version: "1.0.0", size: 13778, org: { slug: "acme", name: "Acme" }, offline: { class: "days-7", until } }] },
  });
  check(t(lease.el("offlineNote").textContent) === "Works offline until 21 Oct 2026, then needs a connection. If you are removed, it stops when it next connects.", "the open page gives the exact offline date before the first open");
  const gone = await page(`https://krate.tech/open/?a=${DIGEST}&private=1`, code, { token: "tok", routes: { [`GET /a/${DIGEST}`]: () => [403, { error: "org-closed", message: "acme is closed" }] } });
  check(/organisation that sent it has closed/.test(gone.el("taName").textContent), "an app from a closed organisation says so in words");
  const online = await page(`https://krate.tech/open/?a=${DIGEST}&private=1`, code, {
    token: "tok", routes: { [`GET /a/${DIGEST}`]: () => [200, { name: "Bounce", size: 1, org: { slug: "acme", name: "Acme" }, offline: { class: "online", until: null } }] },
  });
  check(/Needs a connection every time it opens/.test(online.el("offlineNote").textContent), "an online-only app says it needs a connection each time");
}

/* ---- F3. /login?org= ---------------------------------------------------- */
{
  const signin = readFileSync("docs/landing/login/signin.js", "utf8");
  const html = readFileSync("docs/landing/login/index.html", "utf8");
  const blocks = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  const script = blocks[blocks.length - 1];
  function visit(url) {
    const u = new URL(url);
    const els = {};
    const el = () => { const c = new Set(["hidden"]); return { textContent: "", href: "", className: "", firstChild: { textContent: "" }, classList: { add: (x) => c.add(x), remove: (x) => c.delete(x), contains: (x) => c.has(x) }, addEventListener() {}, focus() {}, get shown() { return !c.has("hidden"); } }; };
    const m = new Map();
    const ctx = {
      localStorage: { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)), removeItem: (k) => m.delete(k) },
      location: { search: u.search, hash: u.hash, pathname: u.pathname, origin: u.origin, hostname: u.hostname, href: u.href, replace() {} },
      URLSearchParams, URL, Date, JSON, crypto: webcrypto, Uint8Array, setTimeout, addEventListener() {},
      history: { replaceState() {} },
      document: { title: "", documentElement: { className: "", classList: { contains: () => false, add() {}, remove() {} } }, getElementById: (id) => (els[id] ||= el()), querySelector: () => el(), body: { insertAdjacentHTML() {} } },
      fetch: () => new Promise(() => {}),
    };
    ctx.window = ctx;
    vm.createContext(ctx);
    vm.runInContext(signin, ctx);
    vm.runInContext(script, ctx);
    return els;
  }
  const page1 = visit("https://krate.tech/login/?org=acme&next=%2Forg%2F%3Fo%3Dacme");
  const door = new URL(page1.orgBtn.href);
  check(page1.orgBtn.shown && door.origin === "https://hub.krate.tech" && door.pathname === "/login/oidc/start" && door.searchParams.get("org") === "acme" && /^[0-9a-f]{32}$/.test(door.searchParams.get("nonce") || ""), "/login?org= offers the company sign-in, with this browser's nonce");
  check(/Sign in to acme/.test(page1.loginH.textContent), "and says which organisation");
  const bad = visit("https://krate.tech/login/?org=acme&error=sso-wrong-domain");
  check(/not at acme's company domain/.test(bad.loginSub.textContent) && !/sso-wrong-domain/.test(bad.loginSub.textContent), "a company sign-in that failed says why in words");
  const nodomain = visit("https://krate.tech/login/?org=acme&error=domain-not-verified");
  check(/has not proved its company domain yet/.test(nodomain.loginSub.textContent), "a company sign-in refused for an unproved domain says so");
  const none = visit("https://krate.tech/login/?org=..%2Fevil");
  check(!(none.orgBtn && none.orgBtn.shown),"a made-up org value offers no company button");
}

const bad = failures();
console.log(bad ? `${bad} check(s) failed` : "ok  the organisation pages' second layer behaves as Addenda 3 and 4 say");
process.exit(bad ? 1 : 0);
