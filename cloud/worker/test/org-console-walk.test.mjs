/* The organisation console (docs/landing/org/) driven against the REAL hub,
 * not a stand-in: the shipped page scripts in the console tests' own small
 * DOM (docs/landing/studio/org-test-kit.mjs), every request answered by
 * this worker over its real Org store. A stand-in answers the shapes its
 * author remembers; this caught the app page reading `people.month`, which
 * the hub never sends, so an owner's app page said 0 people while three
 * had opened it.
 *
 * What an admin and a member see after one app is shipped, updated in a
 * pilot group and opened in browser tabs:
 * - "Copy link" hands out the app's own address, which follows updates and
 *   opens on a phone, wherever the app (not one old version) is shared;
 * - the app page counts the people who opened it, and says who has which
 *   version, by name;
 * - a member's card opens the app in a tab on a phone, Krate first on a
 *   computer.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-console-walk.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, acme, fixture, worker } from "./org-tools.mjs";
import { initKey } from "./org-keys-tools.mjs";

// The console kit reads the shipped scripts by their paths from the repo root.
process.chdir(join(dirname(fileURLToPath(import.meta.url)), "..", "..", ".."));
const { DASH, page, button } = await import("../../../docs/landing/studio/org-test-kit.mjs");

const APP = "dev.krate.bounce";
const SHARE = `https://hub.example/run/org/acme/${APP}`;
const PHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Mobile/15E148 Safari/604.1";
const MAC = "Mozilla/5.0 (Macintosh) Chrome/120";
const t = (s) => String(s || "").replace(/\s+/g, " ");
const enc = encodeURIComponent;
const must = (r, what) => {
  assert.ok(r.status < 300, `${what}: ${r.status} ${JSON.stringify(r.body)}`);
  return r;
};

/// The page's fetch, answered by the worker: the console's hub address is
/// rewritten to the test hub's, and the page's own Authorization goes along.
function realHub(env) {
  return async (url, init = {}) => {
    const u = new URL(url);
    const res = await worker.fetch(new Request(`https://hub.example${u.pathname}${u.search}`, { method: init.method || "GET", headers: init.headers || {}, body: init.body }), env);
    return res;
  };
}

/// Open a console page as `who`, against the real hub; waits until `ready`
/// says the pane has drawn.
async function open(env, who, url, ready, ua = MAC) {
  const copied = [];
  const p = await page(url, DASH, {
    token: `gho_${who}`,
    ua,
    extra: { fetch: realHub(env), navigator: { userAgent: ua, clipboard: { writeText: async (text) => { copied.push(text); } } } },
  });
  for (let i = 0; i < 200 && !ready(p); i++) await new Promise((r) => setTimeout(r, 5));
  assert.ok(ready(p), `${url} as ${who} never drew: ${t(p.el("pane").shown).slice(0, 300)}`);
  return { p, copied, text: () => t(p.el("pane").shown) };
}
const click = async (b) => {
  b.click();
  for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 0));
};

/* acme: alice owner, bob reviewer, carol developer, dan and frank
 * recipients; dan in the pilot group. Bounce 1.0.0 released; carol and
 * frank open it in a tab; 1.0.1 goes to the pilot and dan opens that. */
async function shipped() {
  const env = orgEnv({ ORG_LEASE_KEY: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60" });
  await acme(env);
  await initKey(env);
  const inv = must(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "recipient" } }), "invite");
  must(await call(env, "frank", "POST", `/orgs/acme/invites/${inv.body.code}/accept`), "accept");
  must(await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "pilot" } }), "group");
  must(await call(env, "alice", "PUT", `/orgs/acme/groups/pilot/members/${enc("github:dan")}`), "add dan");
  const ship = async (name) => {
    const up = must(await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) }), `send ${name}`);
    must(await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`), `approve ${name}`);
    return up.body.digest;
  };
  const d1 = await ship("candidate-v1.krate");
  must(await call(env, "alice", "PUT", `/orgs/acme/apps/${enc(APP)}/pilot`, { json: { group: "pilot" } }), "pilot");
  for (const who of ["carol", "frank"]) must(await call(env, who, "GET", `/a/${d1}/web`), `${who} opens in a tab`);
  const d2 = await ship("candidate-v2.krate");
  must(await call(env, "dan", "GET", `/a/${d2}/web`), "dan opens the pilot's version in a tab");
  return { env, d1, d2 };
}

test("an owner's app page: the app's own link, the people who opened it, and who has which version by name", async () => {
  const { env } = await shipped();
  const base = `https://krate.tech/org/?o=acme&tab=apps&app=${APP}`;
  const o = await open(env, "alice", base, (p) => /People, 30 days/.test(t(p.el("pane").shown)));
  assert.match(o.text(), /People, 30 days 3 who opened it/, "three people opened it, said as three");
  await click(button(o.p.el("pane"), "Copy link"));
  assert.deepStrictEqual(o.copied, [SHARE], "the header copies the app's own address");

  const u = await open(env, "alice", `${base}&view=usage`, (p) => /Who has which version/.test(t(p.el("pane").shown)));
  const s = u.text();
  assert.match(s, /1\.0\.0 Opens now 2 people @carol, @frank/, "what everyone gets, and who is on it");
  assert.match(s, /1\.0\.1 1 person still on it; they get the one that opens now the next time they open it online @dan/, "the pilot's version, and who has it");
  assert.ok(s.indexOf("1.0.0 Opens now") < s.indexOf("1.0.1 1 person"), "the version that opens now first");
});

test("Releases: the version that opens now copies the app's address; an older version copies its own", async () => {
  const { env, d2 } = await shipped();
  const r = await open(env, "alice", "https://krate.tech/org/?o=acme&tab=releases", (p) => p.el("pane").querySelectorAll(".rw").length >= 2);
  const rows = r.p.el("pane").querySelectorAll(".rw");
  const cur = rows.find((x) => /Bounce 1\.0\.0/.test(x.querySelector("b").textContent));
  const pilot = rows.find((x) => /Bounce 1\.0\.1/.test(x.querySelector("b").textContent));
  await click(button(cur, "Copy link"));
  await click(button(pilot, "Copy link"));
  assert.deepStrictEqual(r.copied, [SHARE, `https://hub.example/a/${d2}`]);
  assert.match(t(cur.shown), /It opens the version that is current for each person, now and after updates, on a phone or a computer/);
  assert.match(t(pilot.shown), /It opens exactly this version, and stops if it is withdrawn/);
});

test("a member's card: Open in the tab on a phone; on a computer Krate first and the tab beside it; the link survives updates", async () => {
  const { env } = await shipped();
  const ready = (p) => p.el("pane").querySelectorAll(".lcard").length > 0;
  const phone = await open(env, "frank", "https://krate.tech/org/?o=acme", ready, PHONE);
  const card = phone.p.el("pane").querySelectorAll(".lcard")[0];
  const links = card.all((n) => n.tagName === "A");
  assert.deepStrictEqual(links.map((a) => [a.textContent, a.href]), [["Open", SHARE]], "one link, the app's own");
  assert.ok(button(card, "Open in Krate").hidden, "no .krate download offered to a phone");
  await click(button(card, "Copy link"));
  assert.deepStrictEqual(phone.copied, [SHARE]);

  const mac = await open(env, "frank", "https://krate.tech/org/?o=acme", ready, MAC);
  const mcard = mac.p.el("pane").querySelectorAll(".lcard")[0];
  const kids = mcard.querySelector(".lc-a").children;
  assert.deepStrictEqual(kids.map((k) => k.textContent), ["Open in Krate", "Open here", "Copy link"], "Krate first on a computer");
  assert.ok(!kids[0].hidden && /\bdark\b/.test(kids[0].className) && kids[1].href === SHARE);
});

test("removing someone whose computer holds offline access says until when it still opens apps, before the owner confirms", async () => {
  const { env, d1 } = await shipped();
  const lease = must(await call(env, "frank", "GET", `/orgs/acme/leases/${d1}`), "frank's offline access");
  const r = await open(env, "alice", "https://krate.tech/org/?o=acme&tab=people", (p) => p.el("pane").all((n) => n.tagName === "BUTTON" && n.textContent === "Remove").length >= 4);
  const removes = (name) => r.p.el("pane").all((n) => n.tagName === "BUTTON" && n.textContent === "Remove").find((b) => {
    let row = b.parent;
    while (row && !/@frank|@dan/.test(t(row.textContent))) row = row.parent;
    return row && t(row.textContent).includes(name) && !t(row.textContent).includes(name === "@frank" ? "@dan" : "@frank");
  });
  await click(removes("@frank"));
  const said = t(r.p.el("sheetP").textContent);
  const day = r.p.win.KrateOrg.day(lease.body.lease.expires_at);
  assert.match(said, new RegExp(`A computer of theirs that stays offline keeps opening the apps it already has until ${day.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`), said);
  await click(removes("@dan"));
  assert.match(t(r.p.el("sheetP").textContent), /^Their access ends at once/, "someone with no offline access: it ends at once");
});
