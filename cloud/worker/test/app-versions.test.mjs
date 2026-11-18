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
import { createHash } from "node:crypto";
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

test("the app's own link is never cached as one build; a version's own link is (IC-389)", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  const line = v1.body.url.split("/a/")[1];
  const first = await call(env, null, "GET", `/a/${line}?dl=1`);
  assert.equal(first.status, 200);
  assert.equal(first.headers.get("x-krate-sha256"), v1.body.id, "the exact file served is named");
  assert.equal(first.headers.get("etag"), `"${v1.body.id}"`);
  const cc = first.headers.get("cache-control");
  assert.ok(!/immutable|max-age=[1-9]/.test(cc), `a moving link must be asked again, got: ${cc}`);

  // Nothing moved: the browser's copy is still right.
  const same = await call(env, null, "GET", `/a/${line}?dl=1`, { headers: { "if-none-match": `"${v1.body.id}"` } });
  assert.equal(same.status, 304);

  // An update: the copy the browser holds is v1, and the link now gives v2.
  const v2 = await publish(env, "alice", "0.2.0");
  const after = await call(env, null, "GET", `/a/${line}?dl=1`, { headers: { "if-none-match": `"${v1.body.id}"` } });
  assert.equal(after.status, 200, "an old copy is replaced, not confirmed");
  assert.equal(after.headers.get("x-krate-sha256"), v2.body.id);

  // A version's own address never changes, so it may be cached forever.
  const fixed = await call(env, null, "GET", `/a/${v1.body.id}?dl=1`);
  assert.equal(fixed.status, 200);
  assert.match(fixed.headers.get("cache-control"), /immutable/);
  assert.equal(fixed.headers.get("x-krate-sha256"), v1.body.id);
});

// Notes 0.1.0 and 0.2.0 signed by publisher A, 0.2.0 by publisher B, and
// 0.3.0 by a release key A delegated to (a rotation: still A's root), all
// with `krate sign --namespace notes`.
const signed = (name) => new Uint8Array(readFileSync(join(here, "fixtures", `${name}.krate`)));
const publishBytes = (env, who, bytes) =>
  call(env, who, "POST", "/publish", { bytes, headers: { "x-krate-name": "Notes" } });
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");

test("an update keeps the app's publisher: another key, or none, is refused before anything is stored (K-1286)", async () => {
  const env = hubEnv();
  const v1 = await publishBytes(env, "alice", signed("notes-0.1.0-signed-a"));
  assert.equal(v1.status, 200, JSON.stringify(v1.body));

  const other = signed("notes-0.2.0-signed-b");
  const refused = await publishBytes(env, "alice", other);
  assert.equal(refused.status, 409, "another publisher's key is not this app's update");
  assert.match(String(refused.body), /different publisher/);
  assert.equal(env._kv.get(`owner:${sha(other)}`), undefined, "nothing written for a refused update");
  assert.equal(env._kv.get(`app:${sha(other)}`), undefined);

  const unsignedNext = await publish(env, "alice", "0.2.0");
  assert.equal(unsignedNext.status, 409, "an unsigned update of a signed app is refused too");
  assert.match(String(unsignedNext.body), /is not/);

  const same = await publishBytes(env, "alice", signed("notes-0.2.0-signed-a"));
  assert.equal(same.status, 200, JSON.stringify(same.body));
  assert.deepEqual([same.body.url, same.body.version], [v1.body.url, 2], "the same key is the next version");

  const rotated = await publishBytes(env, "alice", signed("notes-0.3.0-signed-a-rotated"));
  assert.equal(rotated.status, 200, "a key the publisher's root delegated to is the same publisher");
  assert.equal(rotated.body.version, 3);
});

// The same two builds with their zip entries written in reverse order:
// other bytes, the same release (K-1285).
test("another file of a release the app already has is that version, not the next (K-1285)", async () => {
  for (const [first, repack, label] of [
    [signed("notes-0.2.0-signed-a"), signed("notes-0.2.0-signed-a-repacked"), "signed: the same release id"],
    [build("0.2.0"), signed("notes-0.2.0-repacked"), "unsigned: the same execution and project"],
  ]) {
    const env = hubEnv();
    assert.notEqual(sha(first), sha(repack), "the fixtures must differ in bytes or this proves nothing");
    const v1 = await publishBytes(env, "alice", first);
    const again = await publishBytes(env, "alice", repack);
    assert.equal(again.status, 200, JSON.stringify(again.body));
    assert.equal(again.body.version, 1, `${label}: no new version`);
    assert.equal(again.body.updated, false);
    assert.equal(await opens(env, v1.body.url.split("/a/")[1]), sha(first), `${label}: the app's link still opens the original`);
    assert.equal(row(env, sha(repack)).superseded, true, `${label}: one gallery row`);
    assert.equal(await opens(env, sha(repack)), sha(repack), "the repack opens at its own address");
  }
});

test("a blocked update is recoverable: the app's link goes back to the newest safe version (IC-669, 1311)", async () => {
  const env = hubEnv();
  env.KRATE_ADMINS = "root";
  const v1 = await publish(env, "alice", "0.1.0");
  const v2 = await publish(env, "alice", "0.2.0");
  const v3 = await publish(env, "alice", "0.3.0");
  const line = v1.body.url.split("/a/")[1];
  assert.equal(await opens(env, line), v3.body.id);

  const takedown = (hash) =>
    call(env, "root", "POST", `/admin/takedown/${hash}`, { json: { reason: "malware in this update" } });

  // An older version blocked: the app's link does not move.
  const old = await takedown(v2.body.id);
  assert.equal(old.status, 200, JSON.stringify(old.body));
  assert.equal(old.body.takedown.stepped_back, undefined, "blocking a version nobody opens moves nothing");
  assert.equal(await opens(env, line), v3.body.id);

  // The current version blocked: the link goes to v1, skipping blocked v2,
  // and the takedown says so.
  const now = await takedown(v3.body.id);
  assert.deepEqual(
    [now.body.takedown.stepped_back.from, now.body.takedown.stepped_back.to, now.body.takedown.stepped_back.to_hash],
    [3, 1, v1.body.id],
    "the move is on the takedown's record",
  );
  assert.equal(await opens(env, line), v1.body.id, "the app's link opens the newest safe version");
  assert.equal(await opens(env, v3.body.id), 451, "the blocked build itself stays blocked");
  const meta = await call(env, null, "GET", `/meta/${line}`);
  assert.equal(meta.body.id, v1.body.id);

  // Nothing safe left: the link answers as the blocked build does.
  const last = await takedown(v1.body.id);
  assert.equal(last.body.takedown.stepped_back.to, null);
  assert.equal(await opens(env, line), 451);
});

test("search ranks by relevance, shows verified identity apart from it, and sells no placement (IC-669, 1315)", async () => {
  const env = hubEnv();
  const pub = (who, v, name, description) =>
    call(env, who, "POST", "/publish", {
      bytes: build(v),
      headers: { "x-krate-name": name, "x-krate-description": description },
    });
  const editor = await pub("alice", "0.1.0", "Photo Editor", "crop and colour your pictures");
  const kit = await pub("bob", "0.2.0", "Photographer Kit", "tools for a shoot");
  // Newest, and the least relevant: only its description says it.
  const journal = await pub("carol", "0.3.0", "Daybook", "a photo a day journal");
  for (const r of [editor, kit, journal]) assert.equal(r.status, 200, JSON.stringify(r.body));
  // Published in one second, newest would be a tie and any order could pass;
  // spread them so the least relevant is plainly the newest.
  [editor, kit, journal].forEach((r, i) => {
    const key = `app:${r.body.id}`;
    env._kv.set(key, JSON.stringify({ ...JSON.parse(env._kv.get(key)), published: 1_000 + i * 1_000 }));
  });

  const search = async (q) => (await call(env, null, "GET", `/apps?q=${encodeURIComponent(q)}`)).body;
  const photo = await search("photo");
  assert.deepEqual(
    photo.results.map((a) => a.meta.name),
    ["Photo Editor", "Photographer Kit", "Daybook"],
    "a word in the name ranks above a word in the description, however new",
  );
  assert.deepEqual(photo.results[0].matched, ["name"]);
  assert.deepEqual(photo.results[2].matched, ["description"]);
  assert.match(photo.ranking.order, /relevance/);
  assert.match(photo.ranking.paid_placement, /^none/);
  assert.match(photo.ranking.verified_identity, /never moves an app up/);
  for (const a of photo.results) {
    assert.equal(typeof a.identity_checks.verified_publisher, "boolean", "every result says who stands behind it");
    assert.equal(typeof a.identity_checks.signed, "boolean");
  }

  // Every word must match: "photo editor" is one app, not three.
  assert.deepEqual((await search("photo editor")).results.map((a) => a.meta.name), ["Photo Editor"]);
  assert.deepEqual((await search("nothing like this")).results, []);
});

test("a publisher's compromised key: its later releases are blocked, the app steps back, the key signs nothing more (IC-669)", async () => {
  const env = hubEnv();
  const v1 = await publishBytes(env, "alice", signed("notes-0.1.0-signed-a"));
  const v2 = await publishBytes(env, "alice", signed("notes-0.2.0-signed-a"));
  assert.deepEqual([v1.status, v2.status, v2.body.version], [200, 200, 2]);
  const line = v1.body.url.split("/a/")[1];
  const keyA = row(env, v1.body.id).release.authority;
  // v1 reached the hub before the theft, v2 after: arrival times apart.
  const stored = JSON.parse(env._kv.get(`line:${line}`));
  stored.versions[0].at = 1_000;
  stored.versions[1].at = 3_000;
  env._kv.set(`line:${line}`, JSON.stringify(stored));
  const report = (who, body) => call(env, who, "POST", "/publisher-keys/compromised", { json: body });

  assert.equal((await report("bob", { key: keyA })).status, 403, "only the account that published with the key may report it");
  assert.equal((await report("alice", { key: "not a key" })).status, 400);
  assert.equal(env._kv.get(`compromised:${keyA}`), undefined, "a refused report records nothing");

  const ok = await report("alice", { key: keyA, since: 2_000, reason: "my laptop was stolen" });
  assert.equal(ok.status, 200, JSON.stringify(ok.body));
  assert.deepEqual(ok.body.blocked, [v2.body.id], "only what arrived after the theft is blocked");
  assert.equal(await opens(env, v2.body.id), 451);
  assert.equal(await opens(env, line), v1.body.id, "the app's link steps back to the safe version");
  const blocklist = await call(env, null, "GET", "/takedowns");
  assert.ok(blocklist.body.blocked.some((b) => b.hash === v2.body.id && b.emergency), "it joins the offline blocklist");

  // The key signs nothing more, even a rotated key of it.
  const again = await publishBytes(env, "alice", signed("notes-0.3.0-signed-a-rotated"));
  assert.equal(again.status, 409);
  assert.match(String(again.body), /reported compromised/);

  // A new key is the way back for the app.
  const fresh = await publishBytes(env, "alice", signed("notes-0.2.0-signed-b"));
  assert.equal(fresh.status, 200, JSON.stringify(fresh.body));
  assert.deepEqual([fresh.body.url, fresh.body.version], [v1.body.url, 3]);
  assert.equal(await opens(env, line), fresh.body.id);
});

test("an unsigned app may take its first publisher", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  const v2 = await publishBytes(env, "alice", signed("notes-0.2.0-signed-a"));
  assert.equal(v2.status, 200, JSON.stringify(v2.body));
  assert.deepEqual([v2.body.url, v2.body.version], [v1.body.url, 2]);
  // From then on, the line holds that publisher.
  const b = await publishBytes(env, "alice", signed("notes-0.2.0-signed-b"));
  assert.equal(b.status, 409);
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

test("a line id taken first by another owner does not take an app off versioning", async () => {
  // The derived line id is 44 bits of a hash anyone can compute, so a
  // stranger who knows alice's owner and her app id can search out an app
  // id of their own that lands on it, and publish first. Planted here
  // directly: the record such a search would leave.
  const { lineIdOf } = await import("../src/lines.js");
  const env = hubEnv();
  const appId = "dev.krate.sql-check";
  const taken = await lineIdOf("github:alice", appId);
  env._kv.set(`line:${taken}`, JSON.stringify({ id: taken, owner: "github:mallory", app_id: "x.y", name: "Squat", current: 1, versions: [{ n: 1, hash: "f".repeat(64) }] }));
  const v1 = await publish(env, "alice", "0.1.0");
  assert.equal(v1.status, 200, JSON.stringify(v1.body));
  assert.equal(JSON.parse(env._kv.get(`line:${taken}`)).owner, "github:mallory", "the fixture really collides with alice's derived id");
  assert.equal(v1.body.version, 1, "alice's app still gets a line");
  const id = v1.body.url.split("/a/")[1];
  assert.notEqual(id, taken, "not the taken id");
  const v2 = await publish(env, "alice", "0.2.0");
  assert.equal(v2.body.url, v1.body.url, "her update lands on the same line");
  assert.equal(v2.body.version, 2);
  const mine = await call(env, "alice", "GET", `/lines?app=${appId}`);
  assert.equal(mine.status, 200, JSON.stringify(mine.body));
  assert.equal(mine.body.id, id, "Studio finds her line by app id");
  assert.equal(JSON.parse(env._kv.get(`line:${taken}`)).owner, "github:mallory", "the squatted record is untouched");
});

test("a tab's storage tag is the vouched publisher's, so two publishers of one app id never share storage", async () => {
  const { storeTagOf } = await import("../src/lines.js");
  const alice = await storeTagOf("github:alice");
  assert.match(alice, /^k[0-9a-f]{16}$/);
  assert.equal(await storeTagOf("github:alice"), alice, "the same publisher, the same tag, every version");
  assert.notEqual(await storeTagOf("github:mallory"), alice, "another publisher, another tag");
  assert.notEqual(await storeTagOf("account:1234"), alice);
  assert.equal(await storeTagOf(""), null, "no vouched owner, no tag: the Player keys by the build instead");
});

test("a link's bytes carry the same owner tag the tab keys storage by (K-1172)", async () => {
  const { storeTagOf } = await import("../src/lines.js");
  const env = hubEnv();
  const alice = await publish(env, "alice", "0.1.0");
  const bob = await publish(env, "bob", "0.2.0");
  const bytesTag = async (hash) => {
    const res = await call(env, null, "GET", `/a/${hash}?dl=1`);
    assert.equal(res.status, 200, `the bytes download: ${res.status}`);
    return res.headers.get("x-krate-store");
  };
  const a = await bytesTag(alice.body.id);
  assert.equal(a, await storeTagOf(row(env, alice.body.id).owner), "from the owner the hub recorded");
  assert.equal(a, (await call(env, null, "GET", `/a/${alice.body.id}/web`)).headers.get("x-krate-store"),
    "the same tag a tab gets, so a phone and a tab key one app's data alike");
  assert.notEqual(await bytesTag(bob.body.id), a, "another publisher of the same app id is another store");
  const head = await call(env, null, "HEAD", `/a/${alice.body.id}?dl=1`);
  assert.equal(head.headers.get("x-krate-store"), a, "a HEAD says the same");
});

test("a tab's storage is keyed by the vouched publisher: the web pack names a tag per owner", async () => {
  const { storeTagOf } = await import("../src/lines.js");
  const env = hubEnv();
  const alice = await publish(env, "alice", "0.1.0");
  const bob = await publish(env, "bob", "0.2.0");
  const tagOf = async (hash) => (await call(env, null, "GET", `/a/${hash}/web`)).headers.get("x-krate-store");
  const a = await tagOf(alice.body.id);
  const b = await tagOf(bob.body.id);
  assert.match(a || "", /^k[0-9a-f]{16}$/, "the web pack names whose app it is");
  assert.equal(a, await storeTagOf(row(env, alice.body.id).owner), "from the owner the hub recorded");
  assert.notEqual(a, b, "another publisher of the same app id is another store");
  const v2 = await publish(env, "alice", "0.3.0", { "x-krate-name": "Notes again" });
  assert.equal(await tagOf(v2.body.id), a, "a publisher's later versions keep their store");
});
