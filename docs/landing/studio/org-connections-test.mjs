// The organisation dashboard's company connections (IC-952), run from the
// shipped scripts against a stand-in hub in the hub's shapes: the
// Connections card in Settings lists each connection without its
// credential; an owner or admin adds one, replaces its credential, changes
// what it allows and who uses it, and removes it; the credential typed is
// checked and sealed in the page to the organisation's key (F-316), so
// nothing but the sealed blob ever leaves the browser, then cleared and
// never shown; a developer sees the list read-only; a credential kept the
// old way says it must be entered again; and the words for each new
// refusal and audit event.
//
//   node docs/landing/studio/org-connections-test.mjs
import { readFileSync } from "node:fs";
import { DASH, check, page, settle, ACME, member, button, failures, orgRoutes } from "./org-test-kit.mjs";

const ALICE = "github:alice";
const SECRET = "Bearer sk-page-test-only-77665544";
const t = (s) => String(s || "").replace(/\s+/g, " ");
// acme's organisation key: the cross-language vector's, which this test
// opens what the page sealed with.
const VECTOR = JSON.parse(readFileSync("crates/orgkeys/tests/vectors/org-seal.json", "utf8"));
const KEYS = { current: { version: 3, public_key: VECTOR.org_public, state: "current", created_at: 1760000000 }, devices: [] };
/* Everything that left the page: each request's address, headers and body. */
const leftThePage = (p) => JSON.stringify(p.sent);
const clearIn = (text, values) => values.some((v) => text.includes(v) || text.includes(Buffer.from(v).toString("base64").slice(0, 20)));
const openWith = async (p, context, sealed) => {
  const S = p.win.KrateOrgSeal;
  try {
    return await S.openForOrg(await S.deriveOrgKey(VECTOR.org_ikm), context, sealed);
  } catch (_) {
    return null;
  }
};
const people = [member(ALICE, "owner"), member("github:bob", "reviewer"), member("github:carol", "developer"), member("github:dan", "recipient"), member("github:erin", "admin")];
const CONN = {
  name: "finance-api",
  base: "https://api.acme.example/finance",
  allow: { methods: ["GET"], paths: ["/invoices"] },
  audience: ["finance"],
  apps: ["com.acme.ledger"],
  credential: { kind: "header", header: "Authorization", set_at: 1760000500, set_by: ALICE, state: "sealed", version: 3 },
  created_at: 1760000500, created_by: ALICE, changed_at: 1760000500, changed_by: ALICE,
  uses: { today: 3, last_30_days: 41 },
};
const GROUPS = { groups: [{ name: "finance", created_at: 1760000000, created_by: ALICE, members: ["github:dan"] }, { name: "ops", created_at: 1760000000, created_by: ALICE, members: [] }] };
function as(me, extra = {}) {
  const list = [CONN];
  return {
    ...orgRoutes(me),
    "GET /orgs/acme": () => [200, { org: { ...ACME, state: "active", offline_default: "days-7" }, me: people.find((m) => m.principal === me), members: people }],
    "GET /orgs/acme/groups": () => [200, GROUPS],
    "GET /orgs/acme/sso": () => [200, null],
    "GET /orgs/acme/billing": () => [200, { plan: "pilot", state: "pilot", seats_used: 4, seats_paid: null, period_end: null, checkout_open: false, manual: null, invoices: [] }],
    "GET /orgs/acme/exports": () => [200, { exports: [] }],
    "GET /orgs/acme/connections": () => [200, { connections: list }],
    "GET /orgs/acme/keys": () => [200, KEYS],
    ...extra,
  };
}
const card = (p) => p.el("connCard");
const inputs = (root, name) => root.all((n) => n.tagName === "INPUT" && n.attrs.name === name);
const set = (root, name, v) => { inputs(root, name)[0].value = v; };

/* ---- the list ----------------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, { token: "t", routes: as(ALICE) });
  const c = card(p);
  check(c && /Company connections/.test(c.shown), "Connections has the Company connections card");
  const shown = t(c.shown);
  check(/finance-api GET on \/invoices/.test(shown), "each connection says its name and what it allows");
  check(shown.includes("https://api.acme.example/finance"), "and its address");
  check(/Only the finance group · com\.acme\.ledger/.test(shown), "and who and which apps may use it");
  check(/Credential: the Authorization header, set by @alice on .* · used 3 times today, 41 in 30 days/.test(shown), "and when its credential was set and how often it is used, never the credential");
  check(/Encrypted: Krate cannot read your credentials\. Only your members' computers can\./.test(shown) && /Encrypted: Krate cannot read this credential\./.test(shown), "the card and each connection say Krate cannot read the credential");
  const details = c.querySelector(".conndetails");
  const plain = t(shown.replace(details ? details.shown : "", ""));
  check(details && /Details/.test(details.shown) && /version 3/.test(details.shown) && /HPKE \(RFC 9180/.test(details.shown), "the exact terms are behind Details");
  check(!/HPKE|version 3|sealed blob|key version/i.test(plain), "and no machinery words outside it");
  check(!shown.includes("sk-"), "no credential anywhere on the page");
  check(/Tells the API who is calling: each request carries a signed Krate-Identity header naming the person/.test(shown), "a connection from before the setting says it tells the API who is calling");
  check(/cap = "net\.connection:finance-api"/.test(shown), "how an app asks for it, with the real name");
  check(button(c, "New connection") && button(c, "Replace credential") && button(c, "Change") && button(c, "Remove"), "an owner can add, replace, change and remove");
}

/* ---- adding one --------------------------------------------------------- */
{
  let created = null;
  const p = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, {
    token: "t",
    routes: as("github:erin", { "POST /orgs/acme/connections": (_h, body) => { created = body; return [201, { connection: { ...CONN, name: body.name } }]; } }),
  });
  const c = card(p);
  button(c, "New connection").click();
  await settle();
  const form = c.querySelector(".connbox");
  check(form && /New connection/.test(form.shown), "New connection opens a form");
  check(inputs(form, "credValue")[0].attrs.type === "password", "the value field never shows what is typed");
  check(inputs(form, "headerName")[0].value === "Authorization", "the header starts as Authorization");
  check(inputs(form, "method").find((i) => i.value === "GET").checked === true && inputs(form, "method").filter((i) => i.checked).length === 1, "GET alone is ticked to begin with");

  // The page checks what it can before anything is sent.
  button(form, "Add connection").click();
  await settle();
  check(/The name is short and plain/.test(form.shown) && !created, "a missing name is said, nothing sent");
  set(form, "connName", "payroll-api");
  set(form, "connBase", "http://payroll.acme.example");
  button(form, "Add connection").click();
  await settle();
  check(/The address starts with https:\/\//.test(form.shown) && !created, "a plain http address is refused in the page");
  // Typed loosely: the page sends it, and seals for it, as the hub keeps it.
  set(form, "connBase", "https://Payroll.Acme.example/v2/");
  button(form, "Add connection").click();
  await settle();
  check(/The credential is missing/.test(form.shown) && !created, "no credential, nothing sent");
  // The hub cannot look inside the seal, so the page checks the value first.
  set(form, "credValue", "Bearer a\nX-Evil: 1");
  button(form, "Add connection").click();
  await settle();
  check(/The value is one line/.test(form.shown) && !created && !p.sent.some((s) => s.key === "GET /orgs/acme/keys"), "a value on two lines is refused in the page, before anything is asked");
  set(form, "credValue", SECRET);
  inputs(form, "method").find((i) => i.value === "POST").checked = true;
  set(form, "paths", "/runs, /people");
  // Only some groups.
  button(form, "Some groups").click();
  button(form, "Add connection").click();
  await settle();
  check(/Pick at least one group/.test(form.shown) && !created, "Some groups with none ticked is said");
  inputs(form, "group").find((i) => i.value === "finance").checked = true;
  set(form, "apps", "com.acme.payroll");
  button(form, "Add connection").click();
  await settle();
  check(created && created.name === "payroll-api" && created.base === "https://payroll.acme.example/v2", "Add sends the name and the address, as the hub keeps it");
  check(created && !created.header && !created.basic && created.credential && created.credential.kind === "header" && created.credential.header === "Authorization" && created.credential.v === 3, "and the credential sealed, with the key version it is sealed to");
  const context = p.win.KrateOrgSeal.connectionContext("acme", "payroll-api", "Authorization", "https://payroll.acme.example/v2");
  check(created && (await openWith(p, context, created.credential.sealed)) === SECRET, "which opens with the organisation's key, for this connection's name, header and address");
  check(!clearIn(leftThePage(p), [SECRET, "sk-page-test-only"]), "nothing but the sealed blob left the browser: no request carries the credential or its base64");
  check(created && JSON.stringify(created.allow) === JSON.stringify({ methods: ["GET", "POST"], paths: ["/runs", "/people"] }), "and what it allows");
  check(created && JSON.stringify(created.audience) === JSON.stringify({ groups: ["finance"] }) && JSON.stringify(created.apps) === JSON.stringify({ ids: ["com.acme.payroll"] }), "and who and which apps");
  check(created && created.identity === true && inputs(form, "identity")[0].checked === true && /Tell the API who is calling/.test(form.shown), "Tell the API who is calling starts on, and is sent");
  const post = p.sent.find((s) => s.key === "POST /orgs/acme/connections");
  check(post && post.headers["idempotency-key"], "with an Idempotency-Key");
  check(/The payroll-api connection is ready/.test(p.el("toast").shown), "and says it is ready");
  check(!t(p.el("pane").shown).includes("sk-page"), "the typed credential is gone from the page");
}

/* ---- replacing the credential, changing, removing ------------------------ */
{
  const seen = {};
  const p = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, {
    token: "t",
    routes: as(ALICE, {
      "PUT /orgs/acme/connections/finance-api/credential": (_h, b) => { seen.cred = b; return [200, { connection: CONN }]; },
      "PUT /orgs/acme/connections/finance-api": (_h, b) => { seen.change = b; return [400, { error: "credential-required", message: "x" }]; },
      "DELETE /orgs/acme/connections/finance-api": () => { seen.removed = true; return [204, null]; },
    }),
  });
  const c = card(p);
  button(c, "Replace credential").click();
  await settle();
  const box = c.querySelector(".connbox");
  check(/Replace the credential of finance-api/.test(box.shown) && /The old one is deleted/.test(box.shown), "Replace says the old one is deleted and apps get the new one next time");
  button(box, "User and password").click();
  set(box, "credUser", "svc-ledger");
  set(box, "credPassword", "hunter2-not-real");
  button(box, "Replace credential").click();
  await settle();
  check(seen.cred && seen.cred.credential && seen.cred.credential.kind === "basic" && seen.cred.credential.header === "Authorization" && !seen.cred.basic && !seen.cred.header, "a user and password go as basic, sealed");
  const basicAt = p.win.KrateOrgSeal.connectionContext("acme", "finance-api", "Authorization", "https://api.acme.example/finance");
  check(seen.cred && (await openWith(p, basicAt, seen.cred.credential.sealed)) === `Basic ${Buffer.from("svc-ledger:hunter2-not-real").toString("base64")}`, "and open to the Basic header, for the connection's own address");
  check(!clearIn(leftThePage(p), ["hunter2-not-real", "svc-ledger:hunter2"]), "the password never left the browser");
  check(/finance-api has a new credential/.test(p.el("toast").shown), "and it says apps get it next time");

  button(card(p), "Change").click();
  await settle();
  const ch = card(p).querySelector(".connbox");
  check(/Its address stays https:\/\/api\.acme\.example\/finance/.test(ch.shown), "Change says the address stays, and how to move it");
  check(inputs(ch, "paths")[0].value === "/invoices" && inputs(ch, "group").find((i) => i.value === "finance").checked, "it starts from what the connection allows now");
  button(ch, "Save changes").click();
  await settle();
  check(seen.change && JSON.stringify(seen.change.allow) === JSON.stringify({ methods: ["GET"], paths: ["/invoices"] }) && !seen.change.header, "Change sends what it allows and who, never a credential");
  check(seen.change && seen.change.identity === true, "and whether it tells the API who is calling");
  inputs(ch, "identity")[0].checked = false;
  button(ch, "Save changes").click();
  await settle();
  check(seen.change && seen.change.identity === false, "turning it off sends identity false");
  check(/A new address needs its credential again/.test(ch.shown), "a refusal is said in words");

  button(card(p), "Remove").click();
  check(/Remove finance-api\?/.test(p.el("sheetH").textContent) && /Its credential is deleted from Krate/.test(p.el("sheetP").textContent), "Remove asks first and says the credential goes");
  p.el("sheetYes").onclick();
  await settle();
  check(seen.removed && /finance-api is removed, with its credential/.test(p.el("toast").shown), "and removes it");
}

/* ---- who sees what ------------------------------------------------------ */
{
  const dev = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, { token: "t", routes: as("github:carol") });
  const c = card(dev);
  check(c && /finance-api/.test(c.shown), "a developer sees the connections, to build apps that use them");
  check(!button(c, "New connection") && !button(c, "Replace credential") && !button(c, "Remove") && !button(c, "Change"), "and nothing to change them");
  const none = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, { token: "t", routes: as(ALICE, { "GET /orgs/acme/connections": () => [200, { connections: [] }] }) });
  check(/No connections yet/.test(card(none).shown), "an empty list says so, and how to begin");
  const older = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, { token: "t", routes: as(ALICE, { "GET /orgs/acme/connections": () => [404, { error: "hub-error", message: "" }] }) });
  check(!older.el("pane").querySelector(".conn"), "a hub from before connections shows no card that can only fail");
  const refused = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, { token: "t", routes: as(ALICE, { "GET /orgs/acme/connections": () => [503, { error: "connections-off", message: "" }] }) });
  check(/Connections are not switched on on this hub yet/.test(card(refused).shown), "a hub with them switched off says so in words");
}

/* ---- encryption's own states -------------------------------------------- */
{
  // No organisation key yet: the page says the one step, and sends nothing.
  let posted = false;
  const p = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, {
    token: "t",
    routes: as(ALICE, { "GET /orgs/acme/keys": () => [200, { current: null, devices: [] }], "POST /orgs/acme/connections": () => { posted = true; return [201, {}]; } }),
  });
  const c = card(p);
  button(c, "New connection").click();
  await settle();
  const form = c.querySelector(".connbox");
  set(form, "connName", "hr-api");
  set(form, "connBase", "https://hr.acme.example");
  set(form, "credValue", SECRET);
  button(form, "Add connection").click();
  await settle();
  check(/Acme needs one setup step first/.test(form.shown) && /krate org keys init/.test(form.shown) && !posted, "no organisation key: the one step is said, and nothing is sent");
  check(!clearIn(leftThePage(p), [SECRET]), "and the credential stayed in the browser");

  // A credential kept the old way says it must be entered again, and how.
  const old = { ...CONN, credential: { ...CONN.credential, state: "needs-entry", version: null } };
  const q = await page("https://krate.tech/org/?o=acme&tab=connections", DASH, { token: "t", routes: as(ALICE, { "GET /orgs/acme/connections": () => [200, { connections: [old] }] }) });
  const shown = t(card(q).shown);
  check(/Enter this credential again/.test(shown) && /deleted it rather than keep a copy it could read/.test(shown) && /Replace credential/.test(shown), "a credential kept the old way says to enter it again, with the button that does it");
  check(!/Encrypted: Krate cannot read this credential/.test(shown), "and does not claim to be encrypted");
}

/* ---- the words ---------------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=activity", DASH, { token: "t", routes: as(ALICE) });
  const K = p.win.KrateOrg;
  const say = (e) => K.eventWords({ actor: ALICE, result: "allowed", reason: null, ...e }, {});
  check(say({ action: "connection.create", target: "connection:finance-api", reason: "https://api.acme.example/finance" }) === "@alice added the finance-api connection, to https://api.acme.example/finance.", "added");
  check(say({ action: "connection.credential", target: "connection:finance-api" }) === "@alice replaced the credential of the finance-api connection.", "credential replaced");
  check(say({ action: "connection.change", target: "connection:finance-api", reason: "allow GET,POST /" }) === "@alice changed the finance-api connection: allow GET,POST /.", "changed");
  check(say({ action: "connection.remove", target: "connection:finance-api" }) === "@alice removed the finance-api connection and its credential.", "removed");
  check(/^Apps used the finance-api connection 12 times on \d+ Oct 2026\.$/.test(say({ actor: "system:krate", action: "connection.use", target: "connection:finance-api", reason: "12 uses on 2026-10-13" })), "the daily count");
  check(say({ actor: "github:bob", result: "denied", action: "connection.use", target: "connection:finance-api", reason: "connection-not-in-audience" }) === "@bob tried to use the finance-api connection: the connection is not for their groups.", "a refused use");
  check(/encrypted the connections' credentials again, after someone left/.test(say({ actor: "github:carol", action: "connection.reseal", target: "key:v2", reason: "2 credentials sealed again by abc" })), "a rotation's re-encryption");
  check(/^The credential of the finance-api connection must be entered again/.test(say({ actor: "system:krate", action: "connection.needs-entry", target: "connection:finance-api" })), "a credential to enter again");
  for (const reason of ["no-such-connection", "connection-taken", "connection-invalid", "credential-required", "connections-off", "connection-not-in-audience", "app-not-allowed", "connection-not-requested", "no-org-key", "key-rotated", "credential-needs-entry"]) {
    const w = K.refusal(reason, { org: "Acme", message: "SENTINEL -- next" });
    check(w.title && w.title !== "That did not work", `${reason} has its own words`);
  }
  check(K.MATRIX["connection.manage"].admin === "yes" && !K.MATRIX["connection.manage"].developer && K.MATRIX["connection.view"].reviewer === "yes" && !K.MATRIX["connection.view"].recipient, "the page's matrix has the connection rows");
}

if (failures()) {
  console.log(`\n${failures()} failed`);
  process.exit(1);
}
console.log("\nall passed");
