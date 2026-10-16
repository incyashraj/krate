// The organisation dashboard's IC-955 Settings cards, run from the shipped
// scripts against a stand-in hub in the hub's shapes: SAML sign-in,
// provisioning by SCIM, and the activity sent to the company's logs. Who
// sees each card follows the role matrix; a secret the hub shows once is
// shown once and kept nowhere; nothing is switched off or revoked without
// asking; every refusal is words; and Activity reads the new events.
//
//   node docs/landing/studio/org-doors-test.mjs
import { DASH, check, page, ACME, member, orgRoutes, button, failures } from "./org-test-kit.mjs";

const ALICE = "github:alice";
const t = (s) => String(s || "").replace(/\s+/g, " ");
// Is an element with this id really on the page? (The stand-in makes one on first ask by id.)
const has = (p, id) => p.el("pane").all((n) => n.attrs && n.attrs.id === id).length > 0;
const TOKEN = "kscim_" + "A".repeat(43);
const SECRET = "whsec_" + "B".repeat(43);
const HUBB = "https://hub.krate.tech";
const people = [member(ALICE, "owner"), member("github:erin", "admin"), member("github:bob", "reviewer"), member("github:carol", "developer")];
const org = (me) => ({
  "GET /orgs/acme": () => [200, { org: { ...ACME, offline_default: "days-7", state: "active", close: null, suspended: false, billing_state: "pilot", recovery: null }, me: people.find((m) => m.principal === me), members: people }],
  "GET /orgs/acme/exports": () => [200, []],
  "GET /orgs/acme/sso": () => [200, null],
});
const scimNone = { default_role: "recipient", tokens: [], people: 0, groups: 0, base_url: `${HUBB}/scim/v2/acme` };
const samlNone = { sp: { entity_id: `${HUBB}/saml/acme`, acs_url: `${HUBB}/login/saml/acs/acme`, metadata_url: `${HUBB}/saml/acme` }, domain: null, domain_verified: false, idp: null };
const delivery = (d = {}) => ({ pending: 0, oldest_pending_at: null, next_attempt_at: null, delivered: 0, given_up: 0, last_delivered_at: null, last_delivered_seq: null, last_error: null, last_error_at: null, last_status: null, ...d });
const exportOn = (d) => ({ url: "https://siem.acme.test/krate?key=k1", host: "siem.acme.test", has_secret: true, set_at: 1760000000, set_by: ALICE, delivery: delivery(d) });
const settings = (me, extra = {}) => page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
  token: "t",
  routes: { ...orgRoutes(me), ...org(me), "GET /orgs/acme/scim": () => [200, scimNone], "GET /orgs/acme/saml": () => [200, samlNone], "GET /orgs/acme/audit-export": () => [200, null], ...extra },
});

/* ---- who sees which card ------------------------------------------------ */
{
  const owner = await settings(ALICE);
  check(has(owner, "samlCard") && has(owner, "scimCard") && has(owner, "auditCard"), "an owner sees SAML, Provisioning and the audit export");
  const admin = await settings("github:erin", { "GET /orgs/acme/audit-export": () => [200, exportOn({ delivered: 3, last_delivered_at: 1760000500 })] });
  check(!has(admin, "samlCard") && !has(admin, "scimCard") && has(admin, "auditCard"), "an admin sees only the audit export");
  const card = admin.el("auditCard");
  check(/siem\.acme\.test/.test(card.shown) && /Delivered/.test(card.shown) && !has(admin, "auditUrl") && !button(card, "Send a test event") && !button(card, "Stop sending"), "and only watches it: no address, no test, no stop");
  check(!/key=k1/.test(card.shown), "the address's query (a receiver's key) is not shown, only the host");
  for (const who of ["github:bob", "github:carol"]) {
    const p = await settings(who);
    check(!has(p, "samlCard") && !has(p, "scimCard") && !has(p, "auditCard"), `${who.slice(7)} sees none of them`);
  }
  const old = await settings(ALICE, { "GET /orgs/acme/scim": () => [404, null], "GET /orgs/acme/saml": () => [404, null], "GET /orgs/acme/audit-export": () => [404, null] });
  check(/not on this hub yet/.test(old.el("scimCard").shown) && /not on this hub yet/.test(old.el("auditCard").shown) && /not on this hub yet/.test(old.el("samlCard").shown), "an older hub without the routes is said in words");
}

/* ---- SCIM ---------------------------------------------------------------- */
{
  const made = { id: "st_abcdefgh", created_at: 1760000600, settings: { ...scimNone, tokens: [{ id: "st_abcdefgh", hint: "kscim_AAAA...", created_at: 1760000600, created_by: ALICE, last_used_at: null }] }, token: TOKEN, base_url: `${HUBB}/scim/v2/acme` };
  const p = await settings(ALICE, {
    "POST /orgs/acme/scim/tokens": () => [201, made],
    "PUT /orgs/acme/scim": (_h, b) => [200, { ...scimNone, default_role: b.default_role }],
  });
  const card = p.el("scimCard");
  check(p.el("scimBase").textContent === `${HUBB}/scim/v2/acme`, "the base URL to give the identity provider is shown, with Copy");
  const roles = p.el("scimRole").all((n) => n.tagName === "OPTION").map((o) => o.value);
  check(JSON.stringify(roles) === JSON.stringify(["recipient", "developer", "reviewer"]) && p.el("scimRole").value === "recipient", "the directory's role is recipient, developer or reviewer -- never owner or admin -- and starts at recipient");
  p.el("scimRole").value = "developer";
  p.el("scimRole").dispatch("change");
  await new Promise((r) => setTimeout(r, 5));
  const put = p.sent.find((s) => s.key === "PUT /orgs/acme/scim");
  check(put && put.body.default_role === "developer" && put.headers["idempotency-key"], "choosing it sends PUT /orgs/acme/scim with an Idempotency-Key");
  check(/No token yet/.test(card.shown), "with no token it says what to do");
  button(card, "Make a SCIM token").click();
  await new Promise((r) => setTimeout(r, 5));
  check(p.el("scimOnce") && p.el("scimOnce").shown.includes(TOKEN) && /Shown once/.test(p.el("scimOnce").shown), "the new token is shown once, saying so, with where to paste it");
  check(/Okta: API Token\. Entra: Secret Token/.test(p.el("scimOnce").shown), "it names the field in Okta and in Entra");
  check(!JSON.stringify(p.localStorage.dump ? p.localStorage.dump() : Object.fromEntries(Object.keys(p.localStorage).map((k) => [k, p.localStorage[k]]))).includes(TOKEN), "the token is not kept in the browser");
  check(/kscim_AAAA\.\.\./.test(p.el("scimCard").shown) && /not used yet/.test(p.el("scimCard").shown), "the token list shows its hint and that it is not used yet");
  // Revoking asks first.
  const listed = await settings(ALICE, { "GET /orgs/acme/scim": () => [200, made.settings], "DELETE /orgs/acme/scim/tokens/st_abcdefgh": () => [204, null] });
  button(listed.el("scimCard"), "Revoke").click();
  check(/Revoke this SCIM token/.test(listed.el("sheetH").textContent) && listed.sent.every((s) => !s.key.startsWith("DELETE")), "Revoke asks first");
  listed.el("sheetYes").onclick();
  await new Promise((r) => setTimeout(r, 5));
  check(listed.sent.some((s) => s.key === "DELETE /orgs/acme/scim/tokens/st_abcdefgh" && s.headers["idempotency-key"]), "then revokes that exact token");
  // A refusal is words.
  const refused = await settings(ALICE, { "POST /orgs/acme/scim/tokens": () => [403, { error: "role-cannot", message: "acme already has 5 SCIM tokens -- revoke one you no longer use first" }] });
  button(refused.el("scimCard"), "Make a SCIM token").click();
  await new Promise((r) => setTimeout(r, 5));
  check(/Your role cannot make a SCIM token/.test(refused.el("scimCard").shown), "a refusal says what was tried, in words");
}

/* ---- audit export ------------------------------------------------------- */
{
  const p = await settings(ALICE, { "PUT /orgs/acme/audit-export": (_h, b) => [201, { ...exportOn(), url: b.url, secret: SECRET }] });
  const card = () => p.el("auditCard");
  check(/Send the activity to your logs/.test(card().shown) && button(card(), "Start sending"), "with nothing set it offers Start sending");
  p.el("auditUrl").value = "http://siem.acme.test/krate";
  button(card(), "Start sending").click();
  check(p.sent.every((s) => s.key !== "PUT /orgs/acme/audit-export") && /starts with https/.test(card().shown), "a plain-http address is caught in the page, with what to do");
  p.el("auditUrl").value = "https://siem.acme.test/krate?key=k1";
  button(card(), "Start sending").click();
  await new Promise((r) => setTimeout(r, 5));
  const put = p.sent.find((s) => s.key === "PUT /orgs/acme/audit-export");
  check(put && put.body.url === "https://siem.acme.test/krate?key=k1" && put.headers["idempotency-key"], "then it sends the address");
  check(p.el("auditOnce") && p.el("auditOnce").shown.includes(SECRET) && /X-Krate-Signature/.test(p.el("auditOnce").shown), "the signing secret is shown once, with the header it checks");
  check(/t=<unix>,v1=<hex HMAC-SHA256 of "<t>\.<body>">/.test(card().shown), "and exactly how the signature is made");

  const retrying = await settings(ALICE, {
    "GET /orgs/acme/audit-export": () => [200, exportOn({ pending: 3, next_attempt_at: 1760000900, last_error: "the receiver answered 500", last_error_at: 1760000800, delivered: 12, given_up: 1 })],
    "POST /orgs/acme/audit-export/test": () => [202, { queued: 99, export: exportOn({ pending: 4, last_error: "the receiver answered 500" }) }],
    "DELETE /orgs/acme/audit-export": () => [204, null],
    "POST /orgs/acme/audit-export/secret": () => [200, { ...exportOn(), secret: SECRET }],
  });
  const rc = retrying.el("auditCard");
  check(retrying.el("auditState").textContent === "Retrying" && /the receiver answered 500/.test(rc.shown) && /3, next try/.test(t(rc.shown)) && /1 \(refused for 24 hours, then given up\)/.test(rc.shown), "delivery state: retrying, the last problem, what waits, what was given up");
  button(rc, "Send a test event").click();
  await new Promise((r) => setTimeout(r, 5));
  check(retrying.sent.some((s) => s.key === "POST /orgs/acme/audit-export/test"), "Send a test event sends one");
  button(retrying.el("auditCard"), "Stop sending").click();
  check(/Stop sending the activity/.test(retrying.el("sheetH").textContent) && /events still waiting are dropped/.test(retrying.el("sheetP").textContent) && retrying.sent.every((s) => s.key !== "DELETE /orgs/acme/audit-export"), "Stop sending asks first and says what is dropped");
  retrying.el("sheetYes").onclick();
  await new Promise((r) => setTimeout(r, 5));
  check(retrying.sent.some((s) => s.key === "DELETE /orgs/acme/audit-export" && s.headers["idempotency-key"]), "then stops");
  const bad = await settings(ALICE, { "PUT /orgs/acme/audit-export": () => [400, { error: "bad-request", message: "that is a private or loopback address -- the hub sends from the internet, so give the receiver's public https URL" }] });
  bad.el("auditUrl").value = "https://10.0.0.8/x";
  button(bad.el("auditCard"), "Start sending").click();
  await new Promise((r) => setTimeout(r, 5));
  check(/private or loopback address\. the hub sends from the internet/.test(bad.el("auditCard").shown) && !/ -- /.test(bad.el("auditCard").shown), "the hub's refusal is shown in words, without its dashes");
}

/* ---- SAML ----------------------------------------------------------------- */
{
  const p = await settings(ALICE, { "PUT /orgs/acme/saml": (_h, b) => [200, { ...samlNone, domain: b.domain, idp: { entity_id: b.idp_entity_id, sso_url: b.idp_sso_url, certificates: [{ sha256: "ab".repeat(32), subject: "CN=x", not_after: 1900000000 }] } }] });
  check(p.el("samlAcs").textContent === `${HUBB}/login/saml/acs/acme` && p.el("samlEntity").textContent === `${HUBB}/saml/acme`, "the ACS URL and entity ID to give the identity provider are shown");
  p.el("samlIdpEntity").value = "http://www.okta.com/exk1";
  p.el("samlIdpUrl").value = "https://acme.okta.com/app/krate/sso/saml";
  p.el("samlDomain").value = "acme.test";
  button(p.el("samlCard"), "Turn on SAML sign-in").click();
  check(p.sent.every((s) => s.key !== "PUT /orgs/acme/saml") && /signing certificate is missing/.test(p.el("samlCard").shown), "a first set-up without the certificate is caught in the page");
  p.el("samlCert").value = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
  button(p.el("samlCard"), "Turn on SAML sign-in").click();
  await new Promise((r) => setTimeout(r, 5));
  const put = p.sent.find((s) => s.key === "PUT /orgs/acme/saml");
  check(put && put.body.idp_entity_id === "http://www.okta.com/exk1" && put.body.domain === "acme.test" && /BEGIN CERTIFICATE/.test(put.body.idp_cert) && put.headers["idempotency-key"], "then it sends the identity provider, certificate and domain");

  const on = { ...samlNone, domain: "acme.test", domain_verified: false, idp: { entity_id: "http://www.okta.com/exk1", sso_url: "https://acme.okta.com/sso", certificates: [{ sha256: "ab".repeat(32), subject: "CN=x", not_after: 1900000000 }] } };
  const set = await settings(ALICE, {
    "GET /orgs/acme/saml": () => [200, on],
    "GET /orgs/acme/sso": () => [200, { issuer: "", client_id: "", has_secret: false, domain: "acme.test", domain_verified: false, auto_join_role: null, enforce: false }],
    "GET /orgs/acme/sso/domain-token": () => [200, { name: "_krate-verify.acme.test", value: "krate-verify=abc123" }],
    "PUT /orgs/acme/sso": (_h, b) => [200, { issuer: "", client_id: "", has_secret: false, domain: b.domain, domain_verified: false, auto_join_role: b.auto_join_role, enforce: false }],
    "DELETE /orgs/acme/saml": () => [204, null],
  });
  const sc = set.el("samlCard");
  check(/Not verified/.test(sc.shown) && /Prove it under Company sign-in/.test(sc.shown) && (!has(set, "samlDomain") || set.el("samlDomain").disabled === true), "with a domain set, SAML says it waits for the domain proof, and the domain is not retyped");
  check(/sha256 abab abab abab abab/.test(t(sc.shown)), "the stored certificate shows by fingerprint, not its text");
  check(/\/login\/\?org=acme/.test(sc.shown), "and the sign-in link to share");
  // The company sign-in card holds the domain proof, joining and enforce, with no OIDC issuer needed.
  check(/_krate-verify\.acme\.test/.test(set.el("ssoCard").shown), "the domain proof is under Company sign-in");
  set.el("ssoJoin").value = "developer";
  button(set.el("ssoCard"), "Save changes").click();
  await new Promise((r) => setTimeout(r, 5));
  const ssoPut = set.sent.find((s) => s.key === "PUT /orgs/acme/sso");
  check(ssoPut && ssoPut.body.issuer === undefined && ssoPut.body.auto_join_role === "developer", "with SAML on, joining is saved without an OIDC issuer");
  button(set.el("samlCard"), "Turn off SAML sign-in").click();
  check(/Turn off SAML sign-in/.test(set.el("sheetH").textContent) && set.sent.every((s) => s.key !== "DELETE /orgs/acme/saml"), "turning SAML off asks first");
  set.el("sheetYes").onclick();
  await new Promise((r) => setTimeout(r, 5));
  check(set.sent.some((s) => s.key === "DELETE /orgs/acme/saml" && s.headers["idempotency-key"]), "then sends DELETE /orgs/acme/saml");
}

/* ---- Activity reads the new events -------------------------------------- */
{
  const p = await settings(ALICE);
  const K = p.win.KrateOrg;
  const say = (e) => K.eventWords({ actor: "scim", result: "allowed", ...e }, {});
  check(say({ action: "member.add", target: "member:email:erin@acme.test", reason: "recipient; created by the directory" }) === "Your directory (SCIM) added erin@acme.test as member.", "a person the directory adds");
  check(say({ action: "member.remove", target: "member:email:erin@acme.test", reason: "recipient; deactivated in the directory" }) === "Your directory (SCIM) removed erin@acme.test, deactivated in the directory.", "a person the directory deactivates");
  check(say({ action: "member.add", target: "member:email:erin@acme.test", reason: "developer; reactivated by the directory" }) === "Your directory (SCIM) brought back erin@acme.test as developer.", "a person the directory brings back");
  check(K.eventWords({ actor: ALICE, result: "allowed", action: "audit-export.set", target: "audit-export:siem.acme.test" }, {}) === "@alice started sending the activity to siem.acme.test.", "starting the export");
  check(K.eventWords({ actor: ALICE, result: "allowed", action: "scim.token.revoke", target: "scim-token:st_x" }, {}) === "@alice revoked a SCIM token. It stops at its next request.", "revoking a token");
  check(K.eventWords({ actor: ALICE, result: "allowed", action: "saml.set", target: "saml:acme.test" }, {}) === "@alice set up company sign-in by SAML.", "setting up SAML");
  check(K.MATRIX["scim.manage"].owner === "yes" && !K.MATRIX["scim.manage"].admin && K.MATRIX["audit-export.view"].admin === "yes" && !K.MATRIX["audit-export.manage"].admin, "the page's matrix has IC-955's rows, as the hub's");
}

if (failures()) {
  console.log(`\n${failures()} failed`);
  process.exit(1);
}
console.log("\nall passed");
