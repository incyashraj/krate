/* The organisation store's rules (IC-946; chapter 14 sections 3 and 7.3):
 * the role matrix, invitations, the last-owner rule under a race,
 * idempotency, and the hash-chained audit.
 *
 * Drives the real worker and the real Org Durable Object over SQLite
 * (test/org-tools.mjs), so every rule is checked on the path a request
 * takes, not by calling the store's methods directly.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-store.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { MATRIX, ROLES, sha256HexSync, canonicalJson, eventPreimage, GENESIS } from "../src/org.js";
import { orgEnv, call, acme, fixture } from "./org-tools.mjs";

const DAY = 24 * 60 * 60;

/* ---- the audit's hash ------------------------------------------------ */

test("the synchronous SHA-256 the audit is written with agrees with crypto.subtle", async () => {
  const inputs = ["", "abc", "a".repeat(55), "a".repeat(56), "a".repeat(64), "a".repeat(1000), "acme é中😀"];
  for (const input of inputs) {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(input));
    const want = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
    assert.strictEqual(sha256HexSync(input), want, `length ${input.length}`);
  }
  assert.strictEqual(canonicalJson({ b: 1, a: [null, "x"] }), '{"a":[null,"x"],"b":1}');
});

/* ---- the role matrix, as the contract's table ------------------------ */

test("the executable matrix is the contract's table", () => {
  const table = {
    "org.view": "y y y y own",
    "invite.owner-admin": "y - - - -",
    "invite.other": "y y - - -",
    "role.change": "y y - - -",
    "role.owner": "y - - - -",
    "member.remove": "y not-owners self self self",
    "root-key.set": "y - - - -",
    "candidate.publish": "y - y - -",
    "candidate.approve": "y - - y -",
    "release.download": "y y y y y",
    "audit.read": "y y - - -",
    // FLEET-CONTRACT.md R (IC-949).
    "release.withdraw": "y y - y -",
    "release.set-current": "y y - - -",
    "group.manage": "y y - - -",
    "release.audience": "y y - - -",
    // Company connections (IC-952).
    "connection.manage": "y y - - -",
    "connection.view": "y y y y -",
    "connection.use": "y y y y y",
    // IC-955: provisioning and audit export.
    "scim.manage": "y - - - -",
    "audit-export.manage": "y - - - -",
    "audit-export.view": "y y - - -",
    // store.org (IC-951): whoever may open a release may use its data.
    "data.use": "y y y y y",
    "data.usage": "y y - - -",
    // The keyring (F-316).
    "keys.hold": "y y y y y",
    "keys.init": "y - - - -",
    "keys.recover": "y y - - -",
    // IC-954: inventory, ownership and usage.
    "inventory.view": "y y - y -",
    "inventory.people": "y y - - -",
    "app.owner": "y y - - -",
    "usage.view": "y y - - -",
    // The signing ring and publishers (IC-958, org-signing.js).
    "signing.hold": "y y - - -",
    "publishers.approve": "y y - - -",
    "publishing.settings": "y - - - -",
    // IC-959: insights, a section per role; names only to owners and admins.
    "insights.view": "y y y y y",
    "insights.org": "y y - - -",
    "insights.developer": "y - y - -",
    "insights.reviewer": "y - - y -",
    "insights.people": "y y - - -",
    // F-319: the organisation's time zone (org-time.js).
    "org.time-zone": "y y - - -",
    "org.logo": "y y - - -",
    // App requests (org-requests.js).
    "request.create": "y y y y y",
    "request.view": "y y y own own",
    "request.manage": "y y y - -",
    // Approval rules and the code behind an approval (org-approvals.js):
    // the people who approve, and a version's own sender, read its code.
    "candidate.changes": "y - own y -",
    "approval.policy": "y - - - -",
  };
  assert.deepStrictEqual(Object.keys(MATRIX).sort(), Object.keys(table).sort());
  for (const [action, row] of Object.entries(table)) {
    row.split(" ").forEach((cell, i) => {
      const role = ROLES[i];
      const want = cell === "y" ? "yes" : cell === "-" ? undefined : cell;
      assert.strictEqual(MATRIX[action][role], want, `${action} for ${role}`);
    });
  }
});

/* Each row of the table, driven through the routes for every role. erin is
 * an admin; the rest come from acme(). */
async function acmeWithAdmin() {
  const env = orgEnv();
  await acme(env);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "admin" } });
  assert.strictEqual(inv.status, 201);
  assert.strictEqual((await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`)).status, 200);
  return env;
}
const BY_ROLE = { owner: "alice", admin: "erin", developer: "carol", reviewer: "bob", recipient: "dan" };

test("view: every role sees the organisation; a recipient sees only their own entry", async () => {
  const env = await acmeWithAdmin();
  for (const [role, login] of Object.entries(BY_ROLE)) {
    const r = await call(env, login, "GET", "/orgs/acme");
    assert.strictEqual(r.status, 200, `${role}: ${JSON.stringify(r.body)}`);
    assert.strictEqual(r.body.me.role, role);
    assert.strictEqual(r.body.org.slug, "acme");
    if (role === "recipient") {
      assert.deepStrictEqual(r.body.members.map((m) => m.display), ["dan"], "own entry only");
    } else {
      assert.strictEqual(r.body.members.length, 5);
    }
  }
  const stranger = await call(env, "stranger", "GET", "/orgs/acme");
  assert.strictEqual(stranger.status, 403);
  assert.strictEqual(stranger.body.error, "not-a-member");
  const nobody = await call(env, null, "GET", "/orgs/acme");
  assert.strictEqual(nobody.status, 401);
  assert.strictEqual(nobody.body.error, "not-signed-in");
});

test("invite owner or admin: owners only", async () => {
  const env = await acmeWithAdmin();
  for (const role of ["owner", "admin"]) {
    for (const [as, login] of Object.entries(BY_ROLE)) {
      const r = await call(env, login, "POST", "/orgs/acme/invites", { json: { github: "frank", role } });
      if (as === "owner") assert.strictEqual(r.status, 201, `${as} invites ${role}`);
      else assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], `${as} invites ${role}`);
    }
  }
});

test("invite developer, reviewer or recipient: owners and admins", async () => {
  const env = await acmeWithAdmin();
  for (const role of ["developer", "reviewer", "recipient"]) {
    for (const [as, login] of Object.entries(BY_ROLE)) {
      const r = await call(env, login, "POST", "/orgs/acme/invites", { json: { github: "frank", role } });
      if (as === "owner" || as === "admin") {
        assert.strictEqual(r.status, 201, `${as} invites ${role}`);
        assert.match(r.body.code, /^[a-z0-9]{20}$/);
        assert.ok(r.body.expires_at > Date.now() / 1000 + 7 * DAY - 60, "expires 7 days after issue");
      } else {
        assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], `${as} invites ${role}`);
      }
    }
  }
});

test("change role among non-owner roles: owners and admins; owner-making: owners only", async () => {
  const env = await acmeWithAdmin();
  for (const [as, login] of Object.entries(BY_ROLE)) {
    if (as === "developer") continue; // carol is the target
    const r = await call(env, login, "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "reviewer" } });
    if (as === "owner" || as === "admin") {
      assert.strictEqual(r.status, 200, `${as}: ${JSON.stringify(r.body)}`);
      assert.strictEqual(r.body.membership.role, "reviewer");
      await call(env, "alice", "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "developer" } });
    } else {
      assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], as);
    }
  }
  const adminMakesOwner = await call(env, "erin", "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "owner" } });
  assert.deepStrictEqual([adminMakesOwner.status, adminMakesOwner.body.error], [403, "role-cannot"]);
  const adminUnmakesOwner = await call(env, "erin", "PUT", "/orgs/acme/members/github%3Aalice", { json: { role: "developer" } });
  assert.deepStrictEqual([adminUnmakesOwner.status, adminUnmakesOwner.body.error], [403, "role-cannot"]);
  const ownerMakesOwner = await call(env, "alice", "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "owner" } });
  assert.strictEqual(ownerMakesOwner.body.membership.role, "owner");
  const ownerUnmakesOwner = await call(env, "alice", "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "developer" } });
  assert.strictEqual(ownerUnmakesOwner.body.membership.role, "developer");
});

test("remove: owners anyone, admins anyone but owners, everyone else only themselves", async () => {
  const env = await acmeWithAdmin();
  // Not themselves: the others are refused.
  for (const [as, login] of Object.entries(BY_ROLE)) {
    if (as === "owner" || as === "admin") continue;
    const r = await call(env, login, "DELETE", "/orgs/acme/members/github%3Aerin");
    assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], as);
  }
  const adminRemovesOwner = await call(env, "erin", "DELETE", "/orgs/acme/members/github%3Aalice");
  assert.deepStrictEqual([adminRemovesOwner.status, adminRemovesOwner.body.error], [403, "role-cannot"]);
  assert.strictEqual((await call(env, "erin", "DELETE", "/orgs/acme/members/github%3Adan")).status, 204, "admin removes a recipient");
  // Themselves: each may leave.
  assert.strictEqual((await call(env, "bob", "DELETE", "/orgs/acme/members/github%3Abob")).status, 204, "reviewer leaves");
  assert.strictEqual((await call(env, "carol", "DELETE", "/orgs/acme/members/github%3Acarol")).status, 204, "developer leaves");
  assert.strictEqual((await call(env, "alice", "DELETE", "/orgs/acme/members/github%3Aerin")).status, 204, "owner removes an admin");
  // Removed is removed: their next request says so.
  const gone = await call(env, "bob", "GET", "/orgs/acme");
  assert.deepStrictEqual([gone.status, gone.body.error], [403, "removed"]);
});

test("root key: owners only", async () => {
  const env = await acmeWithAdmin();
  for (const [as, login] of Object.entries(BY_ROLE)) {
    const r = await call(env, login, "PUT", "/orgs/acme/root-key", { json: { public_key: "ab".repeat(32) } });
    if (as === "owner") assert.strictEqual(r.status, 200);
    else assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], as);
  }
});

test("publish a candidate: owners and developers", async () => {
  const env = await acmeWithAdmin();
  const bytes = { owner: "root-signed.krate", developer: "candidate-v1.krate" };
  for (const [as, login] of Object.entries(BY_ROLE)) {
    const r = await call(env, login, "POST", "/orgs/acme/candidates", { bytes: fixture(bytes[as] || "candidate-v2.krate") });
    if (as === "owner" || as === "developer") assert.strictEqual(r.status, 201, `${as}: ${JSON.stringify(r.body)}`);
    else assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], as);
  }
});

test("approve a candidate: owners and reviewers, never the uploader", async () => {
  const env = await acmeWithAdmin();
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  const digest = up.body.digest;
  for (const as of ["admin", "recipient"]) {
    const r = await call(env, BY_ROLE[as], "POST", `/orgs/acme/candidates/${digest}/approve`);
    assert.deepStrictEqual([r.status, r.body.error], [403, "role-cannot"], as);
  }
  const self = await call(env, "carol", "POST", `/orgs/acme/candidates/${digest}/approve`);
  assert.deepStrictEqual([self.status, self.body.error], [403, "self-approval"], "the developer who uploaded it");
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${digest}/approve`)).status, 200, "a reviewer");
  // An owner who uploads is refused too, and another owner-or-reviewer approves.
  const mine = await call(env, "alice", "POST", "/orgs/acme/candidates", { bytes: fixture("root-signed.krate") });
  const ownSelf = await call(env, "alice", "POST", `/orgs/acme/candidates/${mine.body.digest}/approve`);
  assert.deepStrictEqual([ownSelf.status, ownSelf.body.error], [403, "self-approval"], "an owner who uploaded it");
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${mine.body.digest}/approve`)).status, 200);
});

test("download a release: every role; read the audit: owners and admins", async () => {
  const env = await acmeWithAdmin();
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
  for (const [as, login] of Object.entries(BY_ROLE)) {
    const r = await call(env, login, "GET", `/a/${up.body.digest}`);
    assert.strictEqual(r.status, 200, as);
    const listed = await call(env, login, "GET", "/orgs/acme/releases");
    assert.strictEqual(listed.status, 200, as);
    assert.strictEqual(listed.body[0].digest, up.body.digest);
    const audit = await call(env, login, "GET", "/orgs/acme/audit");
    if (as === "owner" || as === "admin") assert.strictEqual(audit.status, 200, as);
    else assert.deepStrictEqual([audit.status, audit.body.error], [403, "role-cannot"], as);
  }
});

/* ---- the last owner -------------------------------------------------- */

test("no single command leaves zero owners: remove, demote, or accept a lesser invitation", async () => {
  const env = orgEnv();
  await acme(env);
  const leave = await call(env, "alice", "DELETE", "/orgs/acme/members/github%3Aalice");
  assert.deepStrictEqual([leave.status, leave.body.error], [409, "last-owner"]);
  const demote = await call(env, "alice", "PUT", "/orgs/acme/members/github%3Aalice", { json: { role: "developer" } });
  assert.deepStrictEqual([demote.status, demote.body.error], [409, "last-owner"]);
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "alice", role: "developer" } });
  const accept = await call(env, "alice", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  assert.deepStrictEqual([accept.status, accept.body.error], [409, "last-owner"]);
  const show = await call(env, "alice", "GET", "/orgs/acme");
  assert.strictEqual(show.body.me.role, "owner");
});

async function twoOwners() {
  const env = orgEnv();
  await acme(env, { root: null });
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "owner" } });
  await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  return env;
}
const owners = async (env) =>
  (await call(env, "bob", "GET", "/orgs/acme")).body.members.filter((m) => m.role === "owner" && m.state === "active").map((m) => m.display);

test("two owners leaving at the same moment leave exactly one, and the second hears last-owner", async () => {
  for (let round = 0; round < 20; round++) {
    const env = await twoOwners();
    assert.deepStrictEqual((await owners(env)).sort(), ["alice", "erin"]);
    const before = env.ORGS.requests;
    // Both requests are in flight before either commits: both reach the
    // store, and each one's transaction runs whole.
    const [a, e] = await Promise.all([
      call(env, "alice", "DELETE", "/orgs/acme/members/github%3Aalice"),
      call(env, "erin", "DELETE", "/orgs/acme/members/github%3Aerin"),
    ]);
    assert.strictEqual(env.ORGS.requests - before, 2);
    const results = [a, e].map((r) => r.status).sort();
    assert.deepStrictEqual(results, [204, 409], `round ${round}`);
    const loser = [a, e].find((r) => r.status === 409);
    assert.strictEqual(loser.body.error, "last-owner");
    assert.strictEqual((await owners(env)).length, 1, "exactly one owner is left");
  }
});

test("two owners removing each other at the same moment leave exactly one", async () => {
  const env = await twoOwners();
  const [a, e] = await Promise.all([
    call(env, "alice", "DELETE", "/orgs/acme/members/github%3Aerin"),
    call(env, "erin", "DELETE", "/orgs/acme/members/github%3Aalice"),
  ]);
  assert.deepStrictEqual([a, e].map((r) => r.status).sort(), [204, 403]);
  assert.strictEqual([a, e].find((r) => r.status === 403).body.error, "removed", "the one removed first has no authority left");
  assert.strictEqual((await owners(env)).length, 1);
});

/* ---- invitations ----------------------------------------------------- */

test("an invitation is single-use, for one login, and expires seven days after issue", async () => {
  const env = orgEnv();
  await acme(env);
  // A login is compared without case; stored lower-case.
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "Frank", role: "developer" } });
  assert.strictEqual(inv.body.display, "frank");
  const other = await call(env, "stranger", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  assert.deepStrictEqual([other.status, other.body.error], [403, "invite-for-someone-else"]);
  const first = await call(env, "frank", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  assert.strictEqual(first.status, 200);
  assert.deepStrictEqual([first.body.membership.display, first.body.membership.role], ["frank", "developer"]);
  const again = await call(env, "frank", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  assert.deepStrictEqual([again.status, again.body.error], [410, "invite-used"]);

  const store = env.ORGS.object("acme");
  const issued = Math.floor(Date.now() / 1000);
  const late = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "reviewer" } });
  store.now = () => issued + 7 * DAY + 5;
  const expired = await call(env, "erin", "POST", `/orgs/acme/invites/${late.body.code}/accept`);
  assert.deepStrictEqual([expired.status, expired.body.error], [410, "invite-expired"]);
  store.now = () => issued + 7 * DAY - 60;
  assert.strictEqual((await call(env, "erin", "POST", `/orgs/acme/invites/${late.body.code}/accept`)).status, 200, "inside the week it works");
});

test("a withdrawn invitation cannot be accepted, and only its issuer's peers can withdraw it", async () => {
  const env = await acmeWithAdmin();
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "admin" } });
  const byAdmin = await call(env, "erin", "DELETE", `/orgs/acme/invites/${inv.body.code}`);
  assert.deepStrictEqual([byAdmin.status, byAdmin.body.error], [403, "role-cannot"], "an admin cannot withdraw an admin invitation");
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/invites/${inv.body.code}`)).status, 204);
  const accept = await call(env, "frank", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  assert.deepStrictEqual([accept.status, accept.body.error], [410, "invite-revoked"]);
  // Removing an issuer withdraws what they issued and nobody accepted.
  const pending = await call(env, "erin", "POST", "/orgs/acme/invites", { json: { github: "frank", role: "developer" } });
  assert.strictEqual((await call(env, "alice", "DELETE", "/orgs/acme/members/github%3Aerin")).status, 204);
  const orphan = await call(env, "frank", "POST", `/orgs/acme/invites/${pending.body.code}/accept`);
  assert.deepStrictEqual([orphan.status, orphan.body.error], [410, "invite-revoked"]);
});

/* ---- slugs ----------------------------------------------------------- */

test("a slug is validated, and one already created cannot be created again", async () => {
  const env = orgEnv();
  for (const bad of ["A", "a", "1acme", "acme_co", "-acme", "a".repeat(40), ""]) {
    const r = await call(env, "alice", "POST", "/orgs", { json: { slug: bad } });
    assert.deepStrictEqual([r.status, r.body.error], [400, "slug-invalid"], JSON.stringify(bad));
  }
  assert.strictEqual((await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } })).status, 201);
  const taken = await call(env, "stranger", "POST", "/orgs", { json: { slug: "acme", name: "Mine now" } });
  assert.deepStrictEqual([taken.status, taken.body.error], [409, "slug-taken"]);
  const none = await call(env, "alice", "GET", "/orgs/nowhere");
  assert.deepStrictEqual([none.status, none.body.error], [404, "no-such-org"]);
  const nobody = await call(env, null, "POST", "/orgs", { json: { slug: "other" } });
  assert.deepStrictEqual([nobody.status, nobody.body.error], [401, "not-signed-in"]);
});

/* ---- idempotency ----------------------------------------------------- */

test("the same Idempotency-Key gets the same result, once; a reused key with another body is refused", async () => {
  const env = orgEnv();
  const created = await call(env, "alice", "POST", "/orgs", { json: { slug: "acme" }, key: "k-create" });
  assert.strictEqual(created.status, 201);
  const replay = await call(env, "alice", "POST", "/orgs", { json: { slug: "acme" }, key: "k-create" });
  assert.deepStrictEqual([replay.status, replay.body], [201, created.body], "a retried create is its first answer, not slug-taken");

  const one = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "bob", role: "reviewer" }, key: "k1" });
  const two = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "bob", role: "reviewer" }, key: "k1" });
  assert.strictEqual(one.status, 201);
  assert.deepStrictEqual(two.body, one.body, "the same code back");
  const db = env.ORGS.object("acme").ctx.storage.db;
  assert.strictEqual(db.prepare("SELECT COUNT(*) AS n FROM invites").get().n, 1, "the effect happened once");
  assert.strictEqual(db.prepare("SELECT COUNT(*) AS n FROM audit WHERE action = 'invite.create'").get().n, 1);

  const reused = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "carol", role: "developer" }, key: "k1" });
  assert.deepStrictEqual([reused.status, reused.body.error], [409, "idempotency-key-reused"]);
  assert.strictEqual(db.prepare("SELECT COUNT(*) AS n FROM invites").get().n, 1);

  // A key is one person's: bob's k1 is not alice's.
  const accepted = await call(env, "bob", "POST", `/orgs/acme/invites/${one.body.code}/accept`, { key: "k1" });
  assert.strictEqual(accepted.status, 200);

  // A refusal is not remembered: retried after its cause is fixed, it works.
  const early = await call(env, "alice", "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "reviewer" }, key: "k-role" });
  assert.strictEqual(early.body.error, "not-a-member");
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "carol", role: "developer" } });
  await call(env, "carol", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  const later = await call(env, "alice", "PUT", "/orgs/acme/members/github%3Acarol", { json: { role: "reviewer" }, key: "k-role" });
  assert.strictEqual(later.status, 200);
});

test("a retried candidate upload with its key stores nothing twice", async () => {
  const env = orgEnv();
  await acme(env);
  let puts = 0;
  const put = env.BUNDLES.put;
  // Puts of the bundle itself; the Player's web pack (web/<tag>/<hash>,
  // IC-956) is made once beside it, and only on the first upload.
  env.BUNDLES.put = (...a) => ((puts += String(a[0]).startsWith("web/") ? 0 : 1), put(...a));
  const bytes = fixture("candidate-v1.krate");
  const first = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes, key: "up-1" });
  const again = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes, key: "up-1" });
  assert.deepStrictEqual([first.status, again.status], [201, 201]);
  assert.deepStrictEqual(again.body, first.body);
  assert.strictEqual(puts, 1, "the replay was answered before the store was touched");
  const other = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v2.krate"), key: "up-1" });
  assert.deepStrictEqual([other.status, other.body.error], [409, "idempotency-key-reused"]);
  assert.strictEqual(puts, 1);
});

/* ---- the audit ------------------------------------------------------- */

test("the audit has every step and every denial, chained, with nothing secret in it", async () => {
  const env = orgEnv();
  await acme(env);
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  await call(env, "carol", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
  await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
  await call(env, "stranger", "GET", "/orgs/acme");
  await call(env, null, "GET", `/a/${up.body.digest}`);
  const audit = await call(env, "alice", "GET", "/orgs/acme/audit");
  assert.strictEqual(audit.status, 200);
  assert.strictEqual(audit.body.chain_ok, true);
  const events = audit.body.events;
  const lines = events.map((e) => `${e.actor} ${e.action} ${e.result} ${e.reason || ""}`.trim());
  for (const want of [
    "github:alice org.create allowed",
    "github:alice root-key.register allowed",
    "github:alice release-key.revoke allowed",
    "github:bob invite.accept allowed",
    "github:carol candidate.publish allowed",
    "github:carol candidate.approve denied self-approval",
    "github:bob candidate.approve allowed",
    "github:stranger org.view denied not-a-member",
  ]) {
    assert.ok(lines.includes(want), `${want} in\n${lines.join("\n")}`);
  }
  assert.ok(!events.some((e) => e.actor === null), "a request with no session is never audited");
  assert.deepStrictEqual(Object.keys(events[0]).sort(), ["action", "actor", "at", "hash", "prev_hash", "reason", "result", "seq", "target"]);
  assert.strictEqual(events[0].prev_hash, GENESIS);
  const all = JSON.stringify(events);
  assert.ok(!all.includes("krs_"), "no session token");
  assert.ok(!/[0-9a-f]{64}/.test(all.replace(/"(hash|prev_hash)":"[0-9a-f]{64}"/g, "").replace(/(candidate|release):[0-9a-f]{64}/g, "")), "no key or digest beyond the target it names");
  // The hash is over the canonical event, as the contract says.
  const e = events[3];
  assert.strictEqual(e.hash, sha256HexSync(eventPreimage(e)));
});

test("tampering with one stored event makes chain_ok false", async () => {
  for (const tamper of [
    (db) => db.exec("UPDATE audit SET actor = 'mallory' WHERE seq = 3"),
    (db) => db.exec("UPDATE audit SET result = 'allowed', reason = NULL WHERE result = 'denied'"),
    (db) => db.exec("DELETE FROM audit WHERE seq = 4"),
    // A forger who also recomputes the edited event's own hash still
    // breaks the link to the next one.
    (db) => {
      const row = db.prepare("SELECT * FROM audit WHERE seq = 2").get();
      const edited = { ...row, target: "member:mallory" };
      db.prepare("UPDATE audit SET target = ?, hash = ? WHERE seq = 2").run(edited.target, sha256HexSync(eventPreimage(edited)));
    },
  ]) {
    const env = orgEnv();
    await acme(env);
    await call(env, "stranger", "GET", "/orgs/acme");
    assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/audit")).body.chain_ok, true);
    tamper(env.ORGS.object("acme").ctx.storage.db);
    assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/audit")).body.chain_ok, false, tamper.toString());
  }
});

test("a command that fails part-way leaves nothing behind", async () => {
  const env = orgEnv();
  await acme(env);
  const store = env.ORGS.object("acme");
  const audit = store.audit.bind(store);
  // The audit append is the last write of a removal; make it throw.
  store.audit = (...a) => {
    if (a[1] === "member.remove") throw new Error("disk on fire");
    return audit(...a);
  };
  const failed = await call(env, "alice", "DELETE", "/orgs/acme/members/github%3Abob");
  assert.strictEqual(failed.status, 500, "our fault, said as ours");
  store.audit = audit;
  const bob = (await call(env, "alice", "GET", "/orgs/acme")).body.members.find((m) => m.display === "bob");
  assert.strictEqual(bob.state, "active", "the removal rolled back with its audit event");
});
