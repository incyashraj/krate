/* SCIM 2.0 provisioning (IC-955): the real worker and the real Org store
 * over SQLite, driven with the request bodies Okta and Microsoft Entra
 * document for SCIM 2.0. The vectors are copied from:
 *
 *   Okta:  https://developer.okta.com/docs/api/openapi/okta-scim/guides/scim-20
 *          (create user, deactivate by `replace` with a value object,
 *          `filter=userName eq`, create group, add members, remove one
 *          member by `members[value eq "..."]`, rename)
 *   Entra: https://learn.microsoft.com/en-us/entra/identity/app-provisioning/use-scim-to-provision-users-and-groups
 *          (create user, duplicate create is 409, `Replace` of path
 *          `active`, group `Add` / `Remove` with a value list, delete)
 *
 * Only the identifiers are changed: Okta's ids are whatever this hub
 * answered, and Entra's sample userName is a UPN at corp.example, because
 * Krate's userName is an email address (the contract's rule).
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-scim.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, fixture, worker } from "./org-tools.mjs";

const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const PATCH = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

// ---- Okta's documented bodies ------------------------------------------
const oktaCreateUser = (userName) => ({
  schemas: ["urn:ietf:params:scim:schemas:core:2.0:User"],
  userName,
  name: { givenName: "Test", familyName: "User" },
  emails: [{ primary: true, value: userName, type: "work" }],
  displayName: "Test User",
  locale: "en-US",
  externalId: "00ujl29u0le5T6Aj10h7",
  groups: [],
  password: "1mz050nq",
  active: true,
});
const oktaDeactivate = { schemas: [PATCH], Operations: [{ op: "replace", value: { active: false } }] };
const oktaActivate = { schemas: [PATCH], Operations: [{ op: "replace", value: { active: true } }] };
const oktaCreateGroup = { schemas: ["urn:ietf:params:scim:schemas:core:2.0:Group"], displayName: "Test SCIMv2", members: [] };
const oktaAddMembers = (id, display) => ({ schemas: [PATCH], Operations: [{ op: "add", path: "members", value: [{ value: id, display }] }] });
const oktaRemoveMember = (id) => ({ schemas: [PATCH], Operations: [{ op: "remove", path: `members[value eq "${id}"]` }] });
const oktaRename = (id, displayName) => ({ schemas: [PATCH], Operations: [{ op: "replace", value: { id, displayName } }] });

// ---- Entra's documented bodies -----------------------------------------
const entraCreateUser = (userName) => ({
  schemas: ["urn:ietf:params:scim:schemas:core:2.0:User", "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User"],
  externalId: "0a21f0f2-8d2a-4f8e-bf98-7363c4aed4ef",
  userName,
  active: true,
  emails: [{ primary: true, type: "work", value: userName }],
  meta: { resourceType: "User" },
  name: { formatted: "givenName familyName", familyName: "familyName", givenName: "givenName" },
  roles: [],
});
const entraDisable = { Operations: [{ op: "Replace", path: "active", value: false }], schemas: [PATCH] };
const entraCreateGroup = {
  schemas: ["urn:ietf:params:scim:schemas:core:2.0:Group", "http://schemas.microsoft.com/2006/11/ResourceManagement/ADSCIM/2.0/Group"],
  externalId: "8aa1a0c0-c4c3-4bc0-b4a5-2ef676900159",
  displayName: "displayName",
  meta: { resourceType: "Group" },
};
const entraAdd = (id) => ({ schemas: [PATCH], Operations: [{ op: "Add", path: "members", value: [{ $ref: null, value: id }] }] });
const entraRemove = (id) => ({ schemas: [PATCH], Operations: [{ op: "Remove", path: "members", value: [{ $ref: null, value: id }] }] });

// ---- helpers ------------------------------------------------------------

/* One request to /scim/v2/acme as the identity provider. */
async function scim(env, token, method, path, payload, slug = "acme") {
  const headers = { accept: "application/scim+json" };
  if (token) headers.authorization = `Bearer ${token}`;
  if (payload !== undefined) headers["content-type"] = "application/scim+json";
  const res = await worker.fetch(
    new Request(`https://hub.example/scim/v2/${slug}${path}`, { method, headers, body: payload === undefined ? undefined : JSON.stringify(payload) }),
    env,
  );
  const text = await res.text();
  return { status: res.status, body: text ? JSON.parse(text) : null, headers: res.headers };
}

async function world() {
  const env = orgEnv({ ORG_LEASE_KEY: SEED });
  await acme(env);
  const t = await call(env, "alice", "POST", "/orgs/acme/scim/tokens");
  assert.strictEqual(t.status, 201, JSON.stringify(t.body));
  return { env, token: t.body.token, tokenId: t.body.id };
}

async function release(env, name = "candidate-v1.krate") {
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  const ok = await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
  assert.strictEqual(ok.status, 200, JSON.stringify(ok.body));
  return up.body.digest;
}

/* What a device does when it opens a private app: the check-in look. */
const checkIn = (env, who, digest) => call(env, who, "GET", `/a/${digest}`, { headers: { accept: "application/json" } });
const events = async (env) => (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events;
const store = (env) => env.ORGS.object("acme");
const dump = (env) => {
  const db = store(env).sql;
  const tables = db.exec("SELECT name FROM sqlite_master WHERE type = 'table'").toArray().map((r) => r.name);
  return tables.map((t) => JSON.stringify(db.exec(`SELECT * FROM ${t}`).toArray())).join("\n");
};

// ---- the token ----------------------------------------------------------

test("an owner makes a SCIM token: shown once, stored only hashed, and nobody else can make or see one", async () => {
  const { env, token, tokenId } = await world();
  assert.match(token, /^kscim_[A-Za-z0-9_-]{43}$/);
  const settings = await call(env, "alice", "GET", "/orgs/acme/scim");
  assert.strictEqual(settings.status, 200);
  assert.strictEqual(settings.body.default_role, "recipient");
  assert.deepStrictEqual(settings.body.tokens.map((t) => t.id), [tokenId]);
  assert.strictEqual(settings.body.base_url, "https://hub.example/scim/v2/acme");
  assert.ok(!JSON.stringify(settings.body).includes(token), "the token is never shown again");
  assert.ok(!dump(env).includes(token), "the token is not in the store");
  assert.ok(!dump(env).includes(token.slice(10)), "no part past the hint is in the store");
  // Not an admin, not a reviewer, not a recipient.
  const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: "erin", role: "admin" } });
  await call(env, "erin", "POST", `/orgs/acme/invites/${inv.body.code}/accept`);
  for (const who of ["erin", "bob", "dan"]) {
    assert.deepStrictEqual([(await call(env, who, "POST", "/orgs/acme/scim/tokens")).status, who], [403, who]);
    assert.deepStrictEqual([(await call(env, who, "GET", "/orgs/acme/scim")).status, who], [403, who]);
  }
  const audit = await events(env);
  assert.ok(audit.some((e) => e.action === "scim.token.create" && e.actor === "github:alice" && e.result === "allowed"));
  assert.ok(!JSON.stringify(audit).includes(token));
});

test("no token, a wrong one, another organisation's, and a revoked one are each refused -- and none of them grows the audit", async () => {
  const { env, token, tokenId } = await world();
  assert.strictEqual((await scim(env, token, "GET", "/Users")).status, 200);
  const before = (await events(env)).length;
  assert.strictEqual((await scim(env, null, "GET", "/Users")).status, 401);
  assert.strictEqual((await scim(env, "kscim_" + "A".repeat(43), "GET", "/Users")).status, 401);
  assert.strictEqual((await scim(env, "gho_alice", "GET", "/Users")).status, 401, "a person's sign-in is not a SCIM token");
  // Another organisation's token does not open acme.
  await call(env, "frank", "POST", "/orgs", { json: { slug: "beta", name: "Beta" } });
  const other = await call(env, "frank", "POST", "/orgs/beta/scim/tokens");
  assert.strictEqual((await scim(env, other.body.token, "GET", "/Users")).status, 401);
  assert.strictEqual((await scim(env, other.body.token, "GET", "/Users", undefined, "beta")).status, 200);
  // Revoked: refused at its next request, in words that say what to do.
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/scim/tokens/${tokenId}`)).status, 204);
  const revoked = await scim(env, token, "POST", "/Users", oktaCreateUser("erin@corp.example"));
  assert.strictEqual(revoked.status, 401);
  assert.match(revoked.body.detail, /revoked -- an owner of acme makes a new one/);
  assert.deepStrictEqual(revoked.body.schemas, ["urn:ietf:params:scim:api:messages:2.0:Error"]);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/scim")).body.tokens.length, 0);
  const after = await events(env);
  assert.strictEqual(after.length, before + 1, "only the owner's revoke was audited");
  assert.strictEqual(after[after.length - 1].action, "scim.token.revoke");
});

test("refused SCIM requests are rate limited", async () => {
  const { env } = await world();
  const statuses = [];
  for (let i = 0; i < 22; i++) statuses.push((await scim(env, "kscim_" + "B".repeat(43), "GET", "/Users")).status);
  assert.deepStrictEqual(statuses.slice(0, 20), Array(20).fill(401));
  assert.strictEqual(statuses[21], 429);
});

// ---- Okta ---------------------------------------------------------------

test("Okta: find by userName, create, deactivate -- the person's next check-in is refused and their leases end -- and activate again", async () => {
  const { env, token } = await world();
  const digest = await release(env);
  const email = "erin@corp.example";

  // Okta asks first whether the person exists.
  const q = await scim(env, token, "GET", `/Users?filter=userName%20eq%20%22erin%40corp.example%22&startIndex=1&count=100`);
  assert.strictEqual(q.status, 200);
  assert.strictEqual(q.headers.get("content-type"), "application/scim+json");
  assert.deepStrictEqual([q.body.totalResults, q.body.Resources.length], [0, 0]);

  // Before: not a member, so the check-in is refused.
  assert.strictEqual((await checkIn(env, email, digest)).body.error, "not-a-member");

  const made = await scim(env, token, "POST", "/Users", oktaCreateUser(email));
  assert.strictEqual(made.status, 201, JSON.stringify(made.body));
  assert.strictEqual(made.body.userName, email);
  assert.strictEqual(made.body.active, true);
  assert.strictEqual(made.body.externalId, "00ujl29u0le5T6Aj10h7");
  assert.deepStrictEqual(made.body.name, { givenName: "Test", familyName: "User", formatted: "Test User" });
  assert.match(made.body.meta.location, /^https:\/\/hub\.example\/scim\/v2\/acme\/Users\/u[0-9a-f]{24}$/);
  assert.strictEqual(made.body.password, undefined);
  assert.ok(!dump(env).includes("1mz050nq"), "Okta's password is never stored");
  const id = made.body.id;

  // A member with the default role, principal email:<userName>, no invitation.
  const people = (await call(env, "alice", "GET", "/orgs/acme")).body.members;
  const erin = people.find((m) => m.principal === `email:${email}`);
  assert.deepStrictEqual([erin.role, erin.state, erin.added_by], ["recipient", "active", "scim"]);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.members.length, 5);

  // Found by filter now, and by id.
  const found = await scim(env, token, "GET", `/Users?filter=${encodeURIComponent(`userName eq "ERIN@corp.example"`)}`);
  assert.deepStrictEqual(found.body.Resources.map((u) => u.id), [id]);
  assert.strictEqual((await scim(env, token, "GET", `/Users/${id}`)).body.userName, email);

  // She opens the app, and holds offline access.
  assert.strictEqual((await checkIn(env, email, digest)).status, 200);
  assert.strictEqual((await call(env, email, "GET", `/orgs/acme/leases/${digest}`)).status, 200);
  assert.deepStrictEqual((await call(env, email, "GET", "/orgs")).body.map((o) => [o.slug, o.role]), [["acme", "recipient"]]);

  // Okta deactivates her.
  const off = await scim(env, token, "PATCH", `/Users/${id}`, oktaDeactivate);
  assert.strictEqual(off.status, 200, JSON.stringify(off.body));
  assert.strictEqual(off.body.active, false);
  const refused = await checkIn(env, email, digest);
  assert.deepStrictEqual([refused.status, refused.body.error], [403, "removed"]);
  assert.strictEqual((await call(env, email, "GET", `/orgs/acme/leases/${digest}`)).body.error, "removed");
  assert.strictEqual((await call(env, email, "GET", `/a/${digest}`)).status, 403, "and the bytes");
  assert.deepStrictEqual((await call(env, email, "GET", "/orgs")).body, []);
  const audit = await events(env);
  const removal = audit.find((e) => e.action === "member.remove" && e.target === `member:email:${email}`);
  assert.deepStrictEqual([removal.actor, removal.result], ["scim", "allowed"]);
  assert.ok(audit.some((e) => e.action === "member.add" && e.actor === "scim" && e.target === `member:email:${email}`));
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/audit")).body.chain_ok, true);
  // The directory still lists her, inactive.
  assert.strictEqual((await scim(env, token, "GET", `/Users/${id}`)).body.active, false);

  // Okta activates her again: back, with the role she had.
  const on = await scim(env, token, "PATCH", `/Users/${id}`, oktaActivate);
  assert.strictEqual(on.body.active, true);
  assert.strictEqual((await checkIn(env, email, digest)).status, 200);
});

test("Okta PUT replaces the profile but never the address; userName changes are refused in words", async () => {
  const { env, token } = await world();
  const made = await scim(env, token, "POST", "/Users", oktaCreateUser("erin@corp.example"));
  const put = await scim(env, token, "PUT", `/Users/${made.body.id}`, { ...oktaCreateUser("erin@corp.example"), displayName: "Erin E", name: { givenName: "Erin", familyName: "E" } });
  assert.strictEqual(put.status, 200, JSON.stringify(put.body));
  assert.strictEqual(put.body.displayName, "Erin E");
  const moved = await scim(env, token, "PUT", `/Users/${made.body.id}`, oktaCreateUser("erin.new@corp.example"));
  assert.deepStrictEqual([moved.status, moved.body.scimType], [400, "mutability"]);
  assert.match(moved.body.detail, /deactivate this user and create one with the new address/);
  // Inactive by PUT is a removal like any other.
  const off = await scim(env, token, "PUT", `/Users/${made.body.id}`, { ...oktaCreateUser("erin@corp.example"), active: false });
  assert.strictEqual(off.body.active, false);
  assert.strictEqual((await call(env, "erin@corp.example", "GET", "/orgs/acme")).body.error, "removed");
});

test("Okta groups: create, add members, rename, remove one member -- an audience limited to the group follows the directory", async () => {
  const { env, token } = await world();
  const digest = await release(env);
  const email = "erin@corp.example";
  const u = (await scim(env, token, "POST", "/Users", oktaCreateUser(email))).body;

  const g = await scim(env, token, "POST", "/Groups", oktaCreateGroup);
  assert.strictEqual(g.status, 201, JSON.stringify(g.body));
  assert.deepStrictEqual([g.body.displayName, g.body.members], ["Test SCIMv2", []]);
  // It is one of the organisation's groups, named for audiences.
  const groups = (await call(env, "alice", "GET", "/orgs/acme/groups")).body.groups;
  assert.deepStrictEqual(groups.map((x) => x.name), ["test-scimv2"]);
  assert.strictEqual((await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { groups: ["test-scimv2"] } })).status, 200);
  assert.strictEqual((await checkIn(env, email, digest)).body.error, "not-in-audience");

  const added = await scim(env, token, "PATCH", `/Groups/${g.body.id}`, oktaAddMembers(u.id, email));
  assert.strictEqual(added.status, 204);
  assert.deepStrictEqual((await scim(env, token, "GET", `/Groups/${g.body.id}`)).body.members, [{ value: u.id, display: email }]);
  assert.strictEqual((await checkIn(env, email, digest)).status, 200);

  // Renamed in Okta: shown differently, the audience's name unchanged.
  assert.strictEqual((await scim(env, token, "PATCH", `/Groups/${g.body.id}`, oktaRename(g.body.id, "Finance"))).status, 204);
  assert.strictEqual((await scim(env, token, "GET", `/Groups/${g.body.id}`)).body.displayName, "Finance");
  assert.deepStrictEqual((await call(env, "alice", "GET", "/orgs/acme/groups")).body.groups.map((x) => x.name), ["test-scimv2"]);
  assert.strictEqual((await checkIn(env, email, digest)).status, 200);

  // Okta's remove names the member in a filter path.
  assert.strictEqual((await scim(env, token, "PATCH", `/Groups/${g.body.id}`, oktaRemoveMember(u.id))).status, 204);
  assert.strictEqual((await checkIn(env, email, digest)).body.error, "not-in-audience");
  const audit = await events(env);
  for (const action of ["group.create", "group.add", "group.remove"]) assert.ok(audit.some((e) => e.action === action && e.actor === "scim"), action);
});

// ---- Entra --------------------------------------------------------------

test("Entra: create, duplicate is 409, disable by Replace of path active, delete -- each with Entra's shapes", async () => {
  const { env, token } = await world();
  const digest = await release(env);
  const upn = "Test_User_00aa00aa@corp.example";
  const made = await scim(env, token, "POST", "/Users", entraCreateUser(upn));
  assert.strictEqual(made.status, 201, JSON.stringify(made.body));
  assert.strictEqual(made.body.userName, upn.toLowerCase());
  const dup = await scim(env, token, "POST", "/Users", entraCreateUser(upn));
  assert.deepStrictEqual([dup.status, dup.body.scimType, dup.body.status], [409, "uniqueness", "409"]);
  // Entra filters on its joining property, and on externalId.
  const f = await scim(env, token, "GET", `/Users?filter=${encodeURIComponent(`userName eq "${upn}"`)}`);
  assert.deepStrictEqual(f.body.Resources.map((r) => r.id), [made.body.id]);
  const ext = await scim(env, token, "GET", `/Users?filter=${encodeURIComponent("externalId eq 0a21f0f2-8d2a-4f8e-bf98-7363c4aed4ef")}`);
  assert.deepStrictEqual(ext.body.Resources.map((r) => r.id), [made.body.id]);
  // A user that does not exist is an empty list, and a 404 by id.
  assert.strictEqual((await scim(env, token, "GET", `/Users?filter=${encodeURIComponent('userName eq "non-existent user"')}`)).body.totalResults, 0);
  const missing = await scim(env, token, "GET", "/Users/5171a35d82074e068ce2");
  assert.deepStrictEqual([missing.status, missing.body.status], [404, "404"]);

  assert.strictEqual((await checkIn(env, upn.toLowerCase(), digest)).status, 200);
  const off = await scim(env, token, "PATCH", `/Users/${made.body.id}`, entraDisable);
  assert.strictEqual(off.body.active, false);
  assert.strictEqual((await checkIn(env, upn.toLowerCase(), digest)).body.error, "removed");
  // "False" as a string, which Entra has sent, means false too.
  const back = await scim(env, token, "PATCH", `/Users/${made.body.id}`, { schemas: [PATCH], Operations: [{ op: "Replace", path: "active", value: "True" }] });
  assert.strictEqual(back.body.active, true);

  // Delete: removed, gone from the directory's view, refused at check-in.
  assert.strictEqual((await scim(env, token, "DELETE", `/Users/${made.body.id}`)).status, 204);
  assert.strictEqual((await scim(env, token, "GET", `/Users/${made.body.id}`)).status, 404);
  assert.strictEqual((await scim(env, token, "GET", "/Users")).body.Resources.some((r) => r.id === made.body.id), false);
  assert.strictEqual((await checkIn(env, upn.toLowerCase(), digest)).body.error, "removed");
  // The organisation's own record of them stays (R12).
  const m = (await call(env, "alice", "GET", "/orgs/acme")).body.members.find((x) => x.principal === `email:${upn.toLowerCase()}`);
  assert.deepStrictEqual([m.state, m.removed_by], ["removed", "scim"]);
  // Provisioned again: the directory brings back whom it removed.
  const again = await scim(env, token, "POST", "/Users", entraCreateUser(upn));
  assert.deepStrictEqual([again.status, again.body.active], [201, true]);
});

test("Entra groups: create, Add and Remove members with a value list, duplicate is 409, delete", async () => {
  const { env, token } = await world();
  const u = (await scim(env, token, "POST", "/Users", entraCreateUser("dana@corp.example"))).body;
  const g = await scim(env, token, "POST", "/Groups", entraCreateGroup);
  assert.strictEqual(g.status, 201);
  assert.strictEqual(g.body.externalId, "8aa1a0c0-c4c3-4bc0-b4a5-2ef676900159");
  assert.strictEqual((await scim(env, token, "POST", "/Groups", entraCreateGroup)).status, 409);
  const byName = await scim(env, token, "GET", `/Groups?excludedAttributes=members&filter=${encodeURIComponent('displayName eq "displayName"')}`);
  assert.deepStrictEqual(byName.body.Resources.map((r) => [r.id, r.members]), [[g.body.id, undefined]]);
  assert.strictEqual((await scim(env, token, "PATCH", `/Groups/${g.body.id}`, entraAdd(u.id))).status, 204);
  const groups = async () => (await call(env, "alice", "GET", "/orgs/acme/groups")).body.groups;
  assert.deepStrictEqual((await groups()).map((x) => [x.name, x.members]), [["displayname", ["email:dana@corp.example"]]]);
  // An id that is nobody is refused, and nothing of that request is kept.
  const bad = await scim(env, token, "PATCH", `/Groups/${g.body.id}`, { schemas: [PATCH], Operations: [entraRemove(u.id).Operations[0], { op: "Add", path: "members", value: [{ value: "unone" }] }] });
  assert.deepStrictEqual([bad.status, bad.body.scimType], [400, "invalidValue"]);
  assert.deepStrictEqual((await groups())[0].members, ["email:dana@corp.example"], "the whole PATCH rolled back");
  assert.strictEqual((await scim(env, token, "PATCH", `/Groups/${g.body.id}`, entraRemove(u.id))).status, 204);
  assert.deepStrictEqual((await groups())[0].members, []);
  assert.strictEqual((await scim(env, token, "DELETE", `/Groups/${g.body.id}`)).status, 204);
  assert.deepStrictEqual(await groups(), []);
  assert.strictEqual((await scim(env, token, "GET", `/Groups/${g.body.id}`)).status, 404);
});

test("a userName that is not an email address is refused with the mapping to fix", async () => {
  const { env, token } = await world();
  const r = await scim(env, token, "POST", "/Users", entraCreateUser("Test_User_00aa00aa-bb11-cc22-dd33-44ee44ee44ee"));
  assert.deepStrictEqual([r.status, r.body.scimType], [400, "invalidValue"]);
  assert.match(r.body.detail, /userPrincipalName or mail/);
  const filter = await scim(env, token, "GET", `/Users?filter=${encodeURIComponent('name.givenName sw "a"')}`);
  assert.deepStrictEqual([filter.status, filter.body.scimType], [400, "invalidFilter"]);
});

// ---- the rules above the directory -------------------------------------

test("the directory lists invited members, never brings back someone an owner removed, and never removes the last owner", async () => {
  const env = orgEnv();
  // An organisation whose only owner signs in by email.
  assert.strictEqual((await call(env, "olga@corp.example", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } })).status, 201);
  const token = (await call(env, "olga@corp.example", "POST", "/orgs/acme/scim/tokens")).body.token;
  const inv = await call(env, "olga@corp.example", "POST", "/orgs/acme/invites", { json: { email: "pat@corp.example", role: "developer" } });
  assert.strictEqual((await call(env, "pat@corp.example", "POST", `/orgs/acme/invites/${inv.body.code}/accept`)).status, 200);

  const list = await scim(env, token, "GET", "/Users");
  assert.deepStrictEqual(list.body.Resources.map((r) => r.userName).sort(), ["olga@corp.example", "pat@corp.example"]);
  // An invited member is found and linked, not created twice.
  assert.strictEqual((await scim(env, token, "POST", "/Users", oktaCreateUser("pat@corp.example"))).status, 409);

  const olga = list.body.Resources.find((r) => r.userName === "olga@corp.example");
  const last = await scim(env, token, "PATCH", `/Users/${olga.id}`, oktaDeactivate);
  assert.strictEqual(last.status, 409);
  assert.match(last.body.detail, /last owner/);
  assert.strictEqual((await call(env, "olga@corp.example", "GET", "/orgs/acme")).status, 200, "still an owner");

  // An owner removes pat in Krate; the directory cannot undo it.
  assert.strictEqual((await call(env, "olga@corp.example", "DELETE", `/orgs/acme/members/${encodeURIComponent("email:pat@corp.example")}`)).status, 204);
  const pat = list.body.Resources.find((r) => r.userName === "pat@corp.example");
  const undo = await scim(env, token, "PATCH", `/Users/${pat.id}`, oktaActivate);
  assert.strictEqual(undo.status, 409);
  assert.match(undo.body.detail, /removed from acme by olga@corp.example in Krate, so the directory cannot bring them back/);
  assert.strictEqual((await call(env, "pat@corp.example", "GET", "/orgs/acme")).body.error, "removed");
});

test("the SCIM default role is the owner's choice, never owner or admin; a closing organisation takes nobody new", async () => {
  const { env, token } = await world();
  for (const role of ["owner", "admin", "boss"]) {
    const r = await call(env, "alice", "PUT", "/orgs/acme/scim", { json: { default_role: role } });
    assert.deepStrictEqual([r.status, r.body.error, role], [403, "role-cannot", role]);
  }
  assert.strictEqual((await call(env, "bob", "PUT", "/orgs/acme/scim", { json: { default_role: "developer" } })).status, 403);
  const set = await call(env, "alice", "PUT", "/orgs/acme/scim", { json: { default_role: "developer" } });
  assert.deepStrictEqual([set.status, set.body.default_role], [200, "developer"]);
  await scim(env, token, "POST", "/Users", oktaCreateUser("erin@corp.example"));
  const erin = (await call(env, "alice", "GET", "/orgs/acme")).body.members.find((m) => m.principal === "email:erin@corp.example");
  assert.strictEqual(erin.role, "developer");

  // Closing (a sole owner, confirmed): nobody new, removals still work.
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme" } })).status, 201);
  const late = await scim(env, token, "POST", "/Users", oktaCreateUser("late@corp.example"));
  assert.strictEqual(late.status, 409);
  assert.match(late.body.detail, /acme is closing/);
  const id = (await scim(env, token, "GET", `/Users?filter=${encodeURIComponent('userName eq "erin@corp.example"')}`)).body.Resources[0].id;
  assert.strictEqual((await scim(env, token, "PATCH", `/Users/${id}`, oktaDeactivate)).body.active, false);
});

test("ServiceProviderConfig answers only a good token", async () => {
  const { env, token } = await world();
  assert.strictEqual((await scim(env, null, "GET", "/ServiceProviderConfig")).status, 401);
  const c = await scim(env, token, "GET", "/ServiceProviderConfig");
  assert.deepStrictEqual([c.status, c.body.patch.supported, c.body.bulk.supported], [200, true, false]);
  // The book publishes organisations.md as organisations.html; the section an
  // IT admin needs is #provisioning. /docs/organisations/ is a 404 (K-1118).
  assert.strictEqual(c.body.documentationUri, "https://krate.tech/docs/organisations.html#provisioning");
});
