/* Company connections, hub side (IC-952, ORG-FIRST-PLAN.md "IC-952
 * contract"): an owner or admin sets up an API's address and credential
 * once; the credential is write-only and end-to-end encrypted (F-316):
 * sealed to the organisation's key where it is typed, kept here as it came,
 * never in an answer, an audit event or the store in the clear; a device
 * gets the sealed blob only for a member inside the connection's audience,
 * for a released (not withdrawn) version of an app the connection allows
 * that asked for it; each use is counted per day into the audit; a key
 * rotation seals every credential again or does not happen; and a
 * credential kept the old way is deleted and must be entered again.
 *
 * The credentials are sealed by the dashboard's own code (org-seal-tools),
 * so the hub sees exactly what a browser sends. The bundles are real
 * signed and marked fixtures (fixtures/org-connections, written by
 * crates/validator-wasm/tests/org_fixtures.rs), judged by the real
 * validator: com.acme.ledger 1.0.0 asks for net.connection:finance-api,
 * 1.0.1 does not, and com.acme.other asks too.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-connections.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";
import { device, register, memberDevice, recovery, b64 } from "./org-keys-tools.mjs";
import { orgKey, sealed, opened } from "./org-seal-tools.mjs";
import { normalBase, normalAllow, sealedCredentialOf, normalApi, MAX_API } from "../src/org-connections.js";

const here = dirname(fileURLToPath(import.meta.url));
const FIX = join(here, "fixtures", "org-connections");
const fixture = (name) => new Uint8Array(readFileSync(join(FIX, name)));
const KEYS = JSON.parse(readFileSync(join(FIX, "keys.json"), "utf8"));
const SECRET = "Bearer sk-hub-test-only-5f4e3d2c1b0a99887766";
const ROTATED = "Bearer sk-hub-test-rotated-0011223344556677";
const BASE = "https://api.acme.example/finance";
const DAY = 24 * 60 * 60;
const pair = (r) => [r.status, r.body && r.body.error];
const enc = encodeURIComponent;

/* acme's organisation key, as its members' devices hold it. */
const KEY = await orgKey();
const CRED = await sealed(KEY, SECRET, { base: BASE });

const finance = (extra = {}) => ({
  name: "finance-api",
  base: BASE,
  credential: CRED,
  allow: { methods: ["GET"], paths: ["/invoices"] },
  ...extra,
});

async function release(env, name) {
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  const ok = await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
  assert.strictEqual(ok.status, 200, JSON.stringify(ok.body));
  return up.body.digest;
}

/* acme with its own root (the connection fixtures'), alice owner, erin
 * admin, bob reviewer, carol developer, dan recipient, the three apps
 * released, and its key (version 1, KEY) made on Alice's device. No
 * ORG_SECRET_KEY: connections need none. */
async function world({ keyed = true } = {}) {
  const env = orgEnv();
  const ok = (r, what) => {
    if (r.status >= 300) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body)}`);
    return r;
  };
  ok(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
  ok(await call(env, "alice", "PUT", "/orgs/acme/root-key", { json: { public_key: KEYS.root } }), "root");
  for (const [login, role] of [["erin", "admin"], ["bob", "reviewer"], ["carol", "developer"], ["dan", "recipient"]]) {
    const inv = ok(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: login, role } }), `invite ${login}`);
    ok(await call(env, login, "POST", `/orgs/acme/invites/${inv.body.code}/accept`), `accept ${login}`);
  }
  const ledger = await release(env, "ledger.krate");
  const plain = await release(env, "ledger-plain.krate");
  const other = await release(env, "other.krate");
  const alice = keyed ? await initKey(env) : null;
  return { env, ledger, plain, other, alice };
}

/* Alice's device makes acme's key: its public half is KEY's. */
async function initKey(env) {
  const alice = device();
  await register(env, "alice", alice, "Alice's MacBook");
  const r = await call(env, "alice", "POST", "/orgs/acme/keys/init", {
    json: { device_id: alice.id, public_key: KEY.publicHex, envelope: b64(300), recovery: recovery() },
  });
  assert.strictEqual(r.status, 201, JSON.stringify(r.body));
  return alice;
}

const use = (env, who, digest, name = "finance-api") => call(env, who, "GET", `/orgs/acme/connections/${name}/credential/${digest}`);
const events = async (env) => (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events;
const store = (env) => env.ORGS.object("acme").ctx.storage.db;
/* Every value in every table of acme's store, as one string. */
function storeText(env) {
  const db = store(env);
  const tables = db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all().map((t) => t.name);
  return tables.map((t) => JSON.stringify(db.prepare(`SELECT * FROM ${t}`).all())).join("\n");
}
const noSecretIn = (text, what) => {
  for (const s of [SECRET, ROTATED]) {
    assert.ok(!text.includes(s.slice(7)), `${what} holds a credential in the clear`);
    assert.ok(!text.includes(Buffer.from(s).toString("base64").slice(0, 24)), `${what} holds a credential's base64`);
  }
};

/* ---- setting one up ----------------------------------------------------- */

test("only an owner or admin sets one up, the hub keeps the sealed blob as it came, and the credential never comes back", async () => {
  const { env } = await world();
  for (const who of ["bob", "carol", "dan"]) {
    assert.deepStrictEqual(pair(await call(env, who, "POST", "/orgs/acme/connections", { json: finance() })), [403, "role-cannot"], who);
  }
  assert.deepStrictEqual(pair(await call(env, "stranger", "POST", "/orgs/acme/connections", { json: finance() })), [403, "not-a-member"]);
  const made = await call(env, "erin", "POST", "/orgs/acme/connections", { json: finance() });
  assert.strictEqual(made.status, 201, JSON.stringify(made.body));
  const c = made.body.connection;
  assert.deepStrictEqual([c.name, c.base, c.allow, c.audience, c.apps], ["finance-api", BASE, { methods: ["GET"], paths: ["/invoices"] }, null, null]);
  assert.deepStrictEqual([c.credential.kind, c.credential.header, c.credential.set_by, c.credential.state, c.credential.version], ["header", "Authorization", "github:erin", "sealed", 1]);
  assert.ok(!JSON.stringify(made.body).includes(CRED.sealed), "the answer does not even carry the blob");
  assert.deepStrictEqual(pair(await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() })), [409, "connection-taken"]);

  // Who sees what exists: the people who build and approve the apps.
  for (const who of ["alice", "erin", "carol", "bob"]) {
    const list = await call(env, who, "GET", "/orgs/acme/connections");
    assert.strictEqual(list.status, 200, who);
    assert.strictEqual(list.body.connections[0].name, "finance-api");
    noSecretIn(JSON.stringify(list.body), who);
  }
  assert.deepStrictEqual(pair(await call(env, "dan", "GET", "/orgs/acme/connections")), [403, "role-cannot"]);
  assert.deepStrictEqual(pair(await call(env, null, "GET", "/orgs/acme/connections")), [401, "not-signed-in"]);

  // The store holds the blob the browser sealed, byte for byte, and its
  // key version; nothing it could open.
  noSecretIn(storeText(env), "the store");
  const row = store(env).prepare("SELECT sealed, credential_v FROM connections").get();
  assert.deepStrictEqual([row.sealed, row.credential_v], [CRED.sealed, 1]);
  assert.strictEqual(await opened(KEY, { header: "Authorization", sealed: row.sealed }, { base: BASE }), SECRET, "it opens with the organisation key, which the hub never has");
  const audit = (await events(env)).filter((e) => e.action.startsWith("connection."));
  assert.deepStrictEqual(audit.map((e) => [e.actor, e.action, e.target, e.result]), [
    ["github:bob", "connection.manage", "connection:finance-api", "denied"],
    ["github:carol", "connection.manage", "connection:finance-api", "denied"],
    ["github:dan", "connection.manage", "connection:finance-api", "denied"],
    ["github:stranger", "connection.manage", "connection:finance-api", "denied"],
    ["github:erin", "connection.create", "connection:finance-api", "allowed"],
    ["github:alice", "connection.manage", "connection:finance-api", "denied"],
    ["github:dan", "connection.view", "org:acme", "denied"],
  ]);
});

test("a connection must be complete, exact, and safe to send; a credential in the clear is never kept", async () => {
  const { env } = await world();
  const refused = async (body, words) => {
    const r = await call(env, "alice", "POST", "/orgs/acme/connections", { json: { ...finance(), ...body } });
    assert.strictEqual(r.status, 400, `${JSON.stringify(body)} -> ${r.status} ${JSON.stringify(r.body)}`);
    assert.strictEqual(r.body.error, "connection-invalid");
    if (words) assert.match(r.body.message, words);
    noSecretIn(JSON.stringify(r.body), "a refusal");
  };
  await refused({ name: "Finance API" }, /lower-case/);
  await refused({ base: "http://api.acme.example/finance" }, /https/);
  await refused({ base: "https://user:pw@api.acme.example/x" }, /user or password/);
  await refused({ base: "https://api.acme.example/x?key=1" }, /no \? or #/);
  await refused({ base: "https://api.acme.example/a/../b" });
  await refused({ base: "not a url" }, /full URL/);
  // Sealed for an address the hub would keep differently: it would not open.
  await refused({ base: "https://API.acme.example/finance/" }, /send it as https:\/\/api\.acme\.example\/finance/);
  await refused({ credential: { ...CRED, header: "Host" } }, /connection itself sets/);
  await refused({ credential: { ...CRED, header: "Bad Name" } }, /one word/);
  await refused({ credential: { ...CRED, v: 0 } }, /version/);
  await refused({ credential: { ...CRED, sealed: "not base64!" } }, /base64/);
  await refused({ credential: { ...CRED, sealed: "QUJD" } }, /base64/);
  await refused({ credential: undefined }, /credential/);
  // A body carrying the value in the clear -- an old page or Krate -- is
  // refused in words, and nothing of it is kept anywhere.
  await refused({ credential: undefined, header: { name: "Authorization", value: SECRET } }, /sealed to the organisation's key/);
  await refused({ credential: undefined, basic: { user: "svc", password: SECRET } }, /update Krate/);
  await refused({ allow: { methods: ["TRACE"] } }, /method is one of/);
  await refused({ allow: { methods: ["GET"], paths: ["/a/../b"] } }, /\.\./);
  await refused({ allow: { methods: ["GET"], paths: ["/a%2fb"] } });
  const group = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ audience: { groups: ["finance"] } }) });
  assert.deepStrictEqual(pair(group), [404, "no-such-group"]);
  const app = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ apps: { ids: ["Not An Id!"] } }) });
  assert.deepStrictEqual(pair(app), [400, "connection-invalid"]);
  // Nothing was made by any of it, and the clear values are nowhere.
  assert.deepStrictEqual((await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections, []);
  noSecretIn(storeText(env), "the store");
  noSecretIn(JSON.stringify(await events(env)), "the audit");

  // The parts on their own.
  assert.strictEqual(normalBase("https://API.Acme.example:8443/finance/").base, "https://api.acme.example:8443/finance");
  assert.ok(normalBase("http://127.0.0.1:9000/x", { PUBLIC_BASE: "https://hub.krate.tech" }).problem, "plain http only on a local proof hub");
  assert.strictEqual(normalBase("http://127.0.0.1:9000/x", { PUBLIC_BASE: "http://127.0.0.1:8787" }).base, "http://127.0.0.1:9000/x");
  assert.ok(normalBase("http://10.0.0.5/x", { PUBLIC_BASE: "http://127.0.0.1:8787" }).problem, "and only to this machine");
  assert.deepStrictEqual(normalAllow(undefined).allow, { methods: ["GET"], paths: ["/"] }, "GET on everything by default");
  assert.deepStrictEqual(normalAllow({ methods: ["post", "get"], paths: ["invoices/", "/rates"] }).allow, { methods: ["GET", "POST"], paths: ["/invoices", "/rates"] });
  assert.deepStrictEqual(sealedCredentialOf({ credential: { ...CRED, kind: "basic", header: "X-Ignored" } }), { kind: "basic", header_name: "Authorization", v: 1, sealed: CRED.sealed }, "basic always goes as Authorization");
  assert.strictEqual(sealedCredentialOf({}), null);
});

test("no organisation key: nothing can be stored; once there is one, ORG_SECRET_KEY plays no part", async () => {
  const { env, ledger } = await world({ keyed: false });
  const r = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  assert.deepStrictEqual(pair(r), [409, "no-org-key"]);
  assert.match(r.body.message, /krate org keys init acme/);
  await initKey(env);
  // Sealed to a version that is not the current one.
  const stale = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ credential: { ...CRED, v: 2 } }) });
  assert.deepStrictEqual(pair(stale), [409, "key-rotated"]);
  assert.match(stale.body.message, /version 1/);
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() })).status, 201);
  delete env.ORG_SECRET_KEY;
  const got = await use(env, "dan", ledger);
  assert.strictEqual(got.status, 200, JSON.stringify(got.body));
  assert.strictEqual(await opened(KEY, got.body.credential, { base: BASE }), SECRET);
});

/* ---- handing it to a device -------------------------------------------- */

test("the sealed credential goes only to a member inside the audience, for a release of an allowed app that asked for it", async () => {
  const { env, ledger, plain, other } = await world();
  await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "finance" } });
  await call(env, "alice", "PUT", "/orgs/acme/groups/finance/members/github%3Adan");
  const made = await call(env, "alice", "POST", "/orgs/acme/connections", {
    json: finance({ audience: { groups: ["finance"] }, apps: { ids: ["com.acme.ledger"] } }),
  });
  assert.strictEqual(made.status, 201, JSON.stringify(made.body));

  // Dan, in finance, opening com.acme.ledger 1.0.0, which asked for it: the
  // blob the browser sealed, unopened, with what it is sealed for.
  const got = await use(env, "dan", ledger);
  assert.strictEqual(got.status, 200, JSON.stringify(got.body));
  assert.deepStrictEqual(got.body, {
    name: "finance-api",
    base: BASE,
    allow: { methods: ["GET"], paths: ["/invoices"] },
    credential: { kind: "header", header: "Authorization", v: 1, sealed: CRED.sealed },
  });
  assert.strictEqual(got.headers.get("cache-control"), "private, no-store");
  assert.strictEqual(await opened(KEY, got.body.credential, { base: BASE }), SECRET, "a device with the key opens it");

  // Each refusal, with its stable reason; past standing, the address so the
  // device can word it, and never the credential.
  const no = async (who, digest, want, name) => {
    const r = await use(env, who, digest, name);
    assert.deepStrictEqual(pair(r), want, `${who}: ${JSON.stringify(r.body)}`);
    assert.ok(!JSON.stringify(r.body).includes(CRED.sealed), `${who} was handed the blob`);
    return r;
  };
  const bob = await no("bob", ledger, [403, "connection-not-in-audience"]);
  assert.strictEqual(bob.body.base, BASE);
  assert.deepStrictEqual(bob.body.groups, ["finance"]);
  assert.match(bob.body.message, /only for the finance group in Acme/);
  const otherApp = await no("dan", other, [403, "app-not-allowed"]);
  assert.match(otherApp.body.message, /add com\.acme\.other/);
  const notAsked = await no("dan", plain, [403, "connection-not-requested"]);
  assert.match(notAsked.body.message, /net\.connection:finance-api/);
  const stranger = await no("stranger", ledger, [403, "not-a-member"]);
  assert.strictEqual(stranger.body.base, undefined, "a stranger learns nothing about the address");
  await no(null, ledger, [401, "not-signed-in"]);
  await no("dan", "0".repeat(64), [404, "digest-mismatch"]);
  await no("dan", ledger, [404, "no-such-connection"], "payroll-api");

  // A waiting candidate is not a release.
  const waiting = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("ledger.krate") });
  assert.strictEqual(waiting.body.state, "released", "the same bytes are the release they already are");

  // The release's own audience applies too.
  await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "ops" } });
  await call(env, "alice", "PUT", `/orgs/acme/releases/${ledger}/audience`, { json: { groups: ["ops"] } });
  await no("dan", ledger, [403, "not-in-audience"]);
  await call(env, "alice", "PUT", `/orgs/acme/releases/${ledger}/audience`, { json: { everyone: true } });
  assert.strictEqual((await use(env, "dan", ledger)).status, 200);

  // Withdrawn: refused at once.
  await call(env, "alice", "POST", `/orgs/acme/releases/${ledger}/withdraw`, { json: { reason: "bad build" } });
  await no("dan", ledger, [410, "release-withdrawn"]);

  // Every refusal is in the audit, and no event carries the credential.
  const audit = await events(env);
  assert.ok(audit.filter((e) => e.action === "connection.use" && e.result === "denied").length >= 6);
  noSecretIn(JSON.stringify(audit), "the audit");
});

test("a removed member gets nothing, and a removed connection is gone with its credential", async () => {
  const { env, ledger } = await world();
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  assert.strictEqual((await use(env, "dan", ledger)).status, 200);
  assert.strictEqual((await call(env, "alice", "DELETE", "/orgs/acme/members/github%3Adan")).status, 204);
  assert.deepStrictEqual(pair(await use(env, "dan", ledger)), [403, "removed"]);

  assert.strictEqual((await use(env, "carol", ledger)).status, 200);
  for (const who of ["carol", "bob"]) {
    assert.deepStrictEqual(pair(await call(env, who, "DELETE", "/orgs/acme/connections/finance-api")), [403, "role-cannot"], who);
  }
  assert.strictEqual((await call(env, "erin", "DELETE", "/orgs/acme/connections/finance-api")).status, 204);
  const gone = await use(env, "carol", ledger);
  assert.deepStrictEqual(pair(gone), [404, "no-such-connection"]);
  assert.match(gone.body.message, /any more/);
  assert.strictEqual(store(env).prepare("SELECT COUNT(*) AS n FROM connections").get().n, 0, "the sealed credential is deleted");
  assert.deepStrictEqual(pair(await call(env, "erin", "DELETE", "/orgs/acme/connections/finance-api")), [404, "no-such-connection"]);
  // Its uses that day were written to the audit as it went.
  const lines = (await events(env)).filter((e) => e.action === "connection.use" && e.result === "allowed");
  assert.deepStrictEqual(lines.map((e) => [e.actor, e.target, e.reason.replace(/\d{4}-\d\d-\d\d/, "DAY")]), [["system:krate", "connection:finance-api", "2 uses on DAY"]]);
});

/* ---- changing one ------------------------------------------------------ */

test("a new credential is the one handed out next; a new address needs a new credential", async () => {
  const { env, ledger } = await world();
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  assert.strictEqual(await opened(KEY, (await use(env, "dan", ledger)).body.credential, { base: BASE }), SECRET);

  const NEW = await sealed(KEY, ROTATED, { base: BASE });
  for (const who of ["carol", "dan"]) {
    const r = await call(env, who, "PUT", "/orgs/acme/connections/finance-api/credential", { json: { credential: NEW } });
    assert.deepStrictEqual(pair(r), [403, "role-cannot"], who);
  }
  const clear = await call(env, "alice", "PUT", "/orgs/acme/connections/finance-api/credential", { json: { header: { name: "Authorization", value: ROTATED } } });
  assert.deepStrictEqual(pair(clear), [400, "connection-invalid"], "a replacement in the clear is refused too");
  const replaced = await call(env, "alice", "PUT", "/orgs/acme/connections/finance-api/credential", { json: { credential: NEW } });
  assert.strictEqual(replaced.status, 200, JSON.stringify(replaced.body));
  assert.strictEqual(await opened(KEY, (await use(env, "dan", ledger)).body.credential, { base: BASE }), ROTATED, "the next run gets the new one");
  noSecretIn(storeText(env), "the store");
  assert.ok(!storeText(env).includes(CRED.sealed), "the old blob is gone");

  // The allow, audience and apps change without the credential.
  const widened = await call(env, "alice", "PUT", "/orgs/acme/connections/finance-api", { json: { allow: { methods: ["GET", "POST"], paths: ["/"] } } });
  assert.strictEqual(widened.status, 200, JSON.stringify(widened.body));
  assert.deepStrictEqual(widened.body.connection.allow, { methods: ["GET", "POST"], paths: ["/"] });
  // Sending it somewhere new does not: the credential is sealed for this
  // address, and an admin who never knew the secret must not be able to
  // point it at a server of theirs.
  const moved = await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", { json: { base: "https://evil.example/collect" } });
  assert.deepStrictEqual(pair(moved), [400, "credential-required"]);
  assert.strictEqual((await use(env, "dan", ledger)).body.base, BASE, "nothing moved");
  const BASE2 = "https://api2.acme.example/finance";
  const movedWith = await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", {
    json: { base: BASE2, credential: await sealed(KEY, "k-new", { header: "X-Api-Key", base: BASE2 }) },
  });
  assert.strictEqual(movedWith.status, 200, JSON.stringify(movedWith.body));
  const now = (await use(env, "dan", ledger)).body;
  assert.deepStrictEqual([now.base, now.credential.header], [BASE2, "X-Api-Key"]);
  assert.strictEqual(await opened(KEY, now.credential, { base: BASE2 }), "k-new");
  // A hub that moved it anyway would hand out a credential that no longer
  // opens: the seal is for the address it was typed for.
  assert.strictEqual(await opened(KEY, now.credential, { base: "https://evil.example/collect" }), null);

  const audit = (await events(env)).filter((e) => e.result === "allowed" && /^connection\.(create|change|credential)/.test(e.action));
  assert.deepStrictEqual(audit.map((e) => [e.actor, e.action, e.reason]), [
    ["github:alice", "connection.create", BASE],
    ["github:alice", "connection.credential", null],
    ["github:alice", "connection.change", "allow GET,POST /"],
    ["github:erin", "connection.change", `address ${BASE2}; credential replaced`],
  ]);
});

test("the same Idempotency-Key is the same request", async () => {
  const { env } = await world();
  const once = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance(), key: "k-1" });
  assert.strictEqual(once.status, 201);
  const again = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance(), key: "k-1" });
  assert.strictEqual(again.status, 201, "replayed, not connection-taken");
  const other = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ credential: await sealed(KEY, ROTATED, { base: BASE }) }), key: "k-1" });
  assert.deepStrictEqual(pair(other), [409, "idempotency-key-reused"]);
  noSecretIn(storeText(env), "the store");
});

/* ---- the key is replaced ----------------------------------------------- */

test("a removal's key rotation seals every credential again, or does not happen, and the removed member's key does not open the new one", async () => {
  const { env, ledger, alice } = await world();
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  const B = "https://payroll.acme.example";
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: { name: "payroll", base: B, credential: await sealed(KEY, ROTATED, { name: "payroll", base: B }) } });
  const carol = await memberDevice(env, "carol", alice);
  await memberDevice(env, "dan", alice);
  // Dan's device held version 1: KEY. He is removed; the key is stale.
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`)).status, 204);

  // Carol's device, which will replace it, is handed every sealed
  // credential with what it is sealed for. Nobody else is.
  const v = await call(env, "carol", "GET", `/orgs/acme/keys?device=${carol.id}`);
  assert.deepStrictEqual(v.body.credentials.map((c) => [c.name, c.base, c.header, c.v]), [["finance-api", BASE, "Authorization", 1], ["payroll", B, "Authorization", 1]]);
  assert.strictEqual(v.body.credentials[0].sealed, CRED.sealed);
  const bobs = device();
  await register(env, "bob", bobs);
  assert.strictEqual((await call(env, "bob", "GET", `/orgs/acme/keys?device=${bobs.id}`)).body.credentials, undefined, "a device without the key is handed none");

  const NEXT = await orgKey();
  const rotation = (credentials) => ({
    version: 2, public_key: NEXT.publicHex, by_device: carol.id, recovery_blob: b64(200),
    envelopes: { [alice.id]: b64(300), [carol.id]: b64(300) }, credentials,
  });
  const resealed = {
    "finance-api": await sealed(NEXT, SECRET, { base: BASE, v: 2 }),
    payroll: await sealed(NEXT, ROTATED, { name: "payroll", base: B, v: 2 }),
  };
  // Leaving one behind is refused, and so is one sealed to the old version.
  for (const credentials of [undefined, { "finance-api": resealed["finance-api"] }, { ...resealed, payroll: { ...resealed.payroll, v: 1 } }]) {
    const r = await call(env, "carol", "POST", "/orgs/acme/keys/rotate", { json: rotation(credentials) });
    assert.deepStrictEqual(pair(r), [400, "bad-request"], JSON.stringify(credentials));
    assert.match(r.body.message, /sealed again to the new version: (finance-api|payroll) is missing/);
  }
  assert.strictEqual(store(env).prepare("SELECT MAX(version) AS v FROM key_versions").get().v, 1, "no rotation happened");
  // A device without the key is not told which connections exist.
  const bobTry = await call(env, "bob", "POST", "/orgs/acme/keys/rotate", { json: { ...rotation(undefined), by_device: bobs.id } });
  assert.ok(!JSON.stringify(bobTry.body).includes("finance-api"), JSON.stringify(bobTry.body));

  const r = await call(env, "carol", "POST", "/orgs/acme/keys/rotate", { json: rotation(resealed) });
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  const rows = store(env).prepare("SELECT name, credential_v, sealed FROM connections ORDER BY name").all();
  assert.deepStrictEqual(rows.map((c) => [c.name, c.credential_v]), [["finance-api", 2], ["payroll", 2]]);
  const got = await use(env, "carol", ledger);
  assert.strictEqual(got.body.credential.v, 2);
  assert.strictEqual(await opened(NEXT, got.body.credential, { base: BASE }), SECRET, "the remaining members' new key opens it");
  assert.strictEqual(await opened(KEY, got.body.credential, { base: BASE }), null, "the key the removed member holds does not");
  const list = (await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections;
  assert.deepStrictEqual(list.map((c) => c.credential.version), [2, 2]);
  // A credential now has to be sealed to version 2.
  const late = await call(env, "alice", "PUT", "/orgs/acme/connections/finance-api/credential", { json: { credential: CRED } });
  assert.deepStrictEqual(pair(late), [409, "key-rotated"]);
  const lines = (await events(env)).filter((e) => e.action === "connection.reseal");
  assert.deepStrictEqual(lines.map((e) => [e.actor, e.target, e.reason]), [["github:carol", "key:v2", `2 credentials sealed again by ${carol.id}`]]);
  noSecretIn(storeText(env), "the store");
});

test("a credential that does not open on the rotating device is marked to be entered again, not left on the old key", async () => {
  const { env, ledger, alice } = await world();
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  const carol = await memberDevice(env, "carol", alice);
  await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`);
  const NEXT = await orgKey();
  const r = await call(env, "carol", "POST", "/orgs/acme/keys/rotate", {
    json: { version: 2, public_key: NEXT.publicHex, by_device: carol.id, recovery_blob: b64(200), envelopes: { [alice.id]: b64(300), [carol.id]: b64(300) }, credentials: { "finance-api": { unopened: true } } },
  });
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  const row = store(env).prepare("SELECT sealed, credential_v FROM connections").get();
  assert.deepStrictEqual([row.sealed, row.credential_v], ["", 0], "the old blob is gone");
  const c = (await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections[0];
  assert.deepStrictEqual([c.credential.state, c.credential.version], ["needs-entry", null]);
  const refused = await use(env, "carol", ledger);
  assert.deepStrictEqual(pair(refused), [409, "credential-needs-entry"]);
  assert.strictEqual(refused.body.base, BASE);
  assert.match(refused.body.message, /entered again by an owner or admin of Acme/);
  // Entering it again, sealed to the new key, puts it back.
  const back = await call(env, "alice", "PUT", "/orgs/acme/connections/finance-api/credential", { json: { credential: await sealed(NEXT, SECRET, { base: BASE, v: 2 }) } });
  assert.strictEqual(back.status, 200, JSON.stringify(back.body));
  assert.strictEqual(await opened(NEXT, (await use(env, "carol", ledger)).body.credential, { base: BASE }), SECRET);
});

/* ---- the one-time migration -------------------------------------------- */

test("a credential kept the old way is deleted and marked to be entered again, and says so", async () => {
  const { env, ledger } = await world();
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  // As an older hub kept it: sealed with the hub's own key, no key version.
  const oldBlob = JSON.stringify({ iv: b64(12), body: b64(80) });
  store(env).prepare("UPDATE connections SET sealed = ?, credential_v = NULL").run(oldBlob);
  // The next command of any kind migrates it, before anything can use it.
  const refused = await use(env, "dan", ledger);
  assert.deepStrictEqual(pair(refused), [409, "credential-needs-entry"]);
  assert.match(refused.body.message, /kept the old way, which Krate's hub could read/);
  const row = store(env).prepare("SELECT sealed, credential_v FROM connections").get();
  assert.deepStrictEqual([row.sealed, row.credential_v], ["", 0]);
  assert.ok(!storeText(env).includes(oldBlob), "the old blob is gone from the store");
  const c = (await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections[0];
  assert.strictEqual(c.credential.state, "needs-entry");
  const lines = (await events(env)).filter((e) => e.action === "connection.needs-entry");
  assert.deepStrictEqual(lines.map((e) => [e.actor, e.target]), [["system:krate", "connection:finance-api"]], "once");
});

/* ---- counting ---------------------------------------------------------- */

test("each use is counted per day, and each finished day becomes one audit line", async () => {
  const { env, ledger } = await world();
  await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() });
  for (let i = 0; i < 3; i += 1) assert.strictEqual((await use(env, "dan", ledger)).status, 200);
  const today = (await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections[0].uses;
  assert.deepStrictEqual(today, { today: 3, last_30_days: 3 });
  assert.strictEqual((await events(env)).filter((e) => e.action === "connection.use").length, 0, "no line until the day is over");

  // The next day: the first command writes yesterday's line.
  const o = env.ORGS.object("acme");
  const real = o.now.bind(o);
  o.now = () => real() + DAY;
  assert.strictEqual((await use(env, "dan", ledger)).status, 200);
  const lines = (await events(env)).filter((e) => e.action === "connection.use");
  assert.strictEqual(lines.length, 1);
  assert.strictEqual(lines[0].actor, "system:krate");
  assert.match(lines[0].reason, /^3 uses on \d{4}-\d\d-\d\d$/);
  const next = (await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections[0].uses;
  assert.deepStrictEqual(next, { today: 1, last_30_days: 4 });
  // The chain still holds.
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/audit")).body.chain_ok, true);
});

/* ---- the API's summary (SMART-FIXES 4) ------------------------------------ */

const API = {
  title: "Acme Finance", version: "2.1", source: "https://api.acme.example/finance/openapi.json",
  operations: [
    { method: "GET", path: "/invoices", id: "listInvoices", summary: "List invoices", query: ["status", "limit"] },
    { method: "GET", path: "/invoices/{id}", id: "getInvoice", summary: "One invoice" },
    { method: "POST", path: "/invoices", id: "createInvoice", body: true },
  ],
  more: 0,
};

test("a connection keeps its API's summary: set with it, changed or removed later, read in full by whoever may see connections", async () => {
  const { env } = await world();
  const made = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ api: API }) });
  assert.strictEqual(made.status, 201, JSON.stringify(made.body));
  // The list says what it is, never carries the whole.
  assert.deepStrictEqual(made.body.connection.api, { title: "Acme Finance", version: "2.1", source: API.source, operations: 3, more: 0 });
  for (const who of ["alice", "erin", "carol", "bob"]) {
    const r = await call(env, who, "GET", "/orgs/acme/connections/finance-api/api");
    assert.strictEqual(r.status, 200, who);
    assert.deepStrictEqual(r.body, { name: "finance-api", base: BASE, allow: { methods: ["GET"], paths: ["/invoices"] }, api: API }, who);
    noSecretIn(JSON.stringify(r.body), who);
  }
  assert.deepStrictEqual(pair(await call(env, "dan", "GET", "/orgs/acme/connections/finance-api/api")), [403, "role-cannot"]);
  assert.deepStrictEqual(pair(await call(env, "carol", "GET", "/orgs/acme/connections/payroll/api")), [404, "no-such-connection"]);

  // Only an owner or admin changes it; a change says so in the audit.
  assert.deepStrictEqual(pair(await call(env, "carol", "PUT", "/orgs/acme/connections/finance-api", { json: { api: null } })), [403, "role-cannot"]);
  const one = { ...API, operations: API.operations.slice(0, 1) };
  const changed = await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", { json: { api: one } });
  assert.strictEqual(changed.status, 200, JSON.stringify(changed.body));
  assert.strictEqual(changed.body.connection.api.operations, 1);
  const gone = await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", { json: { api: null } });
  assert.strictEqual(gone.body.connection.api, null);
  assert.strictEqual((await call(env, "carol", "GET", "/orgs/acme/connections/finance-api/api")).body.api, null);
  // A change that leaves it out keeps it.
  await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", { json: { api: API } });
  await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", { json: { allow: { methods: ["GET"], paths: ["/invoices", "/rates"] } } });
  assert.strictEqual((await call(env, "carol", "GET", "/orgs/acme/connections/finance-api/api")).body.api.operations.length, 3);
  const said = (await events(env)).filter((e) => e.action === "connection.change").map((e) => e.reason);
  assert.deepStrictEqual(said, ["API description 1 operation", "API description removed", "API description 3 operations", "allow GET /invoices,/rates"]);
});

test("an API summary is short, plain and on one line per field, or it is refused", async () => {
  assert.strictEqual(MAX_API, 24 * 1024);
  const ok = normalApi(API);
  assert.ok(ok.api && JSON.parse(ok.api).operations.length === 3);
  assert.deepStrictEqual(normalApi(null), { api: null });
  // Fields nobody reads are dropped.
  assert.ok(!JSON.parse(normalApi({ ...API, secret: "x", operations: [{ method: "get", path: "/a", extra: 1 }] }).api).secret);
  assert.deepStrictEqual(JSON.parse(normalApi({ operations: [{ method: "get", path: "/a", extra: 1 }] }).api).operations, [{ method: "GET", path: "/a" }]);
  const refused = (v, words) => {
    const r = normalApi(v);
    assert.ok(r.problem, JSON.stringify(v).slice(0, 80));
    if (words) assert.match(r.problem, words);
  };
  refused("text");
  refused([]);
  refused({ ...API, source: "http://api.acme.example/openapi.json" }, /source/);
  refused({ ...API, title: "two\nlines" }, /one line/);
  refused({ operations: [{ method: "BREW", path: "/pot" }] }, /method/);
  refused({ operations: [{ method: "GET", path: "pot" }] }, /starts with \//);
  refused({ operations: [{ method: "GET", path: "/a", summary: "x".repeat(161) }] }, /160/);
  refused({ operations: [{ method: "GET", path: "/a", query: Array(13).fill("q") }] }, /12/);
  refused({ more: -1 }, /whole number/);
  // Just under the cap is kept; just over is refused, never cut.
  const op = (n) => ({ method: "GET", path: `/things/${String(n).padStart(6, "0")}`, summary: "s".repeat(100) });
  const many = (n) => ({ title: "Big", operations: Array.from({ length: n }, (_, i) => op(i)) });
  let n = 1;
  while (!normalApi(many(n + 1)).problem) n += 1;
  assert.ok(new TextEncoder().encode(normalApi(many(n)).api).length <= MAX_API);
  refused(many(n + 1), /over 24 KB/);
  assert.ok(n > 100, `only ${n} operations fit`);
});

test("the hub refuses an API summary it could not keep, and nothing is saved", async () => {
  const { env } = await world();
  const r = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ api: { operations: [{ method: "GET", path: "/a", summary: "line\u0000" }] } }) });
  assert.deepStrictEqual(pair(r), [400, "connection-invalid"]);
  assert.match(r.body.message, /one line/);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections.length, 0);
});
