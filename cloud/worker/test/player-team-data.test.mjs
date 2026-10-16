/* An organisation's team data in a browser tab (store.org, G1), the browser
 * path end to end in Node:
 *
 * - the shared vectors through validator.wasm, the module a tab runs: the
 *   merge rule (cloud/worker/test/fixtures/org-data-vectors.json, which the
 *   runtime and the hub also pass) and sealed rows byte for byte the same as
 *   the computer's (crates/orgdata/tests/vectors/org-records.json), made
 *   through the tab's own sync path and opened by it; a tampered
 *   ciphertext is refused;
 * - the Player's team data (crates/adapter-web/player/web/org-data.js) and
 *   the browser-as-a-device code (docs/landing/kit/org-device.js) against
 *   the real hub Worker and Org Durable Object: a tab waits for the key,
 *   gets it from a colleague's computer in the background, writes a record a
 *   computer reads and reads one a computer wrote -- with the real
 *   sealing -- and is refused outside the app's audience;
 * - the worker's store.org imports (hosts.js): jco's shapes to the copy's
 *   wire shapes and back.
 *
 *   scripts/build-hub-validator.sh
 *   node --experimental-wasm-modules --test cloud/worker/test/player-team-data.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, acme, fixture, tokenFor, worker } from "./org-tools.mjs";
import "../../../docs/landing/kit/org-device.js";
import { TeamData } from "../../../crates/adapter-web/player/web/org-data.js";
import { buildImports, HostError } from "../../../crates/adapter-web/player/web/hosts.js";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..", "..");
const S = globalThis.KrateSigning;
const MODULE = readFileSync(join(here, "..", "src", "validator.wasm"));
const MERGE = JSON.parse(readFileSync(join(here, "fixtures", "org-data-vectors.json"), "utf8"));
const RECORDS = JSON.parse(readFileSync(join(repo, "crates", "orgdata", "tests", "vectors", "org-records.json"), "utf8"));
const HUB = "https://hub.example";
const b64 = (bytes) => Buffer.from(bytes).toString("base64");
const unb64 = (text) => new Uint8Array(Buffer.from(text, "base64"));

let mod;
const module = async () => (mod ||= await S.loadModule(MODULE));

/// A device's key, as a browser keeps it (WebCrypto X25519, in memory here).
const deviceOf = (name) => S.openDevice({ store: S.memoryStore(), name });

/// A keyring for `org` made from `secret` and sealed to `device`.
async function keyringFor(m, device, secret, org = "acme") {
  const pair = await crypto.subtle.generateKey({ name: "X25519" }, true, ["deriveBits"]);
  const jwk = await crypto.subtle.exportKey("jwk", pair.privateKey);
  const s = secret || Buffer.from(jwk.d, "base64url").toString("hex");
  return m.keys({ op: "keyring_create", org, secret: s, device_public: device.public_key });
}

/* ---- the shared vectors, through the module a tab runs ------------------ */

test("the merge vectors hold in the tab's module: stamps, and rows merged into its copy in every order", async () => {
  const m = await module();
  const d = (op) => {
    const a = m.data(op);
    assert.ok(a.ok, JSON.stringify(a));
    return a;
  };
  for (const c of MERGE.parse) assert.strictEqual(d({ op: "stamp_parse", text: c.text }).valid, c.ok, c.text);
  for (const [x, y, want] of MERGE.compare) assert.strictEqual(d({ op: "stamp_compare", a: x, b: y }).order, want, `${x} ${y}`);
  for (const c of MERGE.next) assert.strictEqual(d({ op: "stamp_next", last: c.last, phys: c.phys, device: c.device }).stamp, c.expect, JSON.stringify(c));
  for (const c of MERGE.rebase) assert.strictEqual(d({ op: "rebase", stamp: c.stamp, device_now: c.device_now, hub_now: c.hub_now }).stamp, c.expect, c.why);

  // Rows as a hub would hand them, sealed with a keyring this copy holds,
  // then folded in by the copy's own sync path.
  const device = await deviceOf("vectors");
  const ring = await keyringFor(m, device);
  const keys = { org: "acme", app_id: "dev.acme.vectors", envelope: ring.envelope, device_secret: await device.secret() };
  const fold = (store, rows) => {
    d({ ...keys, op: "sync_begin", store, now: 1760400000000, entropy: b64(new Uint8Array(64)) });
    d({ ...keys, op: "sync_end", store, answer: { now: 1760400000000, rows, cursor: "1" } });
  };
  let n = 0;
  for (const c of MERGE.merge) {
    const sealed = m.data({ ...keys, op: "seal_rows", rows: c.rows.map((r) => ({ ...r, by: "vector" })) }, 24 * c.rows.length).rows;
    const rows = sealed.map((r, i) => ({ ...r, by: "vector", s: c.rows[i].s }));
    const forward = `merge-f-${n}`;
    const backward = `merge-b-${n++}`;
    d({ op: "open", store: forward, device: "0000000000000001" });
    d({ op: "open", store: backward, device: "0000000000000002" });
    fold(forward, rows);
    fold(backward, [...rows].reverse());
    fold(backward, [...rows].reverse());
    const view = (store, id) => {
      const e = d({ op: "get", store, collection: "c", id }).entry;
      return e ? Object.fromEntries(e.fields) : null;
    };
    for (const [id, want] of Object.entries(c.expect)) {
      assert.deepStrictEqual(view(forward, id), want, `${c.name} ${id}`);
      assert.deepStrictEqual(view(backward, id), want, `${c.name} ${id}, reversed and twice`);
    }
  }
});

test("rows sealed in a tab are byte for byte the computer's, they open there, and a tampered one is refused", async () => {
  const m = await module();
  const device = await deviceOf("records");
  const ring = await keyringFor(m, device, RECORDS.org_secret, RECORDS.org);
  assert.strictEqual(ring.public_key, RECORDS.org_public, "the same organisation key from the same secret");
  const keys = { org: RECORDS.org, app_id: RECORDS.app_id, envelope: ring.envelope, device_secret: await device.secret() };
  const store = "records";
  assert.ok(m.data({ op: "open", store, device: "0123456789abcdef" }).ok);
  let now = 1760400000000;
  // Each case written through the app's calls and sealed by the tab's own
  // sync, with the vector's nonce as the page's randomness.
  for (const c of RECORDS.cases) {
    if (c.f === "") {
      // A delete: the record has to be there first.
      assert.ok(m.data({ op: "put", store, collection: c.c, id: c.i, now: now++, me: c.w, fields: [["x", { t: "x" }]] }).ok);
      m.data({ ...keys, op: "sync_begin", store, now: now++ }, 64);
      m.data({ ...keys, op: "sync_end", store, answer: { now, rows: [] } });
      assert.ok(m.data({ op: "delete", store, collection: c.c, id: c.i, now: now++, me: c.w }).written);
    } else {
      assert.ok(m.data({ op: "put", store, collection: c.c, id: c.i, now: now++, me: c.w, fields: [[c.f, c.v]] }).written);
    }
    const begin = m.data({ ...keys, op: "sync_begin", store, now: now++, entropy: b64(Buffer.from(c.nonce, "hex")) });
    assert.ok(begin.ok, JSON.stringify(begin));
    assert.strictEqual(begin.body.writes.length, 1, c.f);
    const w = begin.body.writes[0];
    assert.deepStrictEqual([w.r, w.k, w.kind, w.ver, w.ct], [c.r, c.k, c.kind, RECORDS.version, c.ct], `${c.c}/${c.i}/${c.f}: the computer's bytes`);
    assert.ok(m.data({ ...keys, op: "sync_end", store, answer: { now, rows: [] } }).ok);
  }

  // The computer's rows, opened by a fresh tab copy.
  const reader = "records-reader";
  assert.ok(m.data({ op: "open", store: reader, device: "fedcba9876543210" }).ok);
  const rows = RECORDS.cases.map((c, i) => ({ r: c.r, k: c.k, kind: c.kind, ver: 1, ct: c.ct, s: `00017604000000${String(i).padStart(2, "0")}-00000000-0123456789abcdef`, by: "the hub says" }));
  m.data({ ...keys, op: "sync_begin", store: reader, now }, 64);
  const end = m.data({ ...keys, op: "sync_end", store: reader, answer: { now, rows, cursor: "9" } });
  assert.strictEqual(end.rejected, 0, JSON.stringify(end));
  const s1 = m.data({ op: "get", store: reader, collection: "sites", id: "s1" }).entry;
  assert.deepStrictEqual(Object.fromEntries(s1.fields), { name: { t: "North depot" }, passed: { b: true }, score: { n: 7.5 } });
  assert.strictEqual(s1.changed_by, "dana@acme.example", "the writer sealed inside, not what the hub says");
  assert.deepStrictEqual(m.data({ op: "get", store: reader, collection: "sites", id: "s2" }).entry.fields, [["photo", { x: "AAEC/f7/" }]]);
  assert.deepStrictEqual(m.data({ op: "get", store: reader, collection: "notes", id: "n-é" }).entry.fields, [["body", { t: "Façade crack ✓ 2 m" }]]);

  // Sabotage: one bit of one ciphertext, and the row is refused, not applied.
  const fresh = "records-tampered";
  assert.ok(m.data({ op: "open", store: fresh, device: "1111111111111111" }).ok);
  const bytes = unb64(rows[0].ct);
  bytes[bytes.length - 1] ^= 0x01;
  const bad = { ...rows[0], ct: b64(bytes) };
  // And a ciphertext moved under another field's name.
  const moved = { ...rows[1], k: rows[0].k };
  m.data({ ...keys, op: "sync_begin", store: fresh, now }, 64);
  const refused = m.data({ ...keys, op: "sync_end", store: fresh, answer: { now, rows: [bad, moved], cursor: "2" } });
  assert.strictEqual(refused.rejected, 2);
  assert.strictEqual(m.data({ op: "get", store: fresh, collection: "sites", id: "s1" }).entry, null);
  assert.strictEqual(m.data({ op: "status", store: fresh }).rejected, 2);
});

/* ---- a tab and a computer, through the real hub ------------------------- */

let current = null;
const before = globalThis.fetch;
globalThis.fetch = (url, init) => (String(url).startsWith(`${HUB}/`) ? worker.fetch(new Request(url, init), current) : before(url, init));

/// acme with the app released, and alice's computer holding the
/// organisation's first key (a real keyring, sealed to its device).
async function world() {
  const env = orgEnv({ ORG_LEASE_KEY: "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60" });
  current = env;
  await acme(env);
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`)).status, 200);
  const m = await module();
  const appId = m.judge(fixture("candidate-v1.krate")).judgement.manifest.app.id;
  const computer = await deviceOf("Alice's MacBook");
  const id = m.keys({ op: "device_id", public_key: computer.public_key }).device_id;
  assert.strictEqual((await call(env, "alice", "PUT", `/orgs/acme/keys/devices/${id}`, { json: { public_key: computer.public_key, name: "Alice's MacBook" } })).status, 200);
  const made = await keyringFor(m, computer);
  const init = await call(env, "alice", "POST", "/orgs/acme/keys/init", { json: { device_id: id, public_key: made.public_key, envelope: made.envelope, recovery: made.recovery } });
  assert.strictEqual(init.status, 201, JSON.stringify(init.body));
  return { env, m, digest: up.body.digest, appId, computer: { device: computer, id, envelope: made.envelope } };
}

/// carol's tab: the Player's team data, its copy in a store of its own.
async function tabOf(w, who = "carol", extra = {}) {
  const store = S.memoryStore();
  const device = await S.openDevice({ store, name: "Chrome on Mac" });
  const t = new TeamData({ mod: w.m, signing: S, hub: HUB, token: tokenFor(w.env, who), slug: "acme", appId: w.appId, digest: w.digest, me: who, store, device, ...extra });
  await t.open();
  return { t, store, device };
}

/// Alice's computer, doing what crates/runtime org_store does on every sync:
/// its holder's duty (pass the key to waiting devices), then seal what it
/// wrote, push, pull, open and merge -- the same crate, through the module.
function computerOf(w) {
  const c = w.computer;
  const name = `computer-${Math.random()}`;
  w.m.data({ op: "open", store: name, device: "aaaaaaaaaaaaaaaa" });
  return {
    put: (id, fields) => w.m.data({ op: "put", store: name, collection: "inspections", id, now: Date.now(), me: "alice", fields }),
    get: (id) => w.m.data({ op: "get", store: name, collection: "inspections", id }).entry,
    async duties() {
      return S.refresh({ hub: HUB, token: tokenFor(w.env, "alice"), slug: "acme", device: c.device, mod: w.m, held: c.envelope });
    },
    async sync() {
      const keys = { store: name, org: "acme", app_id: w.appId, envelope: c.envelope, device_secret: await c.device.secret() };
      const begin = w.m.data({ ...keys, op: "sync_begin", now: Date.now() }, 24 * 10);
      assert.ok(begin.ok, JSON.stringify(begin));
      const r = await call(w.env, "alice", "POST", `/orgs/acme/data/${w.digest}/sync`, { json: begin.body });
      assert.strictEqual(r.status, 200, JSON.stringify(r.body));
      const end = w.m.data({ ...keys, op: "sync_end", answer: r.body });
      assert.ok(end.ok, JSON.stringify(end));
      return { body: begin.body, answer: r.body, end };
    },
  };
}

test("a tab waits for the key, a colleague's computer passes it in the background, and a record goes both ways sealed", async () => {
  const w = await world();
  const desk = computerOf(w);
  const { t } = await tabOf(w);

  // New to the organisation: registered as a browser device, and waiting.
  assert.deepStrictEqual(await t.put("inspections", "i1", [["site", { t: "North depot" }], ["passed", { b: false }]]), { ok: null });
  const waiting = await t.sync();
  assert.deepStrictEqual([waiting.ok.online, waiting.ok.pending], [true, 2], JSON.stringify(waiting));
  assert.strictEqual(t.state().link, "waiting-for-key");
  assert.deepStrictEqual(t.standing().ok.link, "waiting-for-key");
  const devices = (await call(w.env, "alice", "GET", "/orgs/acme/keys?device=" + w.computer.id)).body;
  const mine = devices.devices.find((d) => d.principal === "github:carol");
  assert.deepStrictEqual([mine.kind, mine.name, mine.holds], ["browser", "Chrome on Mac", null]);
  assert.deepStrictEqual(devices.requests.map((r) => r.principal), ["github:carol"], "the computer sees who is waiting");

  // Alice's computer, in the background, passes the key on.
  assert.strictEqual((await desk.duties()).status, "holds");
  t.lastSync = 0;
  const synced = await t.sync();
  assert.deepStrictEqual([synced.ok.online, synced.ok.pending], [true, 0], JSON.stringify(synced));
  assert.strictEqual(t.state().link, "synced");
  assert.deepStrictEqual(t.who().ok, { name: "carol", role: "developer", org: "Acme" });

  // What the hub holds: ciphertext only.
  const held = JSON.stringify(w.env.ORGS.object("acme").sql.exec("SELECT * FROM org_data").toArray());
  for (const plain of ["North depot", "inspections", "i1", "site"]) assert.ok(!held.includes(plain), `${plain} reached the hub`);

  // The computer reads what the tab wrote, by carol.
  await desk.sync();
  const seen = desk.get("i1");
  assert.deepStrictEqual(Object.fromEntries(seen.fields), { passed: { b: false }, site: { t: "North depot" } });
  assert.strictEqual(seen.changed_by, "carol");

  // And the other way: the computer marks it passed; the tab reads it.
  assert.ok(desk.put("i1", [["passed", { b: true }]]).written);
  await desk.sync();
  t.lastSync = 0;
  const pulled = await t.sync();
  assert.strictEqual(pulled.ok.changed, true);
  const got = t.get("inspections", "i1").ok;
  assert.deepStrictEqual(Object.fromEntries(got.fields), { passed: { b: true }, site: { t: "North depot" } });
  assert.strictEqual(got.changed_by, "alice");

  // Offline edits to different fields of one record both stay.
  assert.ok(desk.put("i1", [["note", { t: "Roof checked" }]]).written);
  assert.deepStrictEqual(await t.put("inspections", "i1", [["site", { t: "North depot, bay 2" }]]), { ok: null });
  await desk.sync();
  t.lastSync = 0;
  await t.sync();
  await desk.sync();
  const both = Object.fromEntries(t.get("inspections", "i1").ok.fields);
  assert.deepStrictEqual(both, Object.fromEntries(desk.get("i1").fields));
  assert.deepStrictEqual([both.note, both.site], [{ t: "Roof checked" }, { t: "North depot, bay 2" }]);

  // The copy survives the tab closing: a new tab on the same store reads it
  // before any sync.
  const again = new TeamData({ mod: w.m, signing: S, hub: HUB, token: tokenFor(w.env, "carol"), slug: "acme", appId: w.appId, digest: w.digest, me: "carol", store: t.store, device: t.device });
  again.name += "-reopened";
  await again.open();
  assert.deepStrictEqual(Object.fromEntries(again.get("inspections", "i1").ok.fields), both);
});

test("a windowed app's sync answers at once; a command-line app's waits for the exchange", async () => {
  const w = await world();
  await computerOf(w).duties();
  const { t } = await tabOf(w, "carol", { windowed: true });
  // The background delivery happens on the computer's next run.
  await t.run();
  await computerOf(w).duties();
  await t.put("inspections", "i9", [["site", { t: "Depot" }]]);
  const quick = await t.sync();
  assert.strictEqual(quick.ok.pending, 1, "answered before the exchange");
  await t.chain;
  assert.strictEqual(t.status().pending, 0, "and the exchange ran behind it");
  const later = await t.sync();
  assert.strictEqual(later.ok.changed, false);
});

test("outside the app's audience the tab is refused in the hub's words, and someone removed keeps none of its data", async () => {
  const w = await world();
  const { t, store } = await tabOf(w, "dan");
  await t.put("inspections", "i1", [["site", { t: "Depot" }]]);
  await t.sync();
  await computerOf(w).duties();
  t.lastSync = 0;
  assert.strictEqual((await t.sync()).ok.pending, 0, "dan's tab holds the key and synced");
  await call(w.env, "alice", "POST", "/orgs/acme/groups", { json: { name: "finance" } });
  const audience = await call(w.env, "alice", "PUT", `/orgs/acme/releases/${w.digest}/audience`, { json: { groups: ["finance"] } });
  assert.strictEqual(audience.status, 200, JSON.stringify(audience.body));
  t.lastSync = 0;
  const out = await t.sync();
  assert.strictEqual(out.err.tag, "refused", JSON.stringify(out));
  assert.strictEqual(out.err.val.reason, "not-in-audience");
  assert.ok(out.err.val.message.length > 10, out.err.val.message);
  assert.strictEqual(t.get("inspections", "i1").err.tag, "refused", "nothing is served for the rest of the run");
  assert.strictEqual(t.state().link, "refused");
  assert.ok(await store.get(t.key), "an audience refusal keeps the copy");

  // Removed from the organisation: the copy leaves this browser.
  const w2 = await world();
  const tab = await tabOf(w2, "dan");
  await tab.t.put("inspections", "i1", [["site", { t: "Depot" }]]);
  assert.ok(await tab.store.get(tab.t.key));
  assert.strictEqual((await call(w2.env, "alice", "DELETE", `/orgs/acme/members/${encodeURIComponent("github:dan")}`)).status, 204);
  tab.t.lastSync = 0;
  const gone = await tab.t.sync();
  assert.strictEqual(gone.err.val.reason, "removed", JSON.stringify(gone));
  assert.strictEqual(await tab.store.get(tab.t.key), undefined);
});

/* ---- the live socket (SMART-FIXES 7) ------------------------------------ */

/* The hub's hibernation API, stood in (Node has no Workers runtime; see
 * org-data-live.test.mjs), and a browser WebSocket whose upgrade really goes
 * through the worker's route and the Org object's ticket check. The server
 * end's "changed" reaches the tab's onmessage on the next turn, as a
 * network would deliver it; "ping" is answered "pong" as the platform's
 * auto-response does. */
function liveHub(env) {
  const org = env.ORGS.object("acme");
  const sockets = [];
  Object.assign(org.ctx, {
    acceptWebSocket(ws, tags = []) {
      ws.tags = tags;
      sockets.push(ws);
    },
    getWebSockets(tag) {
      return sockets.filter((x) => !x.closed && (tag === undefined || x.tags.includes(tag)));
    },
  });
  let handed = null;
  org.liveResponse = (client) => {
    handed = client;
    return new Response("upgraded", { status: 200 });
  };
  class ServerEnd {
    constructor() {
      this.sent = [];
      this.closed = null;
      this.att = null;
    }
    send(m) {
      if (this.closed) throw new Error("closed");
      this.sent.push(m);
      setTimeout(() => this.client && this.client.onmessage && this.client.onmessage({ data: m }), 0);
    }
    close(code, reason) {
      if (this.closed) return;
      this.closed = { code, reason };
      setTimeout(() => this.client && this.client.fireClose(code), 0);
    }
    serializeAttachment(v) {
      this.att = structuredClone(v);
    }
    deserializeAttachment() {
      return this.att == null ? null : structuredClone(this.att);
    }
  }
  globalThis.WebSocketPair = class {
    constructor() {
      const server = new ServerEnd();
      this[0] = { server };
      this[1] = server;
    }
  };
  const opened = [];
  const socketOf = (url, protocols) => {
    const client = {
      url,
      gone: false,
      send(m) {
        if (client.gone) throw new Error("closed");
        if (m === "ping") setTimeout(() => client.onmessage && client.onmessage({ data: "pong" }), 0);
      },
      close() {
        if (client.gone) return;
        client.gone = true;
        if (client.server && !client.server.closed) client.server.closed = { code: 1000 };
        setTimeout(() => client.onclose && client.onclose({ code: 1000 }), 0);
      },
      fireClose(code) {
        if (client.gone) return;
        client.gone = true;
        client.onclose && client.onclose({ code });
      },
    };
    opened.push(client);
    (async () => {
      const res = await worker.fetch(new Request(url.replace(/^ws/, "http"), { headers: { upgrade: "websocket", "sec-websocket-protocol": protocols.join(", ") } }), env);
      if (res.status !== 200) return client.fireClose(1006);
      client.server = handed.server;
      handed.server.client = client;
      client.onopen && client.onopen();
    })();
    return client;
  };
  return { sockets, socketOf, opened };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, ok, ms = 3000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await ok()) return;
    await sleep(2);
  }
  throw new Error(`timed out waiting for ${what}`);
}

/* A tab that holds the key, ready to start. */
async function keyedTab(w, desk, who, extra) {
  // Its own copy: tabs in one test share the module, which keeps copies by
  // name, and two tabs on one name would read each other's.
  const store = S.memoryStore();
  const device = await S.openDevice({ store, name: `${who}'s browser` });
  const t = new TeamData({ mod: w.m, signing: S, hub: HUB, token: tokenFor(w.env, who), slug: "acme", appId: w.appId, digest: w.digest, me: who, store, device, ...extra });
  t.name += `-${who}-${Math.random()}`;
  await t.open();
  const tab = { t, store, device };
  await tab.t.run();
  await desk.duties();
  tab.t.lastSync = 0;
  await tab.t.run();
  assert.strictEqual(tab.t.keyState, "holds", who);
  return tab;
}

test("a write in one tab reaches another tab's copy at once over the live socket, not at the next poll", async (ctx) => {
  const w = await world();
  const desk = computerOf(w);
  await desk.duties();
  const hub = liveHub(w.env);
  // Polls a minute apart: anything that arrives sooner came by the socket.
  const slow = { socketOf: hub.socketOf, every: 60000, liveEvery: 60000 };
  const carol = await keyedTab(w, desk, "carol", slow);
  const dan = await keyedTab(w, desk, "dan", slow);
  // Stopped whatever happens, so a failure reports instead of hanging.
  ctx.after(() => (carol.t.stop(), dan.t.stop()));
  carol.t.start();
  dan.t.start();
  await until("both sockets open", () => carol.t.live && carol.t.live.open && dan.t.live && dan.t.live.open);
  assert.strictEqual(dan.t.pace(), 60000);
  await carol.t.chain;
  await dan.t.chain;

  const times = [];
  for (let i = 0; i < 20; i += 1) {
    const id = `live-${i}`;
    await carol.t.put("inspections", id, [["site", { t: `Depot ${i}` }]]);
    const t0 = performance.now();
    await carol.t.run();
    await until(`dan reads ${id}`, () => dan.t.get("inspections", id).ok);
    times.push(performance.now() - t0);
  }
  times.sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)];
  console.log(`# live: write in one tab to read in another, median ${median.toFixed(1)} ms, max ${times.at(-1).toFixed(1)} ms (20 writes, in-process hub)`);
  assert.ok(times.at(-1) < 2000, `${times.at(-1)} ms`);
  assert.strictEqual(Object.fromEntries(dan.t.get("inspections", "live-3").ok.fields).site.t, "Depot 3");
  // The tab's own writes do not bounce back to it as "changed".
  const own = hub.sockets.find((x) => x.client === carol.t.live.ws);
  assert.deepStrictEqual(own.sent, []);
  carol.t.stop();
  dan.t.stop();
  assert.strictEqual(carol.t.live, null);
});

test("a dead socket falls back to the quick poll, and reconnects after a backoff with jitter", async (ctx) => {
  const w = await world();
  const desk = computerOf(w);
  await desk.duties();
  const hub = liveHub(w.env);
  const { t } = await keyedTab(w, desk, "carol", { socketOf: hub.socketOf, every: 40, liveEvery: 60000, backoff: 30 });
  ctx.after(() => t.stop());
  t.start();
  await until("the socket opens", () => t.live && t.live.open);
  assert.strictEqual(t.pace(), 60000, "slow while it is healthy");
  const first = t.live.ws;
  // The hub goes away under it (a deploy, a dropped connection).
  hub.sockets.find((x) => x.client === first).close(1006, "gone");
  await until("the drop is noticed", () => t.live === null || t.live.ws !== first);
  assert.ok(t.liveFails >= 1);
  assert.ok(t.liveNextAt > 0);
  await until("a new socket opens", () => t.live && t.live.open && t.live.ws !== first);
  assert.strictEqual(hub.opened.length, 2, "one reconnect, not a loop");
  assert.strictEqual(hub.sockets.filter((x) => !x.closed).length, 1, "one socket, never a pile");
  assert.strictEqual(t.state().link, "synced");
  t.stop();

  // Offline for sockets entirely: syncing carries on and nothing is an error.
  const w2 = await world();
  const desk2 = computerOf(w2);
  await desk2.duties();
  const tries = [];
  const off = await keyedTab(w2, desk2, "carol", { socketOf: () => { tries.push(performance.now()); throw new Error("offline"); }, every: 20, backoff: 10, random: () => 1 });
  ctx.after(() => off.t.stop());
  off.t.start();
  await until("a few tries", () => tries.length >= 4);
  assert.strictEqual(off.t.pace(), 20, "the quick poll carries on");
  await off.t.put("inspections", "x", [["site", { t: "Depot" }]]);
  await until("the write goes up by poll", () => off.t.status().pending === 0);
  assert.notStrictEqual(off.t.state().link, "refused");
  // The waits grow (10, 20, 40 ms here): the tries are not one per sync.
  assert.ok(tries[3] - tries[2] >= 39, `${tries[3] - tries[2]} ms after the third`);
  assert.ok(tries[3] - tries[2] > tries[1] - tries[0], tries.join(" "));
  off.t.stop();
});

test("removed while the socket is open: the hub closes it, the tab syncs at once and its copy leaves", async (ctx) => {
  const w = await world();
  const desk = computerOf(w);
  await desk.duties();
  const hub = liveHub(w.env);
  const dan = await keyedTab(w, desk, "dan", { socketOf: hub.socketOf, every: 60000, liveEvery: 60000, backoff: 10 });
  await dan.t.put("inspections", "i1", [["site", { t: "Depot" }]]);
  ctx.after(() => dan.t.stop());
  dan.t.start();
  await until("the socket opens", () => dan.t.live && dan.t.live.open);
  const ws = hub.sockets.find((x) => x.client === dan.t.live.ws);
  assert.strictEqual((await call(w.env, "alice", "DELETE", `/orgs/acme/members/${encodeURIComponent("github:dan")}`)).status, 204);
  assert.strictEqual(ws.closed && ws.closed.code, 4001, "closed by the hub on the removal");
  await until("the tab hears it", () => dan.t.refused);
  assert.strictEqual(dan.t.refused.reason, "removed");
  assert.strictEqual(await dan.store.get(dan.t.key), undefined, "the copy left this browser");
  assert.strictEqual(dan.t.live, null);
});

/* ---- the worker's imports ---------------------------------------------- */

test("the worker's store.org imports move jco's values to the copy's wire shapes and back", () => {
  const sent = [];
  const answers = {
    "org.put": { ok: null },
    "org.get": { ok: { id: "i1", fields: [["photo", { x: "AAEC" }], ["score", { n: 7.5 }], ["site", { t: "Depot" }], ["ok", { b: true }]], changed_by: "carol", changed_at: 1760400000123 } },
    "org.records": { ok: { entries: [], removed: ["i2"], next: "4", more: false } },
    "org.sync": { ok: { online: true, changed: false, pending: 0, last_synced: 1760400000999 } },
    "org.status": { ok: { link: "waiting-for-key", pending: 2, reached_at: null, org: "Acme" } },
    "org.delete": { err: { tag: "refused", val: { reason: "not-in-audience", message: "Ask an owner." } } },
  };
  const ctx = (org, allowed = true) => ({
    meta: { refusals: {}, imports: [] },
    org,
    grants: { allows: (cap) => allowed && cap === "store.org" },
    bridge: { call: (op, args) => (sent.push([op, args]), { json: answers[op] }) },
    ui: {}, kv: {}, ring: {}, output() {}, notice() {},
  });
  const org = buildImports(ctx("on"))["krate:store/org"];
  org.put("inspections", "i1", [["site", { tag: "text", val: "Depot" }], ["photo", { tag: "bytes", val: new Uint8Array([0, 1, 2]) }], ["gone", { tag: "null" }], ["n", { tag: "number", val: 2 }], ["b", { tag: "boolean", val: false }]]);
  assert.deepStrictEqual(sent[0], ["org.put", { collection: "inspections", id: "i1", fields: [["site", { t: "Depot" }], ["photo", { x: "AAEC" }], ["gone", null], ["n", { n: 2 }], ["b", { b: false }]] }]);
  const e = org.get("inspections", "i1");
  assert.deepStrictEqual(e.fields.map(([n, v]) => [n, v.tag]), [["photo", "bytes"], ["score", "number"], ["site", "text"], ["ok", "boolean"]]);
  assert.deepStrictEqual([...e.fields[0][1].val], [0, 1, 2]);
  assert.deepStrictEqual([e.changedBy, e.changedAt], ["carol", 1760400000123n]);
  assert.deepStrictEqual(org.records("inspections", undefined), { entries: [], removed: ["i2"], next: "4", more: false });
  assert.deepStrictEqual(sent.at(-1), ["org.records", { collection: "inspections", since: null }]);
  assert.deepStrictEqual(org.sync(), { online: true, changed: false, pending: 0, lastSynced: 1760400000999n });
  assert.deepStrictEqual(org.status(), { link: "waiting-for-key", pending: 2, reachedAt: undefined, org: "Acme" });
  assert.throws(() => org.delete("inspections", "i1"), (err) => err instanceof HostError && err.payload.tag === "refused" && err.payload.val.reason === "not-in-audience");
  // Not an organisation's app, a browser that cannot be a device, and an app
  // that was not given store.org: said as on a computer.
  assert.throws(() => buildImports(ctx("not-org"))["krate:store/org"].get("c", "i"), (err) => err.payload.tag === "not-an-org-app");
  assert.throws(() => buildImports(ctx("unsupported"))["krate:store/org"].get("c", "i"), (err) => err.payload.tag === "unsupported");
  assert.throws(() => buildImports(ctx("on", false))["krate:store/org"].get("c", "i"), (err) => err.payload.tag === "denied");
});
