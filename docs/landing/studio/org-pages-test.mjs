// The organisation pages on krate.tech (IC-947): /orgs/, /org/, /join/ and
// the private side of /open/, run from their shipped scripts against a
// stand-in hub that answers in the contract's shapes
// (Plan/org-pilot/CONTRACT.md, Addenda 1 and 2).
//
// What is checked is what a person meets: which tabs and buttons each role
// is shown (the contract's role matrix), that every refusal reads as words
// with a next step and never as its code, that Approve names the exact
// digest it sends, and that a private app names its organisation only to a
// member or to someone removed from it.
//
//   node docs/landing/studio/org-pages-test.mjs
import { KIT, ORGS, DASH, JOIN, OPEN, check, page, settle, storage, P, ACME, member, DIGEST, orgRoutes, tabNames, button, failures } from "./org-test-kit.mjs";

/* ------------------------------------------------------- the kit itself */
{
  const p = await page("https://krate.tech/orgs/", "void 0");
  const K = p.win.KrateOrg;

  // The hub address: krate.tech always talks to hub.krate.tech.
  const loc = (h) => ({ hostname: h });
  const st = (v) => storage({ "krate-dev-hub": v });
  check(K.hub(loc("krate.tech"), st("http://127.0.0.1:8799")) === "https://hub.krate.tech", "krate.tech ignores a development hub setting");
  check(K.hub(loc("127.0.0.1"), st("http://127.0.0.1:8799")) === "http://127.0.0.1:8799", "a copy served on this machine may use a hub on this machine");
  check(K.hub(loc("127.0.0.1"), st("https://evil.example")) === "https://hub.krate.tech", "and never a hub anywhere else");
  check(K.hub(loc("localhost"), st("")) === "https://hub.krate.tech", "with nothing set, it is the real hub");

  // Tabs per role, from the contract's matrix.
  const tabs = (r) => K.tabsFor(r).join(",");
  check(tabs("owner") === "home,apps,waiting,requests,releases,people,computers,security,connections,insights,activity,billing,settings", "an owner runs the organisation: Overview, Apps, Approvals, Requests, people, computers, security, connections, numbers, activity, billing, settings");
  check(tabs("admin") === "home,apps,waiting,requests,releases,people,computers,security,connections,insights,activity,billing,settings", "an admin has the same console as an owner");
  check(tabs("developer") === "home,myapps,build,requests,releases,connections,insights,people,settings", "a developer makes apps: Home, My apps, New app, Requests, company apps, connections, and no Activity");
  check(tabs("reviewer") === "home,waiting,apps,releases,insights,people,settings", "a reviewer has a home and the queue, sees the apps, and has no Activity");
  check(tabs("recipient") === "home,requests,help", "a member has their apps, their requests and help, nothing else");

  check(K.invitable("owner").join() === "owner,admin,developer,reviewer,recipient", "an owner invites any role");
  check(K.invitable("admin").join() === "developer,reviewer,recipient", "an admin invites developers, reviewers and recipients only");
  check(K.invitable("developer").length === 0 && K.invitable("reviewer").length === 0 && K.invitable("recipient").length === 0, "nobody else invites");

  const A = (me, them, owners = 2) => K.memberActions({ principal: me[0], role: me[1] }, { principal: them[0], role: them[1] }, owners);
  check(A(["github:a", "admin"], ["github:o", "owner"]).remove === false, "an admin is not offered Remove on an owner");
  check(A(["github:a", "admin"], ["github:d", "developer"]).remove === true, "an admin may remove a developer");
  check(A(["github:a", "admin"], ["github:d", "developer"]).roles.join() === "developer,reviewer,recipient", "an admin moves people between developer, reviewer and recipient");
  check(A(["github:a", "admin"], ["github:x", "admin"]).roles.length === 0, "an admin cannot change another admin's role");
  check(A(["github:d", "developer"], ["github:e", "reviewer"]).remove === false, "a developer cannot remove someone else");
  check(A(["github:d", "developer"], ["github:d", "developer"]).removeLabel === "Leave", "but can leave");
  const last = A(["github:o", "owner"], ["github:o", "owner"], 1);
  check(last.remove && /only owner/i.test(last.removeWhy) && last.roles.length === 0, "the last owner's Leave explains why it cannot happen, and offers no other role");
  check(A(["github:o", "owner"], ["github:p", "owner"], 2).removeWhy === "", "with two owners, an owner can be removed");

  // Every stable reason reads as words with a next step, never as itself.
  const REASONS = ["not-signed-in", "not-a-member", "removed", "role-cannot", "last-owner", "self-approval", "digest-mismatch",
    "not-signed-for-this-org", "invite-expired", "invite-used", "invite-for-someone-else", "invite-revoked", "slug-taken",
    "slug-invalid", "idempotency-key-reused", "no-such-org", "offline"];
  for (const r of REASONS) {
    const w = K.refusal(r, { org: "Acme", slug: "acme", here: "/org/?o=acme" });
    const all = w.title + " " + w.body;
    // "removed" and "offline" are words already; a code with a dash is not.
    check(w.title && w.body && (!r.includes("-") || !all.includes(r)) && !/[—–`]|--/.test(all), `"${r}" is words: ${w.title}`);
  }
  const si = K.refusal("not-signed-in", { here: "/org/?o=acme&tab=people" });
  check(si.action && si.action.href === "/login/?next=%2Forg%2F%3Fo%3Dacme%26tab%3Dpeople", "the sign-in refusal comes back to the same page");

  // The audit in words.
  const w1 = K.eventWords({ actor: "github:carol", action: "candidate.approve", target: `candidate:${DIGEST}`, result: "denied", reason: "self-approval" }, { [DIGEST]: "Bounce 1.0.0" });
  check(w1 === "@carol tried to approve Bounce 1.0.0: they sent it themselves.", "a refused self-approval reads plainly");
  const w2 = K.eventWords({ actor: "github:alice", action: "invite.create", target: "invite:email:erin@acme.test/recipient", result: "allowed" });
  check(w2 === "@alice invited erin@acme.test as member.", "an email invitation names the inbox");
  const w3 = K.eventWords({ actor: "github:alice", action: "member.role", target: "member:github:bob", result: "allowed", reason: "reviewer->developer" });
  check(w3 === "@alice changed @bob from reviewer to developer.", "a role change says from and to");
  check(K.fingerprint(ACME.root_key) === "0d92 19f2 81ef db72", "a key fingerprint is its first 16 hex in fours");
}

/* ---------------------------------------------------------------- /orgs/ */
{
  const out = await page("https://krate.tech/orgs/", ORGS);
  check(out.el("out").hidden === false && out.el("in").hidden === true, "/orgs/ signed out shows the sign-in choices");
  const links = out.el("outChoices").all((n) => n.tagName === "A").map((a) => a.href);
  check(links.join() === "/login/?next=%2Forgs%2F#github,/login/?next=%2Forgs%2F#google", "GitHub and Google, each coming back to /orgs/");
  const form = out.el("outChoices").all((n) => n.tagName === "FORM")[0];
  const box = form && form.all((n) => n.tagName === "INPUT")[0];
  check(box && box.attrs.type === "email", "and an email box right there");
  box.value = "not an address";
  form.dispatch("submit");
  check(!String(out.location.href).endsWith("#email"), "an address that is not one goes nowhere");
  box.value = "sam@acme.test";
  form.dispatch("submit");
  check(String(out.location.href).endsWith("/login/?next=%2Forgs%2F#email") && !String(out.location.href).includes("sam"), "an address goes on to the email sign-in, never in the link");

  const p = await page("https://krate.tech/orgs/", ORGS, {
    token: "t", routes: { "GET /orgs": () => [200, [{ slug: "acme", name: "Acme", role: "owner" }, { slug: "beta-lab", name: "Beta Lab", role: "recipient" }]] },
  });
  const rows = p.el("list").all((n) => n.tagName === "A");
  check(rows.length === 2 && rows[0].href === "/org/?o=acme" && /Acme acme Owner/.test(P(rows[0].shown)), "/orgs/ lists each organisation with your role");

  const made = await page("https://krate.tech/orgs/", ORGS, {
    token: "t",
    routes: {
      "GET /orgs": () => [200, []],
      "POST /orgs": (_h, b) => (b.slug === "acme" ? [409, { error: "slug-taken", message: "" }] : [201, { org: { slug: b.slug, name: b.name } }]),
    },
  });
  check(made.el("create").hidden === false, "with no organisation yet, the create form is open");
  made.el("cName").value = "Acme";
  made.el("cName").dispatch("input");
  check(made.el("cSlug").value === "acme", "the short name follows the name");
  made.el("createForm").dispatch("submit");
  await settle();
  check(/acme is taken/.test(made.el("cWhy").shown) && !/slug-taken/.test(made.el("cWhy").shown), "a taken name is said in words");
  const firstKey = made.sent.find((s) => s.key === "POST /orgs").headers["idempotency-key"];
  made.el("cSlug").value = "acme-labs";
  made.el("cSlug").dispatch("input");
  made.el("createForm").dispatch("submit");
  await settle();
  const second = made.sent.filter((s) => s.key === "POST /orgs")[1];
  check(second && second.headers["idempotency-key"] && second.headers["idempotency-key"] !== firstKey, "a different request gets a different Idempotency-Key");
  // IC-958: straight on to the first app, in the same page, the step in
  // the address so a reload goes on from there.
  check(made.el("wiz").hidden === false && made.el("in").hidden === true && made.location.href.endsWith("/orgs/?o=acme-labs&step=app"), "a new organisation goes straight on to its first app");
}

/* ----------------------------------------------------------------- /org/ */

{
  const owner = await page("https://krate.tech/org/?o=acme&tab=releases", DASH, { token: "tok-alice", routes: orgRoutes("github:alice") });
  check(tabNames(owner) === "home,apps,waiting,requests,releases,people,computers,security,connections,insights,activity,billing,settings", "/org/ shows an owner their console, grouped as the sidebar draws it");
  check(/you are an owner/i.test(owner.el("oMeta").textContent), "and says which role they hold");
  check(/Bounce 1\.0\.0/.test(owner.el("pane").shown) && /Sent by @carol · approved by @bob/.test(owner.el("pane").shown), "Releases names who sent and who approved");
  check(button(owner.el("pane"), "Open in Krate") && button(owner.el("pane"), "Copy link"), "each release has Open in Krate and Copy link");
  check(/Approvals/.test(owner.el("tabs").shown) && owner.el("tab-waiting").shown.includes("1"), "the Approvals tab counts what is waiting");

  const dan = await page("https://krate.tech/org/?o=acme&tab=activity", DASH, { token: "tok-dan", routes: orgRoutes("github:dan") });
  check(tabNames(dan) === "home,requests,help", "a member's /org/ has their apps, requests and help only");
  check(dan.location.href.endsWith("tab=home"), "and a link to a tab they cannot see lands on the Dashboard instead");

  // A reviewer approves somebody else's upload, and the confirmation names the digest it sends.
  const bob = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, {
    token: "tok-bob",
    routes: orgRoutes("github:bob", { [`POST /orgs/acme/candidates/${"a".repeat(64)}/approve`]: () => [200, { release: { digest: "a".repeat(64), url: "x" } }] }),
  });
  const approve = button(bob.el("pane"), "Approve");
  check(!!approve, "a reviewer is offered Approve on carol's upload");
  approve.click();
  check(bob.el("sheetDigest").shown.includes("a".repeat(64)), "the confirmation shows the exact digest");
  check(bob.sent.every((s) => !s.key.endsWith("/approve")), "nothing is approved before the person confirms");
  bob.el("sheetYes").onclick();
  await settle();
  const sentApprove = bob.sent.find((s) => s.key.endsWith("/approve"));
  check(sentApprove && sentApprove.key === `POST /orgs/acme/candidates/${"a".repeat(64)}/approve` && sentApprove.headers["idempotency-key"], "Approve sends that digest, once, with an Idempotency-Key");
  check(/is released/.test(bob.el("toast").shown), "and says it is released");

  // A developer follows their own versions in My apps, not in the queue.
  const carol = await page("https://krate.tech/org/?o=acme&tab=myapps", DASH, { token: "tok-carol", routes: orgRoutes("github:carol") });
  check(!button(carol.el("pane"), "Approve") && /Bounce/.test(carol.el("pane").shown) && /With a reviewer/.test(carol.el("pane").shown), "the sender sees their version is with a reviewer, and has no Approve button");

  // A refused approval stays on the sheet, in words.
  const late = await page("https://krate.tech/org/?o=acme&tab=waiting", DASH, {
    token: "tok-bob",
    routes: orgRoutes("github:bob", { [`POST /orgs/acme/candidates/${"a".repeat(64)}/approve`]: () => [404, { error: "digest-mismatch", message: "" }] }),
  });
  button(late.el("pane"), "Approve").click();
  late.el("sheetYes").onclick();
  await settle();
  check(/not waiting any more/.test(late.el("sheetWhy").shown) && !/digest-mismatch/.test(late.el("sheetWhy").shown), "a refused approval is explained on the sheet");

  // People: invite by GitHub and by email, then the join link and its expiry.
  const inv = await page("https://krate.tech/org/?o=acme&tab=people", DASH, {
    token: "tok-alice",
    routes: orgRoutes("github:alice", {
      "POST /orgs/acme/invites": (_h, b) => [201, { code: "c0de", expires_at: 1760604800, join_url: "https://krate.tech/join/?o=acme&c=c0de", ...b }],
    }),
  });
  inv.el("invWho").value = "@Bob";
  inv.el("invRole").value = "reviewer";
  button(inv.el("pane"), "Create invitation").click();
  await settle();
  const sentInv = inv.sent.find((s) => s.key === "POST /orgs/acme/invites");
  check(sentInv && sentInv.body.github === "bob" && sentInv.body.role === "reviewer" && !("email" in sentInv.body), "a GitHub invitation sends {github, role}");
  const out = inv.el("invOut");
  check(out.all((n) => n.tagName === "INPUT")[0].value === "https://krate.tech/join/?o=acme&c=c0de" && /until/.test(out.shown), "it shows the join link to copy, and when it expires");
  button(inv.el("pane"), "Work email").click();
  inv.el("invWho").value = "Erin@Acme.test";
  inv.el("invRole").value = "recipient";
  button(inv.el("pane"), "Create invitation").click();
  await settle();
  const sentMail = inv.sent.filter((s) => s.key === "POST /orgs/acme/invites")[1];
  check(sentMail && sentMail.body.email === "erin@acme.test" && !("github" in sentMail.body), "an email invitation sends {email, role}");
  const rows = inv.el("pane").querySelectorAll(".rw");
  const aliceRow = rows.find((r) => /@alice/.test(r.shown));
  check(aliceRow && !button(aliceRow, "Remove") && button(aliceRow, "Leave").attrs["aria-disabled"] === "true", "the only owner's own row holds Leave back");
  button(aliceRow, "Leave").click();
  check(/only owner/i.test(aliceRow.shown), "and pressing it says why");
  check(/Recipients/.test(inv.el("pane").shown) && /erin@acme\.test/.test(inv.el("pane").shown), "recipients are listed apart from members");

  const admin = await page("https://krate.tech/org/?o=acme&tab=people", DASH, {
    token: "tok-x", routes: orgRoutes("github:xadmin", { "GET /orgs/acme": () => [200, { org: ACME, me: member("github:xadmin", "admin"), members: [member("github:alice", "owner"), member("github:xadmin", "admin"), member("github:carol", "developer")] }] }),
  });
  const opts = admin.el("invRole").all((n) => n.tagName === "OPTION").map((o) => o.value).join();
  check(opts === "developer,reviewer,recipient", "an admin's invite offers only the roles an admin may invite");
  const ownerRow = admin.el("pane").querySelectorAll(".rw").find((r) => /@alice/.test(r.shown));
  check(!button(ownerRow, "Remove") && !ownerRow.querySelector("select"), "an admin has no Remove or role control on an owner");

  // Settings and Activity.
  const set = await page("https://krate.tech/org/?o=acme&tab=settings", DASH, { token: "tok-alice", routes: orgRoutes("github:alice") });
  check(/0d92 19f2 81ef db72/.test(set.el("pane").shown) && /by @alice/.test(set.el("pane").shown), "Settings shows the root key's fingerprint and who set it");
  check(/krate publish app\.krate --org acme/.test(set.el("pane").shown) && /krate delegate/.test(set.el("pane").shown) && /krate sign/.test(set.el("pane").shown), "and the commands that send an app to it");
  const act = await page("https://krate.tech/org/?o=acme&tab=activity", DASH, { token: "tok-alice", routes: orgRoutes("github:alice") });
  check(/Chain intact/.test(act.el("pane").shown) && /@alice created the organisation/.test(act.el("pane").shown), "Activity reads in words, with the chain badge");
  const broken = await page("https://krate.tech/org/?o=acme&tab=activity", DASH, {
    token: "tok-alice", routes: orgRoutes("github:alice", { "GET /orgs/acme/audit": () => [200, { events: [], chain_ok: false }] }),
  });
  check(/Chain broken/.test(broken.el("pane").shown), "a broken chain says so");

  // The page-level refusals.
  const removed = await page("https://krate.tech/org/?o=acme", DASH, { token: "tok-erin", routes: { "GET /orgs/acme": () => [403, { error: "removed", message: "" }] } });
  check(/You were removed from acme/.test(removed.el("stop").shown) && removed.el("in").hidden, "a removed member is told so, and sees nothing of the organisation");
  const expired = await page("https://krate.tech/org/?o=acme", DASH, { token: "old", routes: { "GET /orgs/acme": () => [401, { error: "not-signed-in", message: "" }] } });
  check(expired.el("out").hidden === false && expired.localStorage.getItem("krate_tok") === null, "an ended sign-in is dropped and the sign-in choices shown");
  const signedOut = await page("https://krate.tech/org/?o=acme", DASH);
  check(signedOut.el("outChoices").all((n) => n.tagName === "A")[0].href === "/login/?next=%2Forg%2F%3Fo%3Dacme#github", "/org/ signed out offers sign-in that returns to the organisation");
}

/* ---------------------------------------------------------------- /join/ */
{
  const out = await page("https://krate.tech/join/?o=acme&c=abc123def", JOIN);
  check(/invitation to acme/.test(out.el("jH").textContent) && out.el("jBody").all((n) => n.tagName === "A")[0].href === "/login/?next=%2Fjoin%2F%3Fo%3Dacme%26c%3Dabc123def#github", "/join/ signed out offers sign-in that returns to the invitation");

  const inv = { org: { slug: "acme", name: "Acme" }, role: "reviewer", invited_by: { principal: "github:alice", display: "alice" }, for: { principal: "github:bob", display: "bob", kind: "github" }, expires_at: 1760604800, state: "open" };
  const ok = await page("https://krate.tech/join/?o=acme&c=abc123def", JOIN, {
    token: "tok-bob",
    routes: {
      "GET /orgs/acme/invites/abc123def": () => [200, inv],
      "POST /orgs/acme/invites/abc123def/accept": () => [200, { membership: member("github:bob", "reviewer") }],
    },
  });
  check(/Join Acme/.test(ok.el("jH").textContent) && /@alice invited @bob/.test(ok.el("jP").textContent) && /Can approve apps/.test(ok.el("jBody").shown), "it says who invited whom, and what they can do, in plain words");
  button(ok.el("jBody"), "Join Acme").click();
  await settle();
  check(ok.sent.some((s) => s.key === "POST /orgs/acme/invites/abc123def/accept" && s.headers["idempotency-key"]), "Join posts to the invitation with an Idempotency-Key");
  const after = ok.el("jBody").all((n) => n.tagName === "A").map((a) => a.href);
  check(/You are in Acme/.test(ok.el("jH").textContent) && after[0] === "/download/" && after.includes("krate://org/apps?org=acme") && after.includes("/org/?o=acme"), "and then Get Krate, Open Krate, and the organisation on the web");

  for (const [reason, words] of [["invite-for-someone-else", /for someone else/], ["invite-expired", /expired/], ["invite-used", /already used/], ["invite-revoked", /withdrawn/]]) {
    const p = await page("https://krate.tech/join/?o=acme&c=abc123def", JOIN, {
      token: "tok-carol", routes: { "GET /orgs/acme/invites/abc123def": () => [reason === "invite-for-someone-else" ? 403 : 410, { error: reason, message: "" }], "GET /me": () => [200, { user: { login: "carol" } }] },
    });
    check(words.test(p.el("jH").textContent) && !p.el("jH").textContent.includes(reason) && !/Acme/.test(p.el("card").shown + p.el("jBody").shown), `an invitation that is ${reason.replace("invite-", "")} is said in words, naming no organisation`);
  }
  const broken = await page("https://krate.tech/join/?o=acme", JOIN, { token: "t" });
  check(/incomplete/.test(broken.el("jH").textContent) && broken.sent.length === 0, "a link with no code says so without asking the hub");
}

/* ------------------------------------------------ the private /open/ page */
{
  // The script block that renders the app the link names.
  const at = OPEN.indexOf("var hash;");
  const start = OPEN.lastIndexOf("(function () {", at);
  const end = OPEN.indexOf("\n})();", OPEN.indexOf("function privateApp", at)) + "\n})();".length;
  const code = OPEN.slice(start, end);
  const url = `https://krate.tech/open/?a=${DIGEST}&private=1`;
  const route = (status, body) => ({ [`GET /a/${DIGEST}`]: (h) => (h.accept === "application/json" ? [status, body] : [200, null]) });

  const out = await page(url, code);
  check(/Sign in to open it/.test(out.el("taName").textContent) && out.sent.length === 0, "signed out: sign in, and nothing is asked of the hub");
  check(!/Acme/.test(out.el("theirApp").shown + out.el("pageHead").textContent), "and the organisation is not named");
  check(out.el("taMore").all((n) => n.tagName === "A")[0].href === "/login/?next=" + encodeURIComponent(`/open/?a=${DIGEST}&private=1`) + "#github", "sign-in comes back to the app");

  const mem = await page(url, code, { token: "tok-erin", routes: route(200, { name: "Bounce", version: "1.0.0", size: 13778, org: { slug: "acme", name: "Acme" } }) });
  check(mem.sent.every((s) => !s.url.includes("/meta/")), "a private app never asks /meta");
  check(mem.sent[0].headers.accept === "application/json" && mem.sent[0].headers.authorization === "Bearer tok-erin", "it asks /a/ with the session and Accept: application/json");
  check(/From Acme/.test(mem.el("taKick").textContent) && /Bounce 1\.0\.0/.test(mem.el("taName").textContent) && mem.el("taRow").hidden === false, "a member sees the app, who sent it, and Download");

  const stranger = await page(url, code, { token: "tok-stranger", routes: route(403, { error: "not-a-member", message: "" }) });
  check(/not shared with you/.test(stranger.el("taName").textContent) && !/acme/i.test(stranger.el("theirApp").shown + stranger.el("pageHead").textContent), "a stranger is told it is not shared with them, and learns no organisation");
  check(stranger.el("steps").hidden === true, "and is not walked through opening it");

  const gone = await page(url, code, { token: "tok-erin", routes: route(403, { error: "removed", message: "", org: "acme", org_name: "Acme" }) });
  check(/You were removed from Acme/.test(gone.el("taName").textContent), "a removed member is told which organisation removed them");

  const ended = await page(url, code, { token: "old", routes: route(401, { error: "not-signed-in", message: "" }) });
  check(/Sign in to open it/.test(ended.el("taName").textContent) && ended.localStorage.getItem("krate_tok") === null, "an ended sign-in asks to sign in again");
}

const bad = failures();
console.log(bad ? `${bad} check(s) failed` : "ok  the organisation pages behave as the contract says");
process.exit(bad ? 1 : 0);
