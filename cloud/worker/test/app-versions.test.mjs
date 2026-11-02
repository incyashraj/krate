/* One app, many versions (IC-976, cloud/worker/src/lines.js).
 *
 * The same owner publishing the same manifest app id is that app's next
 * version: one link that always opens the current version, one gallery row,
 * every older build still opening at its own link and saying a newer one
 * exists, and only the owner choosing which version is current.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/app-versions.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
// Records Check (crates/adapter-web/player/fixtures/sql-check) packed as
// 0.1.0, 0.2.0 and 0.3.0: one app id, three different builds.
const build = (v) => new Uint8Array(readFileSync(join(here, "fixtures", `notes-${v}.krate`)));
const hubEnv = () => {
  const env = orgEnv({ PUBLIC_BASE: "https://hub.example" });
  env.USAGE = { writeDataPoint() {} };
  return env;
};
const publish = (env, who, v, headers = {}) =>
  call(env, who, "POST", "/publish", { bytes: build(v), headers: { "x-krate-name": "Notes", ...headers } });
const row = (env, hash) => JSON.parse(env._kv.get(`app:${hash}`) || "null");
const opens = async (env, ref) => {
  const r = await call(env, null, "GET", `/a/${ref}?dl=1`);
  return r.status === 200 ? r.headers.get("x-krate-sha256") || r.bytes.length : r.status;
};

test("publishing the same app again is its next version, behind one link", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  assert.equal(v1.status, 200, JSON.stringify(v1.body));
  assert.match(v1.body.url, /^https:\/\/hub\.example\/a\/v[0-9a-f]{11}$/, "the app's own link");
  assert.equal(v1.body.version, 1);
  assert.equal(v1.body.updated, false);
  const v2 = await publish(env, "alice", "0.2.0");
  assert.equal(v2.status, 200, JSON.stringify(v2.body));
  assert.equal(v2.body.url, v1.body.url, "the same link, not another app");
  assert.equal(v2.body.version, 2);
  assert.equal(v2.body.updated, true);
  assert.notEqual(v2.body.version_url, v1.body.version_url, "each build keeps a link of its own");

  // The app's link opens the current version: its meta is v2's.
  const line = v1.body.url.split("/a/")[1];
  const now = await call(env, null, "GET", `/meta/${line}`);
  assert.equal(now.status, 200, JSON.stringify(now.body));
  assert.equal(now.body.id, v2.body.id);
  assert.deepEqual(now.body.version && [now.body.version.n, now.body.version.newer], [2, false]);
  // v1's own link still opens v1, and says a newer version exists.
  const old = await call(env, null, "GET", `/meta/${v1.body.id}`);
  assert.equal(old.body.id, v1.body.id);
  assert.equal(old.body.meta.name, "Notes", "an older build keeps its name");
  assert.deepEqual([old.body.version.n, old.body.version.current, old.body.version.newer], [1, 2, true]);
  assert.equal(old.body.version.app_url, v1.body.url);

  // One gallery row, for the current version, linking the app's own link.
  const gallery = await call(env, null, "GET", "/apps");
  const rows = gallery.body.apps.filter((a) => a.meta.name === "Notes");
  assert.deepEqual(rows.map((a) => a.id), [v2.body.id], "one row per app");
  assert.equal(rows[0].url, v1.body.url);
  assert.equal(rows[0].meta.version_n, 2);
  assert.equal(row(env, v1.body.id).superseded, true);

  // The same bytes again are no new version.
  const again = await publish(env, "alice", "0.2.0");
  assert.equal(again.body.version, 2);
});

test("only the owner sees every version and chooses which is current; unpublishing steps back", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  const v2 = await publish(env, "alice", "0.2.0");
  const v3 = await publish(env, "alice", "0.3.0");
  const line = v1.body.url.split("/a/")[1];

  const mine = await call(env, "alice", "GET", "/lines?app=dev.krate.sql-check");
  assert.equal(mine.status, 200, JSON.stringify(mine.body));
  assert.deepEqual(mine.body.history.map((v) => [v.n, v.version, v.current]), [[1, "0.1.0", false], [2, "0.2.0", false], [3, "0.3.0", true]]);
  const theirs = await call(env, "bob", "GET", `/lines/${line}`);
  assert.equal(theirs.body.current, 3);
  assert.equal(theirs.body.history, undefined, "the version list is the owner's");
  assert.equal((await call(env, "bob", "GET", "/lines?app=dev.krate.sql-check")).status, 404, "bob has no such app");

  // Bob cannot move alice's app back; alice can.
  assert.equal((await call(env, "bob", "POST", `/lines/${line}/current`, { json: { version: 1 } })).status, 403);
  assert.equal((await call(env, null, "POST", `/lines/${line}/current`, { json: { version: 1 } })).status, 403);
  const back = await call(env, "alice", "POST", `/lines/${line}/current`, { json: { version: 1 } });
  assert.equal(back.status, 200, JSON.stringify(back.body));
  assert.equal((await call(env, null, "GET", `/meta/${line}`)).body.id, v1.body.id, "the link opens v1 again");
  const shelf = (await call(env, null, "GET", "/apps")).body.apps.filter((a) => a.meta.name === "Notes");
  assert.deepEqual(shelf.map((a) => a.id), [v1.body.id], "and the gallery row is v1's");
  assert.equal((await call(env, "alice", "POST", `/lines/${line}/current`, { json: { version: 9 } })).status, 404);

  // Unpublishing the current version steps the link back to the newest left.
  await call(env, "alice", "POST", `/lines/${line}/current`, { json: { version: 3 } });
  const gone = await call(env, "alice", "DELETE", `/app/${v3.body.id}`);
  assert.equal(gone.status, 200, JSON.stringify(gone.body));
  assert.equal((await call(env, null, "GET", `/meta/${line}`)).body.id, v2.body.id);
  assert.equal(row(env, v2.body.id).superseded, false, "v2 is the gallery row again");
});

test("another owner's same app id is a separate app, and an open-it-here publish is no version", async () => {
  const env = hubEnv();
  const a = await publish(env, "alice", "0.1.0");
  const b = await publish(env, "bob", "0.2.0");
  assert.notEqual(a.body.url, b.body.url, "bob's app is his own line");
  assert.equal(b.body.version, 1);
  const quiet = await publish(env, "alice", "0.3.0", { "x-krate-unlisted": "1", "x-krate-quiet": "1" });
  assert.equal(quiet.status, 200, JSON.stringify(quiet.body));
  assert.equal(quiet.body.version, undefined, "opening a tab makes no version");
  assert.doesNotMatch(quiet.body.url, /\/a\/v/, "and its link is the build's own");
  const mine = await call(env, "alice", "GET", "/lines?app=dev.krate.sql-check");
  assert.equal(mine.body.versions, 1);
});

test("an app published before versions existed becomes the earlier version, and a shelf pick follows the app", async () => {
  const env = hubEnv();
  const old = await publish(env, "alice", "0.1.0");
  // As it stood before IC-976: a listing row with no line, and no line record.
  const row0 = row(env, old.body.id);
  delete row0.line;
  delete row0.version_n;
  env._kv.set(`app:${old.body.id}`, JSON.stringify(row0));
  env._kv.delete(`line:${old.body.line}`);
  env.GALLERY_PICKS = old.body.id.slice(0, 10);

  const next = await publish(env, "alice", "0.2.0");
  assert.equal(next.body.version, 1, "the first version on its line");
  const adopted = row(env, old.body.id);
  assert.deepEqual([adopted.line, adopted.version_n, adopted.superseded], [next.body.line, 0, true], "kept as the earlier version, not deleted");
  const meta = await call(env, null, "GET", `/meta/${old.body.id}`);
  assert.equal(meta.body.version.newer, true, "its link says a newer version exists");
  assert.equal(meta.body.version.app_url, next.body.url);

  const shelves = (await call(env, null, "GET", "/apps")).body.shelves;
  const picks = shelves.find((s) => s.id === "picks");
  assert.ok(picks, JSON.stringify(shelves.map((s) => s.id)));
  assert.deepEqual(picks.apps.map((a) => a.id), [next.body.id], "the pick follows the app to its current version");
});

test("an update of a gallery app shared by link stays in the gallery; a private app stays private", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  // Studio's Link pane publishes unlisted.
  const v2 = await publish(env, "alice", "0.2.0", { "x-krate-unlisted": "1" });
  const rows = (await call(env, null, "GET", "/apps")).body.apps.filter((a) => a.meta.name === "Notes");
  assert.deepEqual(rows.map((a) => a.id), [v2.body.id], "v2 takes v1's place on the shelf");
  const quiet = hubEnv();
  await publish(quiet, "bob", "0.1.0", { "x-krate-unlisted": "1" });
  await publish(quiet, "bob", "0.2.0", { "x-krate-unlisted": "1" });
  assert.equal((await call(quiet, null, "GET", "/apps")).body.apps.filter((a) => a.meta.name === "Notes").length, 0, "an unlisted app stays unlisted");
  assert.ok(v1.body.url === v2.body.url);
});
