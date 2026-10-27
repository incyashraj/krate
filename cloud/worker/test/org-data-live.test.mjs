/* Team data that feels live (SMART-FIXES 7, org-data-live.js): a sync
 * hands out a one-use ticket, the ticket opens a socket on the
 * organisation's Durable Object, and a write by one person sends every
 * other socket of that app a "changed" carrying the cursor only.
 *
 * Node has no Workers runtime, so the hibernation API is a stand-in here:
 * the object's `ctx` gains acceptWebSocket / getWebSockets (tags honoured,
 * closed sockets dropped, attachments cloned as structured clone does), a
 * WebSocketPair of fake sockets, and the 101 (which a Node Response cannot
 * be) is swapped for a 200 that hands the test the client end. Everything
 * else is real: the worker's route, the Org object over real SQLite, the
 * sync's checks and the ticket. A real-socket run on workerd is
 * cloud/worker/test/live-latency.mjs (wrangler dev).
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-data-live.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme, worker } from "./org-tools.mjs";
import { initKey, sealedRow } from "./org-keys-tools.mjs";
import { rootKey, bundle, manifest, sign, zip, COMPONENT } from "./org-bundle-tools.mjs";
import { formatStamp } from "../src/org-data-merge.js";
import { LIVE_PER_PERSON_APP, CLOSE_STANDING, CLOSE_REPLACED } from "../src/org-data-live.js";

const A = "aaaaaaaaaaaaaaaa";
const B = "bbbbbbbbbbbbbbbb";
const C = "cccccccccccccccc";
const enc = encodeURIComponent;

/* ---- the hibernation stand-in ----------------------------------------- */

class FakeSocket {
  constructor(host) {
    this.host = host;
    this.sent = [];
    this.closed = null;
    this.att = null;
    this.tags = [];
  }
  send(m) {
    if (this.closed) throw new Error("closed");
    this.sent.push(m);
  }
  close(code, reason) {
    if (!this.closed) this.closed = { code, reason };
  }
  serializeAttachment(v) {
    this.att = structuredClone(v);
  }
  deserializeAttachment() {
    return this.att == null ? null : structuredClone(this.att);
  }
}

function hibernation(org) {
  const host = {
    sockets: [],
    acceptWebSocket(ws, tags = []) {
      ws.tags = tags;
      host.sockets.push(ws);
    },
    getWebSockets(tag) {
      return host.sockets.filter((s) => !s.closed && (tag === undefined || s.tags.includes(tag)));
    },
  };
  Object.assign(org.ctx, { acceptWebSocket: host.acceptWebSocket, getWebSockets: host.getWebSockets });
  // The client end the 101 would carry.
  org.liveResponse = (client) => {
    host.last = client;
    return new Response("upgraded", { status: 200 });
  };
  return host;
}

globalThis.WebSocketPair = class {
  constructor() {
    const server = new FakeSocket();
    const client = { server };
    this[0] = client;
    this[1] = server;
  }
};

/* ---- the world ---------------------------------------------------------- */

const otherApp = (key) => {
  const m = manifest("1.0.0").replace('id = "dev.krate.bounce"', 'id = "dev.krate.other"').replace('name = "Bounce"', 'name = "Other"');
  return sign(zip({ "manifest.toml": m, "code.wasm": COMPONENT }), key);
};

async function release(env, slug, bytes) {
  const up = await call(env, "carol", "POST", `/orgs/${slug}/candidates`, { bytes });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  const ok = await call(env, "bob", "POST", `/orgs/${slug}/candidates/${up.body.digest}/approve`);
  assert.strictEqual(ok.status, 200, JSON.stringify(ok.body));
  return up.body.digest;
}

async function world() {
  const env = orgEnv();
  const key = rootKey();
  await acme(env, { root: key.public, revocations: null });
  const digest = await release(env, "acme", bundle(key));
  const other = await release(env, "acme", otherApp(key));
  await initKey(env);
  const org = env.ORGS.object("acme");
  const host = hibernation(org);
  return { env, key, digest, other, org, host };
}

const sync = (env, who, digest, body = {}, slug = "acme") =>
  call(env, who, "POST", `/orgs/${slug}/data/${digest}/sync`, { json: { device: A, now: Date.now(), since: null, writes: [], ...body } });

/* Open a live socket as `who` on `device`: a sync asking for a ticket,
 * then the upgrade with it. Answers { status, socket, ticket }. */
async function open(env, host, who, digest, device, slug = "acme") {
  const s = await sync(env, who, digest, { device, live: true }, slug);
  if (s.status !== 200) return { status: s.status, error: s.body && s.body.error };
  assert.ok(s.body.live && /^[0-9a-f]{64}$/.test(s.body.live.ticket), JSON.stringify(s.body.live));
  const r = await upgrade(env, slug, digest, s.body.live.ticket);
  return { status: r.status, socket: r.status === 200 ? host.last.server : null, ticket: s.body.live.ticket };
}

const upgrade = (env, slug, digest, ticket) =>
  worker.fetch(
    new Request(`https://hub.example/orgs/${slug}/data/${digest}/live`, {
      headers: { upgrade: "websocket", "sec-websocket-protocol": `krate-live, ${ticket}` },
    }),
    env,
  );

const write = (env, who, digest, device, id) =>
  sync(env, who, digest, { device, writes: [sealedRow("tasks", id, "title", formatStamp({ wall: Date.now(), counter: 0, device }))] });

/* ---- the tests ---------------------------------------------------------- */

test("a write by one person sends 'changed' to another's socket for that app, and only that app's", async () => {
  const { env, digest, other, host } = await world();
  const bob = await open(env, host, "bob", digest, B);
  const danOther = await open(env, host, "dan", other, C);
  assert.strictEqual(bob.status, 200);
  assert.strictEqual(danOther.status, 200);
  const carolOwn = await open(env, host, "carol", digest, A);

  const w = await write(env, "carol", digest, A, "t1");
  assert.strictEqual(w.status, 200, JSON.stringify(w.body));
  assert.strictEqual(w.body.accepted, 1);
  assert.deepStrictEqual(bob.socket.sent.map((m) => JSON.parse(m)), [{ t: "changed", s: w.body.cursor }]);
  assert.deepStrictEqual(danOther.socket.sent, [], "another app's socket hears nothing");
  assert.deepStrictEqual(carolOwn.socket.sent, [], "the device that wrote it already has it");

  // A pull that changes nothing tells no one.
  await sync(env, "carol", digest, { device: A });
  assert.strictEqual(bob.socket.sent.length, 1);
});

test("the message carries the cursor only: no record, field, ciphertext, person or app", async () => {
  const { env, digest, host } = await world();
  const bob = await open(env, host, "bob", digest, B);
  const row = sealedRow("tasks", "secret-id", "salary", formatStamp({ wall: Date.now(), counter: 0, device: A }));
  const w = await sync(env, "carol", digest, { device: A, writes: [row] });
  assert.strictEqual(w.body.accepted, 1);
  assert.strictEqual(bob.socket.sent.length, 1);
  const m = bob.socket.sent[0];
  assert.deepStrictEqual(Object.keys(JSON.parse(m)).sort(), ["s", "t"]);
  assert.ok(m.length < 40, m);
  for (const word of [row.r, row.k, row.ct, "carol", "dev.krate.bounce", "secret-id", "salary", digest]) {
    assert.ok(!m.includes(word), `${word} is not in ${m}`);
  }
});

test("another organisation's socket for the same app hears nothing", async () => {
  const { env, key, digest, host } = await world();
  // globex: alice owns it, carol sends and bob approves, the same app id.
  await call(env, "alice", "POST", "/orgs", { json: { slug: "globex", name: "Globex" } });
  await call(env, "alice", "PUT", "/orgs/globex/root-key", { json: { public_key: key.public } });
  for (const [login, role] of [["bob", "reviewer"], ["carol", "developer"]]) {
    const inv = await call(env, "alice", "POST", "/orgs/globex/invites", { json: { github: login, role } });
    await call(env, login, "POST", `/orgs/globex/invites/${inv.body.code}/accept`);
  }
  const g = await release(env, "globex", sign(zip({ "manifest.toml": manifest("1.0.0"), "code.wasm": COMPONENT }), key, "globex"));
  const ghost = hibernation(env.ORGS.object("globex"));
  const bobGlobex = await open(env, ghost, "bob", g, B, "globex");
  assert.strictEqual(bobGlobex.status, 200);
  const bobAcme = await open(env, host, "bob", digest, B);
  await write(env, "carol", digest, A, "t1");
  assert.strictEqual(bobAcme.socket.sent.length, 1);
  assert.deepStrictEqual(bobGlobex.socket.sent, []);
});

test("subscribing takes the sync's own standing: no ticket without it, a ticket used once, and nothing made up", async () => {
  const { env, digest, host } = await world();
  // No session, a stranger: the sync is refused, so there is no ticket.
  assert.deepStrictEqual(await open(env, host, null, digest, B), { status: 401, error: "not-signed-in" });
  assert.deepStrictEqual(await open(env, host, "stranger", digest, B), { status: 403, error: "not-a-member" });
  // Outside the audience: the same refusal the sync gives.
  await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name: "finance" } });
  await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { groups: ["finance"] } });
  assert.deepStrictEqual(await open(env, host, "dan", digest, B), { status: 403, error: "not-in-audience" });
  await call(env, "alice", "PUT", `/orgs/acme/releases/${digest}/audience`, { json: { everyone: true } });

  // A made-up ticket, a used one, one for another release.
  assert.strictEqual((await upgrade(env, "acme", digest, "ab".repeat(32))).status, 401);
  const ok = await open(env, host, "dan", digest, B);
  assert.strictEqual(ok.status, 200);
  assert.strictEqual((await upgrade(env, "acme", digest, ok.ticket)).status, 401, "one use");
  const s = await sync(env, "dan", digest, { device: B, live: true });
  assert.strictEqual((await upgrade(env, "acme", "0".repeat(64), s.body.live.ticket)).status, 401, "bound to its release");
  // No ticket at all, or no protocol: the worker says so before the store.
  const bare = await worker.fetch(new Request(`https://hub.example/orgs/acme/data/${digest}/live`, { headers: { upgrade: "websocket" } }), env);
  assert.strictEqual(bare.status, 400);

  // A ticket whose person was removed after the sync gave it: refused.
  const t = await sync(env, "dan", digest, { device: B, live: true });
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`)).status, 204);
  assert.strictEqual((await upgrade(env, "acme", digest, t.body.live.ticket)).status, 403);

  // The ticket is kept only as its hash, and none of this was audited as
  // anyone's attempt beyond the syncs themselves.
  const stored = env.ORGS.object("acme").rows("SELECT * FROM org_data_live_tickets");
  for (const r of stored) assert.ok(r.hash !== t.body.live.ticket);
  const audit = (await call(env, "alice", "GET", "/orgs/acme/audit")).body;
  assert.strictEqual(audit.chain_ok, true);
});

test("removing a person closes their sockets at once; a withdrawn release closes everyone's", async () => {
  const { env, digest, host } = await world();
  const dan1 = await open(env, host, "dan", digest, B);
  const dan2 = await open(env, host, "dan", digest, C);
  const bob = await open(env, host, "bob", digest, B);
  const before = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events.length;
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/members/${enc("github:dan")}`)).status, 204);
  assert.deepStrictEqual([dan1.socket.closed && dan1.socket.closed.code, dan2.socket.closed && dan2.socket.closed.code], [CLOSE_STANDING, CLOSE_STANDING]);
  assert.strictEqual(bob.socket.closed, null, "everyone else stays");
  // The quiet re-check adds nothing to the audit beyond the removal's own lines.
  const after = (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events;
  assert.ok(!after.slice(before).some((e) => e.action === "data.sync"), JSON.stringify(after.slice(before)));

  await write(env, "carol", digest, A, "t1");
  assert.deepStrictEqual(dan1.socket.sent, [], "a closed socket hears nothing");
  assert.strictEqual(bob.socket.sent.length, 1);

  await call(env, "bob", "POST", `/orgs/acme/releases/${digest}/withdraw`, { json: { reason: "bad build" } });
  assert.strictEqual(bob.socket.closed && bob.socket.closed.code, CLOSE_STANDING);
});

test("a socket that slipped past the re-check still hears nothing once its person is gone", async () => {
  const { env, digest, org, host } = await world();
  const dan = await open(env, host, "dan", digest, B);
  // The member row changes with no command (as if a re-check were missed).
  org.sql.exec("UPDATE members SET state = 'removed' WHERE principal = 'github:dan'");
  await write(env, "carol", digest, A, "t1");
  assert.deepStrictEqual(dan.socket.sent, []);
  assert.strictEqual(dan.socket.closed && dan.socket.closed.code, CLOSE_STANDING);
});

test("a person's sockets for one app stay bounded: a tab reconnecting in a loop replaces, never piles up", async () => {
  const { env, digest, host } = await world();
  const opened = [];
  for (let i = 0; i < LIVE_PER_PERSON_APP + 5; i += 1) opened.push(await open(env, host, "bob", digest, B));
  const live = host.getWebSockets().filter((s) => s.att.p === "github:bob");
  assert.strictEqual(live.length, LIVE_PER_PERSON_APP);
  assert.ok(opened.slice(0, 5).every((o) => o.socket.closed && o.socket.closed.code === CLOSE_REPLACED));
  assert.ok(opened.slice(5).every((o) => !o.socket.closed), "the newest stay");
  // Tickets are bounded the same way.
  for (let i = 0; i < 20; i += 1) await sync(env, "bob", digest, { device: B, live: true });
  const tickets = env.ORGS.object("acme").row("SELECT COUNT(*) AS n FROM org_data_live_tickets WHERE principal = 'github:bob'").n;
  assert.ok(tickets <= LIVE_PER_PERSON_APP, `${tickets} tickets`);
});

test("a ticket runs out after a minute", async () => {
  const { env, digest, org } = await world();
  const s = await sync(env, "bob", digest, { device: B, live: true });
  const now = org.now();
  org.now = () => now + 61;
  assert.strictEqual((await upgrade(env, "acme", digest, s.body.live.ticket)).status, 401);
});

test("the upgrade passes the worker untouched: no CORS rebuild of a 101", async () => {
  const { env, digest, org } = await world();
  // A Response cannot be 101 in Node; stand one in to see it is not rebuilt.
  const marker = { status: 101, webSocket: {}, headers: new Headers(), body: null };
  org.liveResponse = () => marker;
  const s = await sync(env, "bob", digest, { device: B, live: true });
  const res = await upgrade(env, "acme", digest, s.body.live.ticket);
  assert.strictEqual(res, marker);
});
