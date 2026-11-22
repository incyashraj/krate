/* One account, every way of signing in to it (K-1328).
 *
 * The founder signed in to the command line with a Krate session -- his
 * account, GitHub linked -- and republished the featured apps. The hub took
 * the session as `account:<id>` only, so every app came out as a second
 * listing beside the one his GitHub sign-in had published, without the
 * verified mark, and nobody could fold them together: each sign-in owned
 * only what it alone had published.
 *
 * Now an account GitHub vouched for is that GitHub login from any of its
 * sign-ins, and every ownership check asks for any of the person's owners.
 * The login text of an account is never trusted for this: an email sign-in
 * as boss@attacker.example is "boss" and must stay nobody's GitHub (K-1085).
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/one-account-many-sign-ins.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";
import "./github-stub.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const build = (v) => new Uint8Array(readFileSync(join(here, "fixtures", `notes-${v}.krate`)));
const hashOf = (v) => createHash("sha256").update(build(v)).digest("hex");
const hubEnv = () => {
  const env = orgEnv({ PUBLIC_BASE: "https://hub.example" });
  env.USAGE = { writeDataPoint() {} };
  return env;
};
const as = (token) => ({ authorization: `Bearer ${token}` });
const publish = (env, token, v) =>
  call(env, null, "POST", "/publish", { bytes: build(v), headers: { ...as(token), "x-krate-name": "Notes" } });
const row = (env, hash) => JSON.parse(env._kv.get(`app:${hash}`) || "null");
const galleryRows = async (env) => (await call(env, null, "GET", "/apps")).body.apps.filter((a) => a.meta.name === "Notes");

/// GitHub signs `login` in, the way any GitHub-token request does, and a
/// Krate session (a Google or email sign-in) is opened on the same account.
async function personWithGitHubAndSession(env, login) {
  const me = await call(env, null, "GET", "/my/apps", { headers: as(`gho_${login}`) });
  assert.equal(me.status, 200, JSON.stringify(me.body));
  const userId = env._kv.get(`ghlogin:${login}`);
  assert.ok(userId, "the GitHub sign-in made an account and pointed its login at it");
  env._kv.set(`session:krs_${login}-google`, userId);
  return { userId, session: `krs_${login}-google` };
}

test("a republish from the account's other sign-in is the app's next version, verified", async () => {
  const env = hubEnv();
  const { session } = await personWithGitHubAndSession(env, "alice");
  const v1 = await publish(env, "gho_alice", "0.1.0");
  assert.equal(v1.status, 200, JSON.stringify(v1.body));
  const v2 = await publish(env, session, "0.2.0");
  assert.equal(v2.status, 200, JSON.stringify(v2.body));
  assert.equal(v2.body.url, v1.body.url, "the same app, not a second one beside it");
  assert.equal(v2.body.version, 2);
  assert.equal(row(env, v2.body.id).owner, "github:alice", "filed under the owner that holds the line");
  assert.equal(row(env, v2.body.id).author_url, "https://github.com/alice", "and it reads verified");
  assert.deepEqual((await galleryRows(env)).map((a) => a.id), [v2.body.id], "one gallery row");
});

test("an app the account published first keeps its line when GitHub publishes the update", async () => {
  const env = hubEnv();
  const { session } = await personWithGitHubAndSession(env, "alice");
  // Before the link existed the session published as the account.
  const userId = env._kv.get("ghlogin:alice");
  env._kv.set(`user:${userId}`, JSON.stringify({ ...JSON.parse(env._kv.get(`user:${userId}`)), github_login: "" }));
  env._kv.delete("ghlogin:alice");
  const v1 = await publish(env, session, "0.1.0");
  assert.equal(row(env, v1.body.id).owner, `account:${userId}`, "the fixture really is an account-owned app");
  // GitHub signs in again (relinking the login), and publishes the update.
  await call(env, null, "GET", "/my/apps", { headers: as("gho_alice") });
  const v2 = await publish(env, "gho_alice", "0.2.0");
  assert.equal(v2.status, 200, JSON.stringify(v2.body));
  assert.equal(v2.body.url, v1.body.url, "the GitHub sign-in found the account's line");
  assert.equal(row(env, v2.body.id).owner, `account:${userId}`, "same owner, so the app's saved data carries over");
});

test("either sign-in may remove what the other published", async () => {
  const env = hubEnv();
  const { session } = await personWithGitHubAndSession(env, "alice");
  const byGitHub = await publish(env, "gho_alice", "0.1.0");
  const gone = await call(env, null, "DELETE", `/app/${byGitHub.body.id}`, { headers: as(session) });
  assert.equal(gone.status, 200, JSON.stringify(gone.body));
  assert.equal(row(env, byGitHub.body.id), null);

  const bySession = await publish(env, session, "0.2.0");
  const gone2 = await call(env, null, "DELETE", `/app/${bySession.body.id}`, { headers: as("gho_alice") });
  assert.equal(gone2.status, 200, JSON.stringify(gone2.body));
});

test("an app from before versions existed folds in, whichever sign-in republishes it", async () => {
  const env = hubEnv();
  const { session } = await personWithGitHubAndSession(env, "alice");
  // A listing from before lines and owners: only its GitHub author login.
  const legacy = hashOf("0.1.0");
  env._kv.set(`app:${legacy}`, JSON.stringify({ name: "Notes", author: "alice", author_login: "alice", published: 1, size: 1 }));
  const v = await publish(env, session, "0.2.0");
  assert.equal(v.status, 200, JSON.stringify(v.body));
  const old = row(env, legacy);
  assert.equal(old.superseded, true, "the old row is now the app's earlier version");
  assert.equal(old.line, v.body.url.split("/a/")[1]);
  assert.deepEqual((await galleryRows(env)).map((a) => a.id), [v.body.id]);
});

test("an account's login text never makes it somebody's GitHub (K-1085)", async () => {
  const env = hubEnv();
  // The real boss signs in with GitHub and publishes.
  await call(env, null, "GET", "/my/apps", { headers: as("gho_boss") });
  const theirs = await publish(env, "gho_boss", "0.1.0");
  assert.equal(theirs.status, 200);
  // An attacker's email account is called "boss" and says GitHub is linked.
  env._kv.set("session:krs_fake", "fake");
  env._kv.set("user:fake", JSON.stringify({ id: "fake", login: "boss", name: "", providers: ["email", "github"] }));
  assert.notEqual(env._kv.get("ghlogin:boss"), "fake", "the fixture carries the hazard: the login text matches");
  const grab = await call(env, null, "DELETE", `/app/${theirs.body.id}`, { headers: as("krs_fake") });
  assert.equal(grab.status, 403, "the account's own text is not GitHub's word");
  const update = await publish(env, "krs_fake", "0.2.0");
  assert.notEqual(update.body.url, theirs.body.url, "nor may it publish into the real boss's app");
});
