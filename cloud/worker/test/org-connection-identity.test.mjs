/* Telling the API who is calling (SMART-FIXES section 5): with a
 * connection's credential the hub hands a short-lived token, signed with a
 * key derived from the lease key's seed, naming the person, organisation,
 * app, release and connection, for the connection's address. An API checks
 * it against /.well-known/jwks.json. Only after every check the credential
 * fetch makes; only while the connection's setting is on.
 *
 * Checked here with WebCrypto against the JWKS the hub serves, the way an
 * API owner would check it.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-connection-identity.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";
import { device, register, recovery, b64 } from "./org-keys-tools.mjs";
import { orgKey, sealed } from "./org-seal-tools.mjs";
import { leaseKey } from "../src/org-lease.js";
import { IDENTITY_SECONDS, unb64url } from "../src/org-identity-token.js";

const here = dirname(fileURLToPath(import.meta.url));
const FIX = join(here, "fixtures", "org-connections");
const fixture = (name) => new Uint8Array(readFileSync(join(FIX, name)));
const KEYS = JSON.parse(readFileSync(join(FIX, "keys.json"), "utf8"));
const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const BASE = "https://api.acme.example/finance";
const pair = (r) => [r.status, r.body && r.body.error];
const KEY = await orgKey();
const CRED = await sealed(KEY, "Bearer sk-identity-test-only", { base: BASE });

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

/* acme as org-connections.test.mjs builds it, on a hub with a lease key. */
async function world(extra = {}) {
  const env = orgEnv({ ORG_LEASE_KEY: SEED, ...extra });
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
  const alice = device();
  await register(env, "alice", alice, "Alice's MacBook");
  ok(
    await call(env, "alice", "POST", "/orgs/acme/keys/init", {
      json: { device_id: alice.id, public_key: KEY.publicHex, envelope: b64(300), recovery: recovery() },
    }),
    "keys",
  );
  return { env, ledger };
}

const use = (env, who, digest) => call(env, who, "GET", `/orgs/acme/connections/finance-api/credential/${digest}`);
const refresh = (env, who, digest) => call(env, who, "GET", `/orgs/acme/connections/finance-api/identity/${digest}`);
const events = async (env) => (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events;

/* What an API owner does: split the token, find the key by kid in the
 * JWKS, verify EdDSA with WebCrypto, then read the claims. Answers the
 * claims, or null when the signature does not hold. */
async function verified(token, set) {
  const [h, p, s] = String(token).split(".");
  const header = JSON.parse(new TextDecoder().decode(unb64url(h)));
  assert.deepStrictEqual([header.alg, header.typ], ["EdDSA", "JWT"]);
  const jwk = set.keys.find((k) => k.kid === header.kid);
  if (!jwk) return null;
  const key = await crypto.subtle.importKey("jwk", { kty: jwk.kty, crv: jwk.crv, x: jwk.x }, { name: "Ed25519" }, false, ["verify"]);
  const ok = await crypto.subtle.verify({ name: "Ed25519" }, key, unb64url(s), new TextEncoder().encode(`${h}.${p}`));
  return ok ? JSON.parse(new TextDecoder().decode(unb64url(p))) : null;
}

test("the JWKS serves one Ed25519 key with a stable kid, derived from and not equal to the lease key", async () => {
  const env = orgEnv({ ORG_LEASE_KEY: SEED });
  const r = await call(env, null, "GET", "/.well-known/jwks.json");
  assert.strictEqual(r.status, 200);
  assert.strictEqual(r.headers.get("access-control-allow-origin"), "*");
  assert.strictEqual(r.body.keys.length, 1);
  const [k] = r.body.keys;
  assert.deepStrictEqual([k.kty, k.crv, k.alg, k.use], ["OKP", "Ed25519", "EdDSA", "sig"]);
  const digest = Buffer.from(await crypto.subtle.digest("SHA-256", unb64url(k.x))).toString("hex");
  assert.strictEqual(k.kid, digest.slice(0, 16), "the kid is the public key's hash");
  const lease = await leaseKey(env);
  assert.notStrictEqual(Buffer.from(unb64url(k.x)).toString("hex"), lease.publicKey, "the lease key is never the token key");
  // The same seed, the same key, every time.
  assert.deepStrictEqual((await call(env, null, "GET", "/.well-known/jwks.json")).body, r.body);
  // No lease key: an empty set, not an error.
  const none = await call(orgEnv({ ORG_LEASE_KEY: "" }), null, "GET", "/.well-known/jwks.json");
  assert.deepStrictEqual([none.status, none.body], [200, { keys: [] }]);
});

test("the credential fetch carries a token that verifies against the JWKS, with the right claims and audience", async () => {
  const { env, ledger } = await world();
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance() })).status, 201);
  const set = (await call(env, null, "GET", "/.well-known/jwks.json")).body;
  const got = await use(env, "dan", ledger);
  assert.strictEqual(got.status, 200, JSON.stringify(got.body));
  assert.strictEqual(got.body.identity.expires_in, IDENTITY_SECONDS);
  const claims = await verified(got.body.identity.token, set);
  assert.ok(claims, "the signature holds against the JWKS");
  assert.deepStrictEqual(
    [claims.iss, claims.sub, claims.aud, claims.org, claims.app, claims.release, claims.connection],
    ["https://hub.example", "github:dan", BASE, "acme", "com.acme.ledger", ledger, "finance-api"],
  );
  assert.strictEqual(claims.exp - claims.iat, 5 * 60);
  assert.strictEqual(claims.nbf, claims.iat);
  assert.ok(Math.abs(claims.iat - Date.now() / 1000) < 120, "issued now");
  assert.match(claims.jti, /^[0-9a-f]{32}$/);

  // A changed claim does not verify; nor does a token checked with the
  // lease key.
  const [h, p, s] = got.body.identity.token.split(".");
  const forged = Buffer.from(JSON.stringify({ ...claims, sub: "github:alice" })).toString("base64url");
  assert.strictEqual(await verified(`${h}.${forged}.${s}`, set), null);
  const lease = await leaseKey(env);
  const leaseSet = { keys: [{ ...set.keys[0], x: Buffer.from(lease.publicKey, "hex").toString("base64url") }] };
  assert.strictEqual(await verified(`${h}.${p}.${s}`, leaseSet), null);

  // The refresh: a new token for the same run, no credential with it.
  const again = await refresh(env, "dan", ledger);
  assert.strictEqual(again.status, 200, JSON.stringify(again.body));
  assert.deepStrictEqual(Object.keys(again.body).sort(), ["identity", "name"]);
  assert.strictEqual(again.headers.get("cache-control"), "private, no-store");
  const fresh = await verified(again.body.identity.token, set);
  assert.strictEqual(fresh.sub, "github:dan");
  assert.notStrictEqual(fresh.jti, claims.jti);

  // Each one is a use in the daily count, never an audit line of its own.
  const lines = (await events(env)).length;
  await refresh(env, "dan", ledger);
  await refresh(env, "carol", ledger);
  assert.strictEqual((await events(env)).length, lines, "no audit line per token");
  const view = (await call(env, "alice", "GET", "/orgs/acme/connections")).body.connections[0];
  assert.strictEqual(view.uses.today, 4);
  assert.strictEqual(view.identity, true, "on by default");
});

test("a refused person gets no token, from the fetch or the refresh", async () => {
  const { env, ledger } = await world();
  await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "finance" } });
  await call(env, "alice", "PUT", "/orgs/acme/groups/finance/members/github%3Adan");
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ audience: { groups: ["finance"] } }) })).status, 201);
  assert.strictEqual((await refresh(env, "dan", ledger)).body.identity.expires_in, IDENTITY_SECONDS);
  for (const [who, want] of [["bob", [403, "connection-not-in-audience"]], ["stranger", [403, "not-a-member"]], [null, [401, "not-signed-in"]]]) {
    for (const ask of [use, refresh]) {
      const r = await ask(env, who, ledger);
      assert.deepStrictEqual(pair(r), want, `${who}: ${JSON.stringify(r.body)}`);
      assert.strictEqual(r.body.identity, undefined, `${who} was handed a token`);
      assert.ok(!JSON.stringify(r.body).includes("eyJ"), `${who} was handed a token`);
    }
  }
  // Removed after the app opened: the next refresh is refused.
  assert.strictEqual((await call(env, "alice", "DELETE", "/orgs/acme/members/github%3Adan")).status, 204);
  const gone = await refresh(env, "dan", ledger);
  assert.deepStrictEqual(pair(gone), [403, "removed"]);
  assert.strictEqual(gone.body.identity, undefined);
  // A digest that is not a release.
  assert.deepStrictEqual(pair(await refresh(env, "carol", "0".repeat(64))), [404, "digest-mismatch"]);
});

test("with the setting off there is no token; an admin turns it back on, in the audit", async () => {
  const { env, ledger } = await world();
  const made = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ identity: false }) });
  assert.strictEqual(made.status, 201, JSON.stringify(made.body));
  assert.strictEqual(made.body.connection.identity, false);
  const got = await use(env, "dan", ledger);
  assert.strictEqual(got.status, 200);
  assert.strictEqual(got.body.identity, undefined, "no token with the credential");
  assert.ok(got.body.credential.sealed, "the credential still comes");
  const again = await refresh(env, "dan", ledger);
  assert.deepStrictEqual([again.status, again.body.identity], [200, null]);

  const bad = await call(env, "alice", "PUT", "/orgs/acme/connections/finance-api", { json: { identity: "yes" } });
  assert.deepStrictEqual(pair(bad), [400, "connection-invalid"]);
  assert.deepStrictEqual(pair(await call(env, "carol", "PUT", "/orgs/acme/connections/finance-api", { json: { identity: true } })), [403, "role-cannot"]);
  const on = await call(env, "erin", "PUT", "/orgs/acme/connections/finance-api", { json: { identity: true } });
  assert.strictEqual(on.status, 200, JSON.stringify(on.body));
  assert.strictEqual(on.body.connection.identity, true);
  assert.ok((await use(env, "dan", ledger)).body.identity.token);
  const change = (await events(env)).filter((e) => e.action === "connection.change");
  assert.deepStrictEqual(change.map((e) => [e.actor, e.reason]), [["github:erin", "tells the API who is calling"]]);

  // No lease key on the hub: the credential still comes, with no token.
  delete env.ORG_LEASE_KEY;
  const keyless = await use(env, "dan", ledger);
  assert.strictEqual(keyless.status, 200);
  assert.strictEqual(keyless.body.identity, undefined);
});

test("the credential cannot ride in Krate-Identity", async () => {
  const { env } = await world();
  const r = await call(env, "alice", "POST", "/orgs/acme/connections", { json: finance({ credential: { ...CRED, header: "krate-identity" } }) });
  assert.deepStrictEqual(pair(r), [400, "connection-invalid"]);
});
