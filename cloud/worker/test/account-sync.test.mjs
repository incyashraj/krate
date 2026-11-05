/* A person's own app data on every device (IC-981, account-sync.js).
 *
 * The REAL worker over a counting KV, signed in as GitHub users through the
 * stub (gho_<login>). What it proves: only a signed-in account reaches its
 * keys and buckets, and never another's; a device gets the data key only
 * when a device that holds it (or the recovery code) passes it on; the hub
 * keeps every version no other replaces and drops only what a newer one
 * replaced; and a sync costs one KV write when it changed something, none
 * when it did not.
 *
 *   node --experimental-wasm-modules cloud/worker/test/account-sync.test.mjs
 */
import assert from "node:assert/strict";
import test from "node:test";
import "./github-stub.mjs";
import worker from "../src/index.js";
import { covers, keepVersion } from "../src/account-sync.js";

const KV = new Map();
const writes = [];
const env = {
  PUBLIC_BASE: "https://hub.test",
  APPS: {
    __standIn: true,
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => {
      writes.push(k);
      KV.set(k, v);
    },
    delete: async (k) => {
      KV.delete(k);
    },
    list: async ({ prefix = "" } = {}) => ({ keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
  },
};

async function call(who, method, path, payload) {
  const headers = {};
  if (who) headers.authorization = `Bearer gho_${who}`;
  let body;
  if (payload !== undefined) {
    headers["content-type"] = "application/json";
    body = JSON.stringify(payload);
  }
  const res = await worker.fetch(new Request(`https://hub.test${path}`, { method, headers, body }), env, { waitUntil() {} });
  const text = await res.text();
  let parsed = null;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = text;
  }
  return { status: res.status, body: parsed };
}

async function deviceIdOf(hex) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(hex));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("").slice(0, 16);
}

async function device(seed) {
  const public_key = seed.repeat(64).slice(0, 64);
  return { public_key, id: await deviceIdOf(public_key) };
}

const blob = (s) => Buffer.from(s).toString("base64");
const params = { alg: "argon2id", salt: "00".repeat(16), m_kib: 19 * 1024, t: 2, p: 1 };
const BUCKET = "ab".repeat(32);
const NAME = "cd".repeat(32);

test("nothing without a sign-in", async () => {
  assert.equal((await call(null, "GET", "/account/sync/keys")).status, 401);
  assert.equal((await call(null, "GET", `/account/sync/buckets/${BUCKET}`)).status, 401);
  assert.equal((await call("nobody-at-all-x", "GET", "/account/sync/keys", undefined)).status, 200, "any GitHub user is an account");
});

test("the first device makes the key; a second waits until a device that holds it says yes", async () => {
  const mac = await device("1a");
  const phone = await device("2b");
  let r = await call("ada", "PUT", `/account/sync/devices/${mac.id}`, { public_key: mac.public_key, name: "Mac" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  // A device id that is not its key's is refused.
  r = await call("ada", "PUT", `/account/sync/devices/${phone.id}`, { public_key: mac.public_key, name: "x" });
  assert.equal(r.status, 400);
  r = await call("ada", "GET", `/account/sync/keys?device=${mac.id}`);
  assert.equal(r.body.key, null);
  assert.ok(r.body.account, "the view names the account");
  assert.equal(r.body.device.device_id, mac.id);

  r = await call("ada", "POST", "/account/sync/init", {
    device_id: mac.id, version: 1, public_key: "ee".repeat(32), envelope: blob("sealed to the mac"),
    recovery: { public_key: "ff".repeat(32), params, blob: blob("sealed to the code") },
  });
  assert.equal(r.status, 201, JSON.stringify(r.body));
  r = await call("ada", "POST", "/account/sync/init", {
    device_id: mac.id, version: 1, public_key: "ee".repeat(32), envelope: blob("again"),
    recovery: { public_key: "ff".repeat(32), params, blob: blob("again") },
  });
  assert.equal(r.status, 409);
  assert.equal(r.body.error, "key-exists", "one key per account");

  assert.equal((await call("ada", "PUT", `/account/sync/devices/${phone.id}`, { public_key: phone.public_key, name: "Galaxy S22 Ultra" })).status, 200);
  r = await call("ada", "GET", `/account/sync/keys?device=${phone.id}`);
  assert.equal(r.body.envelope, null, "the phone holds nothing yet");
  assert.equal(r.body.recovery.public_key, "ff".repeat(32));
  assert.equal(r.body.recovery.blob, undefined, "the view carries no envelope but the device's own");
  const listed = r.body.devices.find((d) => d.device_id === phone.id);
  assert.equal(listed.holds, null);
  assert.equal(listed.name, "Galaxy S22 Ultra");
  assert.equal(r.body.devices.find((d) => d.device_id === mac.id).holds, 1);

  // Only a device that holds the key can pass it on.
  r = await call("ada", "POST", "/account/sync/grants", { device_id: phone.id, version: 1, blob: blob("x"), by_device: phone.id });
  assert.equal(r.status, 403);
  r = await call("ada", "POST", "/account/sync/grants", { device_id: phone.id, version: 1, blob: blob("sealed to the phone"), by_device: mac.id });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  r = await call("ada", "GET", `/account/sync/keys?device=${phone.id}`);
  assert.equal(r.body.envelope.blob, blob("sealed to the phone"));
  assert.equal(r.body.envelope.granted_device, mac.id);

  // The recovery envelope, for a device with no other at hand.
  r = await call("ada", "GET", "/account/sync/recovery");
  assert.equal(r.body.blob, blob("sealed to the code"));
  assert.deepEqual(r.body.params, params);
  const tablet = await device("3c");
  await call("ada", "PUT", `/account/sync/devices/${tablet.id}`, { public_key: tablet.public_key, name: "iPad" });
  r = await call("ada", "POST", "/account/sync/grants", { device_id: tablet.id, version: 1, blob: blob("from the code"), by_device: "recovery" });
  assert.equal(r.status, 200);

  // Forgetting a device takes its envelope.
  assert.equal((await call("ada", "DELETE", `/account/sync/devices/${tablet.id}`)).status, 200);
  r = await call("ada", "GET", `/account/sync/keys?device=${tablet.id}`);
  assert.equal(r.body.device, null);
  assert.equal(r.body.envelope, null);
  assert.ok(!r.body.devices.some((d) => d.device_id === tablet.id));
});

test("another account sees none of it", async () => {
  const mac = await device("1a");
  let r = await call("bob", "GET", `/account/sync/keys?device=${mac.id}`);
  assert.equal(r.body.key, null);
  assert.equal(r.body.device, null, "ada's device is not bob's");
  assert.deepEqual(r.body.devices, []);
  r = await call("bob", "POST", "/account/sync/grants", { device_id: mac.id, version: 1, blob: blob("x"), by_device: mac.id });
  assert.notEqual(r.status, 200);
});

const A = "aaaaaaaaaaaaaaaa";
const B = "bbbbbbbbbbbbbbbb";
const w = (vv, c, by = Object.keys(vv)[0], kind = "v") => ({ name: NAME, vv, by, kind, k: 1, c: blob(c) });

test("a bucket keeps every version no other replaces, in one write per sync", async () => {
  writes.length = 0;
  let r = await call("ada", "GET", `/account/sync/buckets/${BUCKET}`);
  assert.deepEqual(r.body, { rev: 0, entries: {} });
  assert.equal(writes.length, 0, "a read writes nothing");

  r = await call("ada", "PUT", `/account/sync/buckets/${BUCKET}`, { writes: [w({ [A]: 1 }, "mac 1"), { ...w({ [A]: 1 }, "other"), name: "ef".repeat(32) }] });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.rev, 1);
  assert.equal(writes.length, 1, "two entries, one KV write");

  // Made knowing it: replaces it.
  r = await call("ada", "PUT", `/account/sync/buckets/${BUCKET}`, { writes: [w({ [A]: 1, [B]: 1 }, "phone on top")] });
  assert.deepEqual(r.body.entries[NAME].map((v) => v.c), [blob("phone on top")]);
  // Made apart: both kept.
  r = await call("ada", "PUT", `/account/sync/buckets/${BUCKET}`, { writes: [w({ [A]: 2 }, "mac apart")] });
  assert.deepEqual(r.body.entries[NAME].map((v) => v.c).sort(), [blob("mac apart"), blob("phone on top")].sort());
  // An older one, or one already held, changes nothing and writes nothing.
  const before = writes.length;
  r = await call("ada", "PUT", `/account/sync/buckets/${BUCKET}?since=${r.body.rev}`, { writes: [w({ [A]: 1 }, "stale"), w({ [A]: 2 }, "mac apart")] });
  assert.equal(writes.length, before, "nothing changed, nothing written");
  assert.equal(r.body.unchanged, true);
  // Settled: a version that has seen both replaces both.
  r = await call("ada", "PUT", `/account/sync/buckets/${BUCKET}`, { writes: [w({ [A]: 3, [B]: 1 }, "settled")] });
  assert.deepEqual(r.body.entries[NAME].map((v) => v.c), [blob("settled")]);
  const rev = r.body.rev;
  // Nothing new since: no bytes.
  r = await call("ada", "GET", `/account/sync/buckets/${BUCKET}?since=${rev}`);
  assert.deepEqual(r.body, { rev, unchanged: true });
  r = await call("ada", "GET", `/account/sync/buckets/${BUCKET}?since=${rev - 1}`);
  assert.ok(r.body.entries[NAME]);

  // Another account naming the same bucket reaches its own, empty one.
  r = await call("bob", "GET", `/account/sync/buckets/${BUCKET}`);
  assert.deepEqual(r.body, { rev: 0, entries: {} });
  const keys = [...KV.keys()].filter((k) => k.includes(BUCKET));
  assert.equal(keys.length, 1, "only ada's bucket exists");
});

test("what is not a sealed entry is refused", async () => {
  for (const bad of [
    { ...w({ [A]: 1 }, "x"), name: "not-hex" },
    { ...w({ [A]: 1 }, "x"), vv: {} },
    { ...w({ [A]: 1 }, "x"), vv: { [A]: 0 } },
    { ...w({ [A]: 1 }, "x"), vv: { nothex: 1 } },
    { ...w({ [A]: 1 }, "x"), kind: "z" },
    { ...w({ [A]: 1 }, "x"), c: "not base64!" },
    { ...w({ [A]: 1 }, "x"), by: "me" },
  ]) {
    const r = await call("ada", "PUT", `/account/sync/buckets/${BUCKET}`, { writes: [bad] });
    assert.equal(r.status, 400, JSON.stringify(bad));
  }
  assert.equal((await call("ada", "GET", "/account/sync/buckets/not-a-bucket")).status, 404);
  assert.equal((await call("ada", "PUT", `/account/sync/buckets/${BUCKET}`, { nope: 1 })).status, 400);
});

test("the version rule", () => {
  assert.ok(covers({ [A]: 2 }, { [A]: 1 }));
  assert.ok(!covers({ [A]: 1 }, { [B]: 1 }));
  const versions = [{ vv: { [A]: 1 } }];
  assert.equal(keepVersion(versions, { vv: { [B]: 1 } }), true);
  assert.equal(versions.length, 2);
  assert.equal(keepVersion(versions, { vv: { [A]: 1 } }), false, "already held");
  assert.equal(keepVersion(versions, { vv: { [A]: 1, [B]: 1 } }), true);
  assert.deepEqual(versions, [{ vv: { [A]: 1, [B]: 1 } }], "it replaced both");
});
