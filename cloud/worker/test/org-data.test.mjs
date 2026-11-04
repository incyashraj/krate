/* Team data for an organisation's apps, hub side (store.org, IC-951),
 * sealed (F-316).
 *
 * The merge rule against the vectors the runtime reads too, then the real
 * Org Durable Object over real SQLite, reached through the worker the way a
 * device reaches it, with sealed rows: who may sync (member, released, not
 * withdrawn, in the audience), per-field merge on opaque names, the wrong
 * clock that does not win, the key version, the limits, deletes and their
 * thirty days, the day's audit line, the export and the sweep -- and what
 * the store holds: only what the device sent, opaque.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-data.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, acme, fixture, worker } from "./org-tools.mjs";
import { initKey, memberDevice, sealedRow, opaque, b64 } from "./org-keys-tools.mjs";
import { directory } from "./zip-tools.mjs";
import { parseStamp, formatStamp, compareStamps, nextStamp, rebase, validValue, wins, liveFields, recordSize } from "../src/org-data-merge.js";

const here = dirname(fileURLToPath(import.meta.url));
const V = JSON.parse(readFileSync(join(here, "fixtures", "org-data-vectors.json"), "utf8"));
const DAY_MS = 86400000;
const A = "aaaaaaaaaaaaaaaa";
const B = "bbbbbbbbbbbbbbbb";
const enc = encodeURIComponent;
const pair = (r) => [r.status, r.body && r.body.error];

/* ---- the rule, against the shared vectors ------------------------------ */

test("stamps parse, print and compare as the runtime's do", () => {
  for (const c of V.parse) assert.strictEqual(parseStamp(c.text) !== null, c.ok, c.text);
  const s = parseStamp("0001760400000000-00000007-aaaaaaaaaaaaaaaa");
  assert.strictEqual(formatStamp(s), "0001760400000000-00000007-aaaaaaaaaaaaaaaa");
  for (const [x, y, want] of V.compare) assert.strictEqual(compareStamps(x, y), want, `${x} ${y}`);
  for (const c of V.next) {
    const got = nextStamp(c.last, c.phys, c.device);
    assert.strictEqual(got, c.expect, JSON.stringify(c));
    if (c.last) assert.ok(got > c.last, "above what was seen");
  }
  for (const c of V.rebase) assert.strictEqual(rebase(c.stamp, c.device_now, c.hub_now), c.expect, c.why);
});

function applyRows(store, rows) {
  for (const r of rows) {
    const k = `${r.c}\u0000${r.i}`;
    const cells = store.get(k) || {};
    if (wins(r.s, cells[r.f] ? cells[r.f].s : null)) cells[r.f] = { v: r.v, s: r.s };
    store.set(k, cells);
  }
}

function viewOf(store, id) {
  const live = liveFields(store.get(`c\u0000${id}`) || {});
  return live.length ? Object.fromEntries(live.map(([n, c]) => [n, c.v])) : null;
}

test("the merge vectors hold in every order, and twice is once", () => {
  for (const c of V.merge) {
    for (const v of c.rows) assert.ok(validValue(v.v), `${c.name}: value`);
    const forward = new Map();
    applyRows(forward, c.rows);
    const backward = new Map();
    applyRows(backward, [...c.rows].reverse());
    applyRows(backward, [...c.rows].reverse());
    for (const [id, want] of Object.entries(c.expect)) {
      assert.deepStrictEqual(viewOf(forward, id), want, `${c.name} ${id}`);
      assert.deepStrictEqual(viewOf(backward, id), want, `${c.name} ${id} reversed`);
    }
    if (c.size !== undefined) assert.strictEqual(recordSize(forward.get("c\u0000r1")), c.size, c.name);
  }
});

/* ---- the store --------------------------------------------------------- */

async function released(env, name = "candidate-v1.krate") {
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture(name) });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  const ok = await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
  assert.strictEqual(ok.status, 200, JSON.stringify(ok.body));
  return up.body.digest;
}

/* acme with a released app and its key, and the hub's clock held where a
 * test puts it. */
async function world(extra = {}) {
  const env = orgEnv({ ORG_LEASE_KEY: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60", ...extra });
  await acme(env);
  const digest = await released(env);
  const aliceDevice = await initKey(env);
  const clock = { ms: 1760400000000 };
  const org = env.ORGS.object("acme");
  org.nowMs = () => clock.ms;
  org.now = () => Math.floor(clock.ms / 1000);
  return { env, digest, clock, org, aliceDevice };
}

const sync = (env, who, digest, body) => call(env, who, "POST", `/orgs/acme/data/${digest}/sync`, { json: { device: A, now: 1760400000000, since: null, writes: [], ...body } });
const st = (wall, counter = 0, dev = A) => formatStamp({ wall, counter, device: dev });
/* The newest row for (c, i, f) among rows. */
const rowFor = (rows, c, i, f) => rows.filter((r) => r.r === opaque("r", c, i) && r.k === opaque("f", c, i, f)).sort((x, y) => (x.s < y.s ? 1 : -1))[0];

test("two members' offline edits to different fields of one record both stay, and the store holds only what was sealed", async () => {
  const { env, digest, clock, org } = await world();
  const t = clock.ms;
  const title = sealedRow("tasks", "t1", "title", st(t - 60000));
  const carol = await sync(env, "carol", digest, { now: t, writes: [title] });
  assert.strictEqual(carol.status, 200, JSON.stringify(carol.body));
  assert.deepStrictEqual([carol.body.accepted, carol.body.key_version], [1, 1]);
  assert.deepStrictEqual(carol.body.me, { name: "carol", role: "developer" });
  clock.ms += 1000;
  const done = sealedRow("tasks", "t1", "done", st(t - 30000, 0, B));
  const dan = await sync(env, "dan", digest, { device: B, now: clock.ms, writes: [done] });
  assert.strictEqual(dan.status, 200, JSON.stringify(dan.body));
  // Dan, a recipient, gets both fields of the record, sealed as they were sent.
  assert.strictEqual(rowFor(dan.body.rows, "tasks", "t1", "title").ct, title.ct);
  assert.strictEqual(rowFor(dan.body.rows, "tasks", "t1", "done").ct, done.ct);
  const again = await sync(env, "carol", digest, { now: clock.ms, since: carol.body.cursor });
  assert.deepStrictEqual(again.body.rows.map((r) => [r.k, r.by]), [[done.k, "dan"]]);
  // What the store holds is exactly what was sent, and nothing that names
  // the collection, the id or a field.
  const raw = org.rows("SELECT * FROM org_data ORDER BY seq");
  assert.deepStrictEqual(Object.keys(raw[0]), ["app_id", "rec", "fkey", "kind", "ct", "ver", "stamp", "by", "size", "seq"]);
  assert.deepStrictEqual(raw.map((r) => [r.rec, r.fkey, r.kind, r.ct, r.ver]), [
    [title.r, title.k, "v", title.ct, 1],
    [done.r, done.k, "v", done.ct, 1],
  ]);
  const dump = JSON.stringify(org.rows("SELECT * FROM org_data"));
  for (const word of ["tasks", "t1", "title", "done"]) assert.ok(!dump.includes(`"${word}"`), word);
});

test("a device whose clock is a day fast does not win over a later edit; its write comes back as what won", async () => {
  const { env, digest, clock } = await world();
  const t = clock.ms;
  const bobs = sealedRow("tasks", "t1", "title", nextStamp(null, t + DAY_MS, B));
  clock.ms = t + 20000;
  const carols = sealedRow("tasks", "t1", "title", nextStamp(null, t + 10000, A));
  assert.strictEqual((await sync(env, "carol", digest, { now: clock.ms, writes: [carols] })).body.accepted, 1);
  clock.ms = t + 30000;
  const bob = await sync(env, "bob", digest, { device: B, now: t + DAY_MS + 30000, writes: [bobs] });
  assert.strictEqual(bob.status, 200, JSON.stringify(bob.body));
  assert.strictEqual(bob.body.accepted, 0, "the earlier edit lost");
  assert.strictEqual(rowFor(bob.body.rows, "tasks", "t1", "title").ct, carols.ct, "Bob is sent what won");
  assert.ok(bob.body.rows.every((r) => r.s <= st(clock.ms + 2000, 0, "f".repeat(16))), "no stamp past the hub's now");
  assert.ok(bobs.s > carols.s, "without the rebase the fast clock would have won");
});

test("a clock a day slow is not lost to older edits", async () => {
  const { env, digest, clock } = await world();
  const t = clock.ms;
  await sync(env, "carol", digest, { now: t, writes: [sealedRow("tasks", "t1", "title", nextStamp(null, t, A))] });
  clock.ms = t + 20000;
  const second = sealedRow("tasks", "t1", "title", nextStamp(null, t + 10000 - DAY_MS, B));
  const bob = await sync(env, "bob", digest, { device: B, now: t + 20000 - DAY_MS, writes: [second] });
  assert.strictEqual(bob.body.accepted, 1, "the later edit wins");
  const back = rowFor(bob.body.rows, "tasks", "t1", "title");
  assert.deepStrictEqual([back.ct, parseStamp(back.s).wall], [second.ct, t + 10000]);
});

test("writes go under the current key: none before there is one, an older version is refused after a rotation", async () => {
  const env = orgEnv();
  await acme(env);
  const digest = await released(env);
  const now = Date.now();
  const w = sealedRow("tasks", "t1", "title", st(now));
  assert.deepStrictEqual(pair(await sync(env, "carol", digest, { now, writes: [w] })), [409, "no-org-key"]);
  assert.strictEqual((await sync(env, "carol", digest, { now })).status, 200, "a pull with nothing to send is fine");
  const alice = await initKey(env);
  assert.deepStrictEqual(pair(await sync(env, "carol", digest, { now, writes: [{ ...w, ver: 2 }] })), [409, "key-rotated"]);
  assert.strictEqual((await sync(env, "carol", digest, { now, writes: [w] })).body.accepted, 1);
  // Dan is removed: the key is stale, writes under v1 still go until a
  // holder rotates; after that v1 is refused.
  await memberDevice(env, "carol", alice);
  await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`);
  assert.strictEqual((await sync(env, "carol", digest, { now, writes: [sealedRow("tasks", "t2", "a", st(now + 1))] })).status, 200);
  const view = (await call(env, "alice", "GET", `/orgs/acme/keys?device=${alice.id}`)).body;
  const envelopes = Object.fromEntries(view.seal_to.map((d) => [d.device_id, b64(300)]));
  const rot = await call(env, "alice", "POST", "/orgs/acme/keys/rotate", {
    json: { version: 2, public_key: "ab".repeat(32), by_device: alice.id, envelopes, recovery_blob: b64(200) },
  });
  assert.strictEqual(rot.status, 200, JSON.stringify(rot.body));
  assert.deepStrictEqual(pair(await sync(env, "carol", digest, { now, writes: [sealedRow("tasks", "t3", "a", st(now + 2))] })), [409, "key-rotated"]);
  const v2 = await sync(env, "carol", digest, { now, writes: [sealedRow("tasks", "t3", "a", st(now + 2), { ver: 2 })] });
  assert.strictEqual(v2.body.accepted, 1);
  assert.strictEqual(v2.body.key_version, 2);
});

test("refused: no session, a stranger, a removed member, outside the audience, a withdrawn release, a waiting candidate, an unmarked app", async () => {
  const { env, digest } = await world();
  const ok = await sync(env, "carol", digest, { writes: [sealedRow("tasks", "t1", "title", st(1760400000000))] });
  assert.strictEqual(ok.status, 200);

  assert.deepStrictEqual(pair(await sync(env, null, digest, {})), [401, "not-signed-in"]);
  assert.deepStrictEqual(pair(await sync(env, "stranger", digest, {})), [403, "not-a-member"]);
  assert.deepStrictEqual(pair(await sync(env, "stranger", "0".repeat(64), {})), [403, "not-a-member"]);

  const waiting = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v2.krate") });
  assert.deepStrictEqual(pair(await sync(env, "carol", waiting.body.digest, {})), [403, "not-released"]);
  assert.deepStrictEqual(pair(await sync(env, "carol", "0".repeat(64), {})), [403, "not-released"]);
  const unmarked = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("unmarked.krate") });
  assert.deepStrictEqual(pair(unmarked), [422, "not-marked-for-this-org"]);
  const { sha256Hex } = await import("./r2-mock.mjs");
  assert.deepStrictEqual(pair(await sync(env, "carol", await sha256Hex(fixture("unmarked.krate")), {})), [403, "not-released"]);

  await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "finance" } });
  await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { groups: ["finance"] } });
  assert.deepStrictEqual(pair(await sync(env, "dan", digest, {})), [403, "not-in-audience"]);
  await call(env, "alice", "PUT", `/orgs/acme/groups/finance/members/${enc("github:dan")}`);
  assert.strictEqual((await sync(env, "dan", digest, {})).status, 200, "in the group, in the audience");
  await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { everyone: true } });

  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`)).status, 204);
  assert.deepStrictEqual(pair(await sync(env, "dan", digest, {})), [403, "removed"]);

  await call(env, "bob", "POST", `/orgs/acme/releases/${digest}/withdraw`, { json: { reason: "bad build" } });
  assert.deepStrictEqual(pair(await sync(env, "carol", digest, {})), [410, "release-withdrawn"]);

  const audit = (await call(env, "alice", "GET", "/orgs/acme/audit")).body;
  const denied = audit.events.filter((e) => e.action === "data.sync" && e.result === "denied").map((e) => e.reason);
  for (const reason of ["not-a-member", "not-released", "not-in-audience", "removed", "release-withdrawn"]) {
    assert.ok(denied.includes(reason), `${reason} audited: ${denied}`);
  }
  assert.strictEqual(audit.chain_ok, true);
});

test("a sync names the people the release reaches: members inside its audience, by name and role, and nobody else (IC-979)", async () => {
  const { env, digest } = await world();
  const people = async (who = "carol") => {
    const r = await sync(env, who, digest, {});
    assert.strictEqual(r.status, 200, JSON.stringify(r.body));
    return r.body.people;
  };
  // Everyone: the four members, sorted, name and role and nothing more.
  assert.deepStrictEqual(await people(), [
    { name: "alice", role: "owner" },
    { name: "bob", role: "reviewer" },
    { name: "carol", role: "developer" },
    { name: "dan", role: "recipient" },
  ]);
  // Narrowed to finance with Carol in it: Dan is outside, and is not named;
  // the owner and the reviewer see every release, so they are.
  await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "finance" } });
  await call(env, "alice", "PUT", `/orgs/acme/groups/finance/members/${enc("github:carol")}`);
  await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { groups: ["finance"] } });
  assert.deepStrictEqual((await people()).map((p) => p.name), ["alice", "bob", "carol"]);
  // Dan joins the group: he is named again.
  await call(env, "alice", "PUT", `/orgs/acme/groups/finance/members/${enc("github:dan")}`);
  assert.deepStrictEqual((await people("dan")).map((p) => p.name), ["alice", "bob", "carol", "dan"]);
  // Widened, then Dan removed from the organisation: gone from the list.
  await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { everyone: true } });
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`)).status, 204);
  assert.deepStrictEqual((await people()).map((p) => p.name), ["alice", "bob", "carol"]);
  // Only names and roles: no groups, no devices, no sign-in kinds.
  for (const p of await people()) assert.deepStrictEqual(Object.keys(p).sort(), ["name", "role"]);
});

test("another release of the same app id shares the data; the data is per app id", async () => {
  const { env, digest } = await world();
  const w = sealedRow("tasks", "t1", "title", st(1760400000000));
  await sync(env, "carol", digest, { writes: [w] });
  const v2 = await released(env, "candidate-v2.krate");
  const r = await sync(env, "carol", v2, { device: B });
  assert.strictEqual(rowFor(r.body.rows, "tasks", "t1", "title").ct, w.ct);
});

test("limits: names, kinds and ciphertext checked; the sealed record bound, the push size and the app's quota", async () => {
  const { env, digest } = await world({ ORG_DATA_QUOTA_BYTES: "4096" });
  const good = sealedRow("tasks", "t1", "title", st(1760400000000));
  for (const bad of [
    { ...good, r: "tasks" },
    { ...good, k: "ABC" },
    { ...good, kind: "x" },
    { ...good, s: "yesterday" },
    { ...good, ver: 0 },
    { ...good, ct: "not base64!" },
    { ...good, ct: b64(10) },
    { ...good, ct: b64(66 * 1024) },
  ]) {
    assert.deepStrictEqual(pair(await sync(env, "carol", digest, { writes: [bad] })), [400, "bad-request"], JSON.stringify(bad).slice(0, 120));
  }

  // A record over the bound, even made of two pushes.
  const big = await world();
  const t = 1760400000000;
  assert.strictEqual((await sync(big.env, "carol", big.digest, { writes: [sealedRow("notes", "n1", "a", st(t), { bytes: 40 * 1024 })] })).status, 200);
  const over = await sync(big.env, "carol", big.digest, { writes: [sealedRow("notes", "n1", "b", st(t, 1), { bytes: 40 * 1024 })] });
  assert.deepStrictEqual(pair(over), [413, "too-large"]);
  const kept = big.org.rows("SELECT fkey FROM org_data WHERE rec = ?", opaque("r", "notes", "n1"));
  assert.deepStrictEqual(kept.map((r) => r.fkey), [opaque("f", "notes", "n1", "a")], "nothing of the refused push was kept");

  const huge = await call(big.env, "carol", "POST", `/orgs/acme/data/${big.digest}/sync`, {
    bytes: new TextEncoder().encode(JSON.stringify({ device: A, now: 1, writes: [], pad: "z".repeat(1024 * 1024) })),
    headers: { "content-type": "application/json" },
  });
  assert.deepStrictEqual(pair(huge), [413, "too-large"]);

  // The quota (4 KiB of ciphertext and bookkeeping here).
  const fill = [0, 1, 2].map((n) => sealedRow("files", `f${n}`, "body", st(t, 10 + n), { bytes: 1000 }));
  const filled = await sync(env, "carol", digest, { writes: fill });
  assert.strictEqual(filled.status, 200, JSON.stringify(filled.body));
  assert.ok(filled.body.quota.used > 3000 && filled.body.quota.limit === 4096, JSON.stringify(filled.body.quota));
  assert.deepStrictEqual(pair(await sync(env, "carol", digest, { writes: [sealedRow("files", "f9", "body", st(t, 20), { bytes: 1000 })] })), [413, "over-quota"]);
  assert.strictEqual((await sync(env, "carol", digest, { writes: [sealedRow("files", "f0", "", st(t, 30))] })).status, 200, "a delete frees room");
  assert.strictEqual((await sync(env, "carol", digest, { writes: [sealedRow("files", "f9", "body", st(t, 40), { bytes: 1000 })] })).status, 200);
});

test("a delete is a tombstone for thirty days, then a device behind it starts over from the hub's copy", async () => {
  const { env, digest, clock, org } = await world();
  const t = clock.ms;
  const first = await sync(env, "carol", digest, {
    now: t,
    writes: [sealedRow("tasks", "t1", "title", nextStamp(null, t, A)), sealedRow("tasks", "t2", "title", nextStamp(null, t, A))],
  });
  const dan = await sync(env, "dan", digest, { device: B, now: t });
  assert.ok(rowFor(dan.body.rows, "tasks", "t1", "title"));
  const del = await sync(env, "carol", digest, { now: t + 1000, since: first.body.cursor, writes: [sealedRow("tasks", "t1", "", nextStamp(null, t + 1000, A))] });
  assert.strictEqual(del.body.accepted, 1);
  // The field the delete outlived is gone from the store.
  assert.strictEqual(org.row("SELECT * FROM org_data WHERE fkey = ?", opaque("f", "tasks", "t1", "title")), null);
  // An older write to t1 arriving later does not bring it back.
  const late = await sync(env, "dan", digest, { device: B, now: t + 2000, since: dan.body.cursor, writes: [sealedRow("tasks", "t1", "note", nextStamp(null, t + 500, B))] });
  assert.strictEqual(late.body.accepted, 0);
  assert.ok(late.body.rows.some((r) => r.kind === "t" && r.r === opaque("r", "tasks", "t1")), "Dan is sent the tombstone");
  clock.ms = t + 31 * DAY_MS;
  await sync(env, "carol", digest, { now: clock.ms });
  const behind = await sync(env, "dan", digest, { device: B, now: clock.ms, since: dan.body.cursor });
  assert.strictEqual(behind.body.full, true, "start over from the hub's copy");
  assert.deepStrictEqual(behind.body.rows.map((r) => r.r), [opaque("r", "tasks", "t2")]);
});

test("the audit gets one line per app per day: counts of writes and people, never content", async () => {
  const { env, digest, clock } = await world();
  const t = clock.ms;
  const secret = sealedRow("tasks", "t1", "title", nextStamp(null, t, A));
  await sync(env, "carol", digest, { now: t, writes: [secret] });
  await sync(env, "dan", digest, { device: B, now: t, writes: [sealedRow("tasks", "t1", "done", nextStamp(null, t, B))] });
  await sync(env, "dan", digest, { device: B, now: t });
  const usage = await call(env, "alice", "GET", "/orgs/acme/data");
  assert.deepStrictEqual(usage.body.apps.map((a) => [a.app_id, a.records, a.today.writes, a.today.readers]), [["dev.krate.bounce", 1, 2, 2]]);
  assert.strictEqual((await call(env, "carol", "GET", "/orgs/acme/data")).body.error, "role-cannot");
  clock.ms = t + DAY_MS;
  const audit = (await call(env, "alice", "GET", "/orgs/acme/audit")).body;
  const lines = audit.events.filter((e) => e.action === "data.daily");
  assert.deepStrictEqual(lines.map((e) => [e.actor, e.target, e.reason]), [["system:krate", "app:dev.krate.bounce", "2025-10-14: 2 writes, 2 readers"]]);
  assert.ok(!JSON.stringify(audit).includes(secret.ct.slice(0, 20)), "no ciphertext in the audit either");
  assert.strictEqual(audit.chain_ok, true);
});

function unzip(bytes) {
  const out = {};
  for (const e of directory(bytes)) {
    const nameLen = bytes[e.local + 26] | (bytes[e.local + 27] << 8);
    const extraLen = bytes[e.local + 28] | (bytes[e.local + 29] << 8);
    const start = e.local + 30 + nameLen + extraLen;
    out[e.name] = bytes.subarray(start, start + e.size);
  }
  return out;
}

test("the export carries the sealed data and the keyring's envelopes; the sweep deletes both with the organisation", async () => {
  const { env, digest, org } = await world();
  const w = sealedRow("tasks", "t1", "title", nextStamp(null, 1760400000000, A));
  await sync(env, "carol", digest, { writes: [w] });
  const ex = await call(env, "alice", "POST", "/orgs/acme/export");
  assert.strictEqual(ex.status, 202, JSON.stringify(ex.body));
  const files = unzip((await call(env, "alice", "GET", `/orgs/acme/exports/${ex.body.export.id}`)).bytes);
  const rows = JSON.parse(new TextDecoder().decode(files["app-data.json"]));
  assert.deepStrictEqual(rows.map((r) => [r.app_id, r.record, r.field, r.kind, r.key_version, r.ciphertext, r.by]), [
    ["dev.krate.bounce", w.r, w.k, "v", 1, w.ct, "github:carol"],
  ]);
  const keyring = JSON.parse(new TextDecoder().decode(files["keyring.json"]));
  assert.strictEqual(keyring.versions.length, 1);
  assert.strictEqual(keyring.envelopes.length, 1);
  assert.ok(keyring.recovery.params.includes("argon2id"));
  const close = await call(env, "alice", "POST", "/orgs/acme/close", { json: { confirm_slug: "acme" } });
  assert.strictEqual(close.status, 201, JSON.stringify(close.body));
  const ends = (await call(env, "alice", "GET", "/orgs/acme")).body.org.close.ends_at;
  org.now = () => ends + 31 * 86400;
  assert.strictEqual((await sync(env, "carol", digest, {})).body.error, "org-closed");
  await worker.scheduled({ cron: "17 3 * * *" }, env);
  assert.strictEqual(org.row("SELECT COUNT(*) AS n FROM org_data").n, 0, "data deleted with the organisation");
  assert.strictEqual(org.row("SELECT COUNT(*) AS n FROM key_envelopes").n, 0, "envelopes too");
  const lines = org.rows("SELECT action FROM audit WHERE action IN ('data.deleted', 'key.deleted')").map((r) => r.action);
  assert.deepStrictEqual(lines.sort(), ["data.deleted", "key.deleted"]);
});
