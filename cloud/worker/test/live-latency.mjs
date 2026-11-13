/* Team data's live socket on a real Workers runtime (SMART-FIXES 7): the
 * time from one person's write to another person's device holding it,
 * through `wrangler dev` (workerd, the hibernation API, real WebSockets).
 *
 * Not part of `node --test`: it needs the hub running locally, with test
 * identities (testIdentity in src/index.js, loopback only):
 *
 *   cd cloud/worker
 *   npx wrangler dev --local --port 8787 --persist-to <scratch dir> \
 *     --var PUBLIC_BASE:http://127.0.0.1:8787 \
 *     --var KRATE_TEST_IDENTITIES:t_alice=alice,t_bob=bob,t_carol=carol,t_dan=dan
 *   node --experimental-wasm-modules test/live-latency.mjs [rounds]
 *
 * Carol's device writes a sealed field; Dan's device holds a live socket,
 * hears "changed", and syncs from its cursor. It prints, for each round:
 * write sent -> "changed" heard, and write sent -> Dan's sync answer holds
 * the row. Then it removes Dan and checks the hub closes his socket.
 */
import { createHash, randomBytes } from "node:crypto";
import { rootKey, manifest, sign, zip, COMPONENT } from "./org-bundle-tools.mjs";

const HUB = process.env.HUB || "http://127.0.0.1:8787";
const ROUNDS = Number(process.argv[2] || 30);
const slug = `live-${randomBytes(3).toString("hex")}`;
const b64 = (n) => randomBytes(n).toString("base64");
const hexOf = (b) => Buffer.from(b).toString("hex");
const opaque = (...parts) => createHash("sha256").update(parts.join("\u0000")).digest("hex");
const stamp = (wall, device) => `${String(wall).padStart(16, "0")}-00000000-${device}`;

async function call(who, method, path, { json, bytes } = {}) {
  const headers = { authorization: `Bearer t_${who}` };
  let body;
  if (json !== undefined) {
    headers["content-type"] = "application/json";
    body = JSON.stringify(json);
  } else if (bytes) body = bytes;
  const res = await fetch(`${HUB}${path}`, { method, headers, body });
  const text = await res.text();
  let parsed = text;
  try {
    parsed = JSON.parse(text);
  } catch (_) {}
  if (res.status >= 300) throw new Error(`${method} ${path}: ${res.status} ${text}`);
  return parsed;
}

const A = "aaaaaaaaaaaaaaaa";
const D = "dddddddddddddddd";

async function setUp() {
  const key = rootKey();
  await call("alice", "POST", "/orgs", { json: { slug, name: "Live test" } });
  await call("alice", "PUT", `/orgs/${slug}/root-key`, { json: { public_key: key.public } });
  for (const [who, role] of [["bob", "reviewer"], ["carol", "developer"], ["dan", "recipient"]]) {
    const inv = await call("alice", "POST", `/orgs/${slug}/invites`, { json: { github: who, role } });
    await call(who, "POST", `/orgs/${slug}/invites/${inv.code}/accept`);
  }
  const up = await call("carol", "POST", `/orgs/${slug}/candidates`, { bytes: sign(zip({ "manifest.toml": manifest("1.0.0"), "code.wasm": COMPONENT }), key, slug) });
  await call("bob", "POST", `/orgs/${slug}/candidates/${up.digest}/approve`);
  // The organisation's first key, held by alice's device (fake envelope:
  // the hub never opens one).
  const pub = hexOf(randomBytes(32));
  const id = createHash("sha256").update(pub).digest("hex").slice(0, 16);
  await call("alice", "PUT", `/orgs/${slug}/keys/devices/${id}`, { json: { public_key: pub, name: "laptop" } });
  await call("alice", "POST", `/orgs/${slug}/keys/init`, {
    json: {
      device_id: id,
      public_key: hexOf(randomBytes(32)),
      envelope: b64(300),
      recovery: { public_key: hexOf(randomBytes(32)), params: { alg: "argon2id", salt: hexOf(randomBytes(16)), m_kib: 19456, t: 2, p: 1 }, blob: b64(200) },
    },
  });
  return up.digest;
}

const sync = (who, digest, body) => call(who, "POST", `/orgs/${slug}/data/${digest}/sync`, { json: { now: Date.now(), since: null, writes: [], ...body } });

function openLive(digest, ticket) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${HUB.replace(/^http/, "ws")}/orgs/${slug}/data/${digest}/live`, ["krate-live", ticket]);
    ws.onopen = () => resolve(ws);
    ws.onerror = (e) => reject(new Error(`socket did not open: ${e.message || e.type}`));
  });
}

const pct = (xs, p) => xs[Math.min(xs.length - 1, Math.floor(p * xs.length))];

async function main() {
  const digest = await setUp();
  const first = await sync("dan", digest, { device: D, live: true });
  let cursor = first.cursor;
  const ws = await openLive(digest, first.live.ticket);
  console.log(`socket open, protocol ${JSON.stringify(ws.protocol)}`);
  let heard = null;
  const messages = [];
  ws.onmessage = (e) => {
    messages.push(String(e.data));
    if (heard) heard(performance.now());
  };
  // The heartbeat the clients send, answered without waking the object.
  const pong = new Promise((r) => {
    const before = ws.onmessage;
    ws.onmessage = (e) => {
      if (e.data === "pong") {
        ws.onmessage = before;
        r(true);
      } else before(e);
    };
  });
  ws.send("ping");
  console.log(`ping answered: ${await Promise.race([pong, new Promise((r) => setTimeout(() => r(false), 2000))])}`);

  const toChanged = [];
  const toRow = [];
  for (let i = 0; i < ROUNDS; i += 1) {
    const row = { r: opaque("r", "tasks", `t${i}`), k: opaque("f", "tasks", `t${i}`, "title"), kind: "v", s: stamp(Date.now(), A), ver: 1, ct: b64(60) };
    const got = new Promise((r) => (heard = r));
    const t0 = performance.now();
    const w = sync("carol", digest, { device: A, writes: [row] });
    const t1 = await got;
    heard = null;
    // Dan's device syncs from its cursor the moment it hears.
    const pulled = await sync("dan", digest, { device: D, since: cursor });
    const t2 = performance.now();
    await w;
    if (!pulled.rows.some((x) => x.r === row.r && x.ct === row.ct)) throw new Error("Dan's sync did not bring the row");
    cursor = pulled.cursor;
    toChanged.push(t1 - t0);
    toRow.push(t2 - t0);
  }
  toChanged.sort((a, b) => a - b);
  toRow.sort((a, b) => a - b);
  const f = (xs) => `median ${pct(xs, 0.5).toFixed(1)} ms, p90 ${pct(xs, 0.9).toFixed(1)} ms, max ${xs.at(-1).toFixed(1)} ms`;
  console.log(`${ROUNDS} writes`);
  console.log(`  write sent -> "changed" heard by the other device: ${f(toChanged)}`);
  console.log(`  write sent -> the other device's sync holds the row: ${f(toRow)}`);
  console.log(`  every message: ${JSON.stringify([...new Set(messages.map((m) => Object.keys(JSON.parse(m)).sort().join(",")))])} keys, longest ${Math.max(...messages.map((m) => m.length))} bytes`);

  // Removed: the hub closes the socket.
  const closed = new Promise((r) => (ws.onclose = (e) => r(e.code)));
  await call("alice", "DELETE", `/orgs/${slug}/members/${encodeURIComponent("github:dan")}`);
  const code = await Promise.race([closed, new Promise((r) => setTimeout(() => r("still open after 3 s"), 3000))]);
  console.log(`dan removed: his socket closed with ${code}`);
  if (code !== 4001) process.exitCode = 1;
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
