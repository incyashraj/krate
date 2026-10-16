// Staged rollout, Slack and Teams channels, SIEM presets for the activity
// export, and the two Insights fixes (K-1114, K-1115), run from the shipped
// console scripts against a stand-in hub in the hub's shapes.
//
//   node docs/landing/studio/org-rollout-chat-test.mjs
import { DASH, check, page, settle, ACME, member, orgRoutes, button, failures } from "./org-test-kit.mjs";

const ALICE = "github:alice";
const APP = "dev.krate.bounce";
const D0 = "0".repeat(64);
const D1 = "1".repeat(64);
const D2 = "2".repeat(64);
const t = (s) => String(s || "").replace(/\s+/g, " ");
const has = (p, id) => p.el("pane").all((n) => n.attrs && n.attrs.id === id).length > 0;
const rel = (digest, version, extra = {}) => ({
  app_id: APP, name: "Bounce", version, digest, url: `https://hub.krate.tech/a/${digest}`, state: "released",
  uploaded_by: "github:carol", approved_by: "github:bob", at: 1760000300, audience: null, offline: "days-7", leases: { active: 0, until: null }, ...extra,
});
const V = (d, v) => ({ digest: d, version: v, name: "Bounce" });
const ROLLOUT = {
  everyone: V(D1, "1.0.1"),
  pilot: { group: "pilot", people: 12, version: V(D2, "1.0.2"), set_at: 1760000000, set_by: ALICE },
  pins: [{ group: "finance", people: 3, version: V(D0, "1.0.0"), set_at: 1760000000, set_by: ALICE }],
};
const appsWith = (rollout, extra = {}) => ({
  apps: [{ app_id: APP, name: "Bounce", current: D1, for_you: { digest: D1, ring: "everyone", group: null }, rollout, releases: [rel(D2, "1.0.2"), rel(D1, "1.0.1"), rel(D0, "1.0.0")], ...extra }],
});
const GROUPS = { groups: [{ name: "finance", created_at: 1, created_by: ALICE, members: ["github:dan", "github:fay", "github:gil"] }, { name: "pilot", created_at: 1, created_by: ALICE, members: Array.from({ length: 12 }, (_, i) => `github:p${i}`) }] };
const people = [member(ALICE, "owner"), member("github:erin", "admin"), member("github:bob", "reviewer"), member("github:carol", "developer"), member("github:dan", "recipient")];
const as = (me, extra = {}) => ({
  ...orgRoutes(me),
  "GET /orgs/acme": () => [200, { org: { ...ACME, state: "active", offline_default: "days-7" }, me: people.find((m) => m.principal === me), members: people }],
  "GET /orgs/acme/apps": () => [200, appsWith(ROLLOUT)],
  "GET /orgs/acme/groups": () => [200, GROUPS],
  ...extra,
});
const ringRows = (p) => p.el("pane").querySelectorAll(".ring");
const rowOf = (p, text) => p.el("pane").querySelectorAll(".rw").find((r) => r.querySelector("b") && r.querySelector("b").textContent.startsWith(text));

/* ---- staged rollout on Company apps ------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t",
    routes: as(ALICE, {
      [`POST /orgs/acme/apps/${APP}/pilot/release`]: () => [200, { app_id: APP, current: V(D2, "1.0.2"), rollout: { ...ROLLOUT, pilot: { ...ROLLOUT.pilot, version: null } } }],
      [`DELETE /orgs/acme/apps/${APP}/pins/finance`]: () => [204, null],
      [`POST /orgs/acme/releases/${D2}/withdraw`]: () => [200, { release: rel(D2, "1.0.2", { state: "withdrawn" }), current: { app_id: APP, digest: D1 }, leases: { active: 0, until: null }, fallback_words: "The pilot group had 1.0.2 as its pilot; it gets 1.0.1 now." }],
    }),
  });
  const rings = ringRows(p).map((r) => t(r.shown));
  check(/Pilot: 1\.0\.2 for the pilot group \(12 people\)/.test(rings[0]) && /Everyone else gets 1\.0\.1 until you release it to everyone/.test(rings[0]), "the card says the pilot's version, its group and how many people, and what everyone else gets");
  check(/Kept: the finance group \(3 people\) stays on 1\.0\.0/.test(rings[1]), "and each kept group, with what it stays on");
  check(/Everyone/.test(rowOf(p, "Bounce 1.0.1").shown) && /Pilot/.test(rowOf(p, "Bounce 1.0.2").shown) && /Kept for finance/.test(rowOf(p, "Bounce 1.0.0").shown), "each version says which ring it is");
  check(!/[0-9a-f]{64}/.test(p.el("pane").shown.replace(/https:\/\/\S+/g, "")), "no fingerprint on the card itself");

  button(ringRows(p)[0], "Release to everyone").click();
  check(/Release 1\.0\.2 to everyone\?/.test(p.el("sheetH").textContent) && /gets 1\.0\.2 from now on, instead of 1\.0\.1, which stays released, so you can switch back/.test(t(p.el("sheetP").textContent)), "Release to everyone asks first, in plain words");
  check(/The pilot group keeps getting new versions first\./.test(p.el("sheetPts").shown) && /Kept groups stay where they are: finance on 1\.0\.0\./.test(p.el("sheetPts").shown), "and says the pilot and kept groups stay as they are");
  check(p.sent.every((s) => !s.key.endsWith("/pilot/release")), "nothing is sent before the person confirms");
  p.el("sheetYes").onclick();
  await settle();
  const rel2 = p.sent.find((s) => s.key === `POST /orgs/acme/apps/${APP}/pilot/release`);
  check(rel2 && rel2.headers["idempotency-key"], "then it is sent once, with an Idempotency-Key");
  check(/1\.0\.2 is released to everyone\./.test(p.el("toast").shown), "and said");

  button(ringRows(p)[1], "Let them go").click();
  check(/Let the finance group get new versions\?/.test(p.el("sheetH").textContent) && /They get what everyone gets of Bounce \(1\.0\.1\)/.test(p.el("sheetP").textContent), "letting a kept group go says what they get instead");
  p.el("sheetYes").onclick();
  await settle();
  check(p.sent.some((s) => s.key === `DELETE /orgs/acme/apps/${APP}/pins/finance` && s.headers["idempotency-key"]), "and unpins it");

  // Withdrawing the pilot's version says where the pilot group goes.
  button(rowOf(p, "Bounce 1.0.2"), "Withdraw").click();
  check(/It is with the pilot group as its pilot: they get Bounce 1\.0\.1 again\./.test(p.el("sheetPts").shown), "withdrawing the pilot's version says the pilot group goes back");
  p.el("sheetInput").value = "breaks on save";
  p.el("sheetYes").onclick();
  await settle();
  check(/The pilot group had 1\.0\.2 as its pilot; it gets 1\.0\.1 now\./.test(p.el("toast").shown), "and the toast says the hub's own words for what changed");
  check(!button(rowOf(p, "Bounce 1.0.2"), "Release to everyone") && !button(rowOf(p, "Bounce 1.0.2"), "Make current"), "the pilot's version is released to everyone from the rings, once; its row offers no Make current");
}

/* ---- setting a pilot group and a pin ------------------------------------ */
{
  const none = { everyone: V(D1, "1.0.1"), pilot: null, pins: [] };
  const p = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, {
    token: "t",
    routes: as("github:erin", {
      "GET /orgs/acme/apps": () => [200, appsWith(none)],
      [`PUT /orgs/acme/apps/${APP}/pilot`]: (_h, b) => [200, { app_id: APP, rollout: { ...none, pilot: { group: b.group, people: 12, version: null } } }],
      [`PUT /orgs/acme/apps/${APP}/pins/finance`]: (_h, b) => [200, { app_id: APP, rollout: none }],
    }),
  });
  const pane = p.el("pane");
  button(pane, "Try new versions with a group first").click();
  await settle();
  const sel = pane.querySelectorAll("select").find((s) => s.attrs["aria-label"] === "Group");
  check(sel && sel.all((n) => n.tagName === "OPTION").map((o) => o.textContent).join("|") === "finance (3 people)|pilot (12 people)", "the groups to pick, with how many people");
  sel.value = "pilot";
  button(pane, "Next").click();
  check(/Send new versions to the pilot group first\?/.test(p.el("sheetH").textContent) && /Everyone else stays on 1\.0\.1 until you press Release to everyone\./.test(p.el("sheetP").textContent), "the sheet says what happens to the next approved version and to everyone else");
  p.el("sheetYes").onclick();
  await settle();
  const put = p.sent.find((s) => s.key === `PUT /orgs/acme/apps/${APP}/pilot`);
  check(put && put.body.group === "pilot" && put.headers["idempotency-key"], "an admin sets the pilot group");

  button(pane, "Keep a group on a version").click();
  await settle();
  const g = pane.querySelectorAll("select").find((s) => s.attrs["aria-label"] === "Group");
  const v = pane.querySelectorAll("select").find((s) => s.attrs["aria-label"] === "Version");
  g.value = "finance";
  v.value = D0;
  button(pane, "Next").click();
  check(/Keep the finance group on 1\.0\.0\?/.test(p.el("sheetH").textContent) && /whatever is released to everyone, until you let them go/.test(p.el("sheetP").textContent), "keeping a group on a version asks first");
  p.el("sheetYes").onclick();
  await settle();
  const pin = p.sent.find((s) => s.key === `PUT /orgs/acme/apps/${APP}/pins/finance`);
  check(pin && pin.body.digest === D0, "and keeps it on that exact version");
}

/* ---- people in a ring, and people who cannot change rings ---------------- */
{
  const mine = { apps: [{ app_id: APP, name: "Bounce", current: D2, for_you: { digest: D2, ring: "pilot", group: "pilot" }, releases: [rel(D2, "1.0.2"), rel(D1, "1.0.1")] }] };
  const p = await page("https://krate.tech/org/?o=acme&tab=home", DASH, { token: "t", routes: as("github:dan", { "GET /orgs/acme/apps": () => [200, mine], "GET /orgs/acme/insights": () => [200, { sections: ["yours"], yours: { apps: [], devices: [] } }] }) });
  check(/Version 1\.0\.2, early: you are in the pilot group, which gets new versions first\./.test(t(p.el("pane").shown)), "someone in the pilot group reads, on their apps, that they have an early version");
  check(!button(p.el("pane"), "Release to everyone") && !button(p.el("pane"), "Keep a group on a version"), "and has nothing to change");
  const dev = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: as("github:carol", { "GET /orgs/acme/apps": () => [200, mine] }) });
  check(/You are in the pilot group, which gets new versions first\./.test(dev.el("pane").shown), "a developer in the pilot group reads it on Company apps");
  const bob = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "t", routes: as("github:bob") });
  check(/Pilot: 1\.0\.2 for the pilot group/.test(bob.el("pane").shown) && !button(bob.el("pane"), "Release to everyone") && !button(bob.el("pane"), "Let them go"), "a reviewer sees the rings and cannot change them");
}

/* ---- one app's page ------------------------------------------------------ */
{
  const p = await page(`https://krate.tech/org/?o=acme&tab=apps&app=${APP}&view=versions`, DASH, {
    token: "t",
    routes: as(ALICE, { "GET /orgs/acme/inventory": () => [200, { apps: [] }], "GET /orgs/acme/insights": () => [200, { sections: [] }] }),
  });
  const shown = t(p.el("pane").shown);
  check(/1\.0\.1 is current · the pilot group has 1\.0\.2 first/.test(shown), "the app's header says what everyone gets and what the pilot has");
  check(/Pilot: 1\.0\.2 for the pilot group/.test(shown) && /Kept: the finance group/.test(shown), "its Versions tab has the rings");
  const vrows = p.el("pane").querySelectorAll(".vrow").map((r) => t(r.shown));
  check(vrows.some((r) => /^1\.0\.2 .*Pilot$/.test(r)) && vrows.some((r) => /^1\.0\.0 .*Kept for finance$/.test(r)) && vrows.some((r) => /^1\.0\.1 .*Everyone$/.test(r)), "each version in the list says its ring");
}

/* ---- the words ----------------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=activity", DASH, { token: "t", routes: as(ALICE) });
  const K = p.win.KrateOrg;
  const say = (e) => K.eventWords({ actor: ALICE, result: "allowed", reason: null, ...e }, { [D2]: "Bounce 1.0.2", [D0]: "Bounce 1.0.0" });
  check(say({ action: "release.pilot", target: `app:${APP}`, reason: "group:pilot" }) === `@alice set new versions of ${APP} to go to the pilot group first.`, "setting the pilot group");
  check(say({ actor: "github:bob", action: "release.pilot", target: `app:${APP}`, reason: `group:pilot ${D2} approved` }) === "Bounce 1.0.2 went to the pilot group first; everyone else stays where they are.", "an approval into the pilot");
  check(say({ action: "release.pilot.release", target: `app:${APP}`, reason: D2 }) === "@alice released Bounce 1.0.2 to everyone.", "release to everyone");
  check(say({ action: "release.pin", target: `app:${APP}/finance`, reason: D0 }) === "@alice kept the finance group on Bounce 1.0.0.", "a pin");
  check(say({ action: "release.unpin", target: `app:${APP}/finance`, reason: `${D0} withdrawn` }) === "The finance group is no longer kept on Bounce 1.0.0: it was withdrawn.", "a pin ended by a withdrawal");
  check(say({ action: "chat.add", target: "chat:hooks.slack.com" }) === "@alice connected a channel at hooks.slack.com for notices.", "a channel connected");
  const r = K.refusal("in-pilot", { org: "Acme", message: "Bounce 1.0.2 is with the pilot group in Acme first -- you get it when it is released to everyone", for_you: { version: "1.0.1" } });
  check(r.title === "This version goes to a pilot group first" && /Bounce 1\.0\.2 is with the pilot group in Acme first\. You get it when it is released to everyone in Acme\. Your version now is 1\.0\.1\./.test(r.body), "the in-pilot refusal in words, with the person's own version");
  check(K.MATRIX["release.rollout"].admin === "yes" && !K.MATRIX["release.rollout"].reviewer && K.MATRIX["chat.manage"].admin === "yes" && !K.MATRIX["chat.manage"].developer, "the page's matrix has the new rows, as the hub's");
}

/* ---- Slack and Teams ----------------------------------------------------- */
const org = (me) => ({
  "GET /orgs/acme": () => [200, { org: { ...ACME, offline_default: "days-7", state: "active", close: null, suspended: false, billing_state: "pilot", recovery: null }, me: people.find((m) => m.principal === me), members: people }],
  "GET /orgs/acme/exports": () => [200, []],
  "GET /orgs/acme/sso": () => [200, null],
  "GET /orgs/acme/scim": () => [404, null],
  "GET /orgs/acme/saml": () => [404, null],
});
const settings = (me, extra = {}) => page("https://krate.tech/org/?o=acme&tab=settings", DASH, {
  token: "t",
  routes: { ...orgRoutes(me), ...org(me), "GET /orgs/acme/audit-export": () => [200, null], "GET /orgs/acme/chat": () => [200, { hooks: [], kinds: [], per_day: 50, used_today: 0 }], ...extra },
});
const HOOK = { id: "a1b2c3d4e5f6", service: "slack", host: "hooks.slack.com", last4: "9Z7q", label: "#apps", kinds: ["approval-waiting", "security"], added_at: 1760000000, added_by: ALICE, last_ok_at: null, last_error: "the channel's webhook answered 404: it may have been removed in Slack -- add it again", last_error_at: 1760000500 };
{
  const p = await settings(ALICE, {
    "POST /orgs/acme/chat": (_h, b) => [201, { hook: { ...HOOK, kinds: b.kinds, label: b.label || null, last_error: null }, first: true }],
  });
  check(has(p, "chatCard") && /Slack and Teams/.test(p.el("chatCard").shown), "an owner sees Slack and Teams in Settings");
  const card = p.el("chatCard");
  p.el("chatUrl").value = "https://evil.example/services/T/B/x";
  button(card, "Connect channel").click();
  await settle();
  check(p.sent.every((s) => s.key !== "POST /orgs/acme/chat") && /That is not a Slack or Teams webhook/.test(card.shown) && /Workflows/.test(card.shown), "an address that is not a Slack or Teams webhook is caught here, with where to find the right one");
  const boxes = card.all((n) => n.tagName === "INPUT" && n.attrs.type === "checkbox");
  check(boxes.length === 4 && boxes.filter((b) => b.checked).length === 3 && !p.el("chatNew-request").checked, "four kinds to choose, requests off to start, as the mail has it");
  p.el("chatNew-request").checked = true;
  p.el("chatUrl").value = "https://hooks.slack.com/services/T000/B000/abcdEFGH9Z7q";
  p.el("chatLabel").value = "#apps";
  button(card, "Connect channel").click();
  await settle();
  const post = p.sent.find((s) => s.key === "POST /orgs/acme/chat");
  check(post && post.body.url.startsWith("https://hooks.slack.com/services/") && JSON.stringify(post.body.kinds) === JSON.stringify(["approval-waiting", "version-decided", "request", "security"]) && post.body.label === "#apps", "Connect sends the URL, the kinds and the name");
  check(/Connected\. Send a test message to check it arrives\./.test(p.el("toast").shown), "and says what to do next");
}
{
  const p = await settings("github:erin", {
    "GET /orgs/acme/chat": () => [200, { hooks: [HOOK], per_day: 50, used_today: 7 }],
    [`POST /orgs/acme/chat/${HOOK.id}/test`]: () => [502, { error: "chat-failed", message: "the test did not arrive: the channel's webhook answered 404: it may have been removed in Slack -- add it again" }],
    [`DELETE /orgs/acme/chat/${HOOK.id}`]: () => [204, null],
  });
  const card = p.el("chatCard");
  const shown = t(card.shown);
  check(/#apps on Slack/.test(shown) && /hooks\.slack\.com ending 9Z7q/.test(shown) && !/abcdEFGH/.test(shown), "a channel shows its name, host and last four, never the URL");
  check(/A version waits for approval; Security/.test(shown) && /Last problem, .*: the channel's webhook answered 404/.test(shown), "what it posts, and its last problem in words");
  check(/7 posts today, of 50 a day at most\./.test(shown), "and how much of the day's allowance is used");
  button(card, "Send a test message").click();
  await settle();
  check(p.sent.some((s) => s.key === `POST /orgs/acme/chat/${HOOK.id}/test`) && /The test did not arrive/.test(card.shown) && /removed in Slack/.test(card.shown), "an admin sends a test; a failed one says why");
  button(card, "Remove").click();
  check(/Stop posting to #apps on Slack\?/.test(p.el("sheetH").textContent) && p.sent.every((s) => !s.key.startsWith("DELETE")), "removing asks first");
  p.el("sheetYes").onclick();
  await settle();
  check(p.sent.some((s) => s.key === `DELETE /orgs/acme/chat/${HOOK.id}` && s.headers["idempotency-key"]), "then removes it");
  for (const who of ["github:bob", "github:carol"]) {
    const q = await settings(who);
    check(!has(q, "chatCard"), `${who.slice(7)} does not see it`);
  }
  const old = await settings(ALICE, { "GET /orgs/acme/chat": () => [404, { error: "not-found", message: "" }] });
  check(/not on this hub yet/.test(old.el("chatCard").shown), "an older hub is said in words");
}

/* ---- SIEM presets -------------------------------------------------------- */
{
  const SECRET = "whsec_" + "B".repeat(43);
  const p = await settings(ALICE, { "PUT /orgs/acme/audit-export": (_h, b) => [201, { url: b.url || "https://http-intake.logs.datadoghq.eu/api/v2/logs", host: "x", preset: b.preset, fields: b.fields || {}, has_token: true, has_secret: true, delivery: {}, secret: SECRET }] });
  const card = p.el("auditCard");
  const seg = card.all((n) => n.tagName === "BUTTON" && n.dataset && n.dataset.preset).map((b) => b.textContent);
  check(seg.join("|") === "Any receiver|Splunk|Microsoft Sentinel|Datadog", "four destinations to choose from");
  card.all((n) => n.dataset && n.dataset.preset === "splunk")[0].click();
  check(/HTTP Event Collector address/.test(card.shown) && /HEC token/.test(card.shown) && has(p, "auditIndex"), "Splunk asks for its collector, token, index and sourcetype");
  p.el("auditUrl").value = "https://http-inputs-acme.splunkcloud.com";
  button(card, "Start sending").click();
  await settle();
  check(p.sent.every((s) => s.key !== "PUT /orgs/acme/audit-export") && /Give the HEC token/.test(card.shown), "without the token nothing is sent, and it says what is missing");
  p.el("auditToken").value = "1b2c3d4e-5f60-4a7b-8c9d-0e1f2a3b4c5d";
  p.el("auditIndex").value = "security";
  button(card, "Start sending").click();
  await settle();
  const put = p.sent.find((s) => s.key === "PUT /orgs/acme/audit-export");
  check(put && put.body.preset === "splunk" && put.body.url === "https://http-inputs-acme.splunkcloud.com" && put.body.token === "1b2c3d4e-5f60-4a7b-8c9d-0e1f2a3b4c5d" && put.body.fields.index === "security", "Splunk sends its address, token and index");

  const s = await settings(ALICE, { "PUT /orgs/acme/audit-export": () => [201, { url: "u", host: "h", preset: "sentinel", fields: {}, has_token: true, has_secret: true, delivery: {} }] });
  const c2 = s.el("auditCard");
  c2.all((n) => n.dataset && n.dataset.preset === "sentinel")[0].click();
  check(/Logs Ingestion API/.test(c2.shown) && /TimeGenerated, Org, Seq, Actor, Action, Target, Result, Reason, PrevHash, Hash and Attempt/.test(c2.shown), "Sentinel says how to set up the rule, with its columns");
  for (const [id, v] of [["auditEndpoint", "https://acme-a1b2.westeurope-1.ingest.monitor.azure.com"], ["auditTenant", "11111111-2222-3333-4444-555555555555"], ["auditClient", "66666666-7777-8888-9999-000000000000"], ["auditToken", "s3cr3t"], ["auditDcr", "dcr-" + "ab".repeat(16)], ["auditStream", "Custom-KrateAudit_CL"]]) s.el(id).value = v;
  button(c2, "Start sending").click();
  await settle();
  const sp = s.sent.find((x) => x.key === "PUT /orgs/acme/audit-export");
  check(sp && sp.body.preset === "sentinel" && sp.body.fields.dcr_id === "dcr-" + "ab".repeat(16) && sp.body.fields.stream === "Custom-KrateAudit_CL" && sp.body.token === "s3cr3t" && !sp.body.url, "Sentinel sends the rule's fields and the client secret, no address of its own");

  const d = await settings(ALICE, {
    "GET /orgs/acme/audit-export": () => [200, { url: "https://http-intake.logs.datadoghq.eu/api/v2/logs", host: "http-intake.logs.datadoghq.eu", preset: "datadog", fields: { site: "datadoghq.eu" }, has_token: true, has_secret: true, set_at: 1, set_by: ALICE, delivery: { pending: 0, delivered: 4 } }],
    "PUT /orgs/acme/audit-export": (_h, b) => [200, { url: "u", host: "http-intake.logs.datadoghq.eu", preset: "datadog", fields: b.fields, has_token: true, has_secret: true, delivery: {} }],
  });
  const c3 = d.el("auditCard");
  check(/http-intake\.logs\.datadoghq\.eu, as Datadog/.test(c3.shown) && d.el("auditSite").value === "datadoghq.eu" && /Leave it empty to keep the key saved/.test(c3.shown), "a saved Datadog export shows where it goes, its site, and that its key is kept");
  button(c3, "Save").click();
  await settle();
  const dp = d.sent.find((x) => x.key === "PUT /orgs/acme/audit-export");
  check(dp && dp.body.preset === "datadog" && dp.body.fields.site === "datadoghq.eu" && dp.body.token === undefined, "saving without a new key keeps the one saved");
}

/* ---- K-1114 and K-1115 ---------------------------------------------------- */
{
  const p = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, { token: "t", routes: orgRoutes("github:bob") });
  button(p.el("pane"), "Approve").click();
  const dg = p.el("sheetDigest");
  const det = dg.all((n) => n.tagName === "DETAILS")[0];
  check(det && det.all((n) => n.tagName === "SUMMARY")[0].textContent === "Technical details" && det.shown.includes("a".repeat(64)), "K-1114: the approve sheet's fingerprint is folded under Technical details");
  check(!/[0-9a-f]{64}/.test(t(p.el("pane").shown)) && !/sha256/i.test(p.el("pane").shown), "and the Waiting page itself shows no sha256");

  const dev = (name, extra) => ({ device_id: name.slice(0, 4), name, principal: ALICE, display: "alice", added_at: 1750000000, last_seen: null, holds_current: true, identified: true, silent: false, ...extra });
  const sec = {
    key: { version: 1, state: "current" }, two_person: true, approvers: 2, solo_releases: 0, publishers: [],
    devices: [dev("Old PC", { identified: false }), dev("Mac", { last_seen: 1755000000, silent: true })],
    refusals: { week: [], week_total: 0, weekly_average_before: 0, spike: false }, withdrawn_offline: [], sign_in: {}, directory: null, audit_export: null,
  };
  const insightsBody = { sections: ["org"], names: true, now: 1760000000, org: { adoption: { members: 1, active_week: 0, active_month: 0, daily: [], trend: [], apps: 0, apps_used: 0, top: [], unused: [], joined_not_opened: 0 }, people: { by_role: {}, by_sign_in: {}, inactive: [], invites: [], company_sign_in: null, without_company_sign_in: [] }, security: sec, releases: { approved_month: 0, median_wait_seconds: null, approvals_counted: 0, waiting: 0, oldest_waiting_since: null, withdrawn_month: 0, rollbacks_month: 0 }, cost: { plan: "pilot", state: "pilot", seats_used: 1, seats_paid: null, free_seats: 3, active_month: 0 } } };
  const q = await page("https://krate.tech/org/?o=acme&tab=insights", DASH, { token: "t", routes: { ...orgRoutes(ALICE), "GET /orgs/acme/insights": () => [200, insightsBody] } });
  const shown = t(q.el("pane").shown);
  check(/Old PC Not identified/.test(shown) && /its Krate is older and does not say when it checks in, so update Krate there to see it here/.test(shown), "K-1115: a computer that never said which it is reads Not identified, with what to do");
  check(/1 has been silent for 60 days/.test(shown) && !/Old PC Silent/.test(shown), "and only the computer that stopped checking in counts as silent");
  check(!/devices?\b/i.test(shown.replace(/device manag\w+/gi, "")), "K-1114: Insights says computers, never devices");
}

process.exit(failures() ? 1 : 0);
