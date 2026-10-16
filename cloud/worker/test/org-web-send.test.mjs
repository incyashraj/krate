/* Send to an organisation from Studio in a browser (G5): KrateSigning.publish
 * in docs/landing/kit/org-device.js, through the hub's real routes and
 * store, with the real validator module and node's WebCrypto.
 *
 * A developer's browser makes its own release key, asks to publish, is
 * approved in the background by the owner's browser, signs the app with
 * the slip and sends it with its name and release note; it waits for a
 * second person. An owner whose browser holds the signing key signs as the
 * organisation. A reviewer is refused before anything is made. A browser
 * whose permission was withdrawn forgets its key and says so.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-web-send.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join as joinPath } from "node:path";
import { orgEnv, call, tokenFor, worker } from "./org-tools.mjs";
import "../../../docs/landing/kit/org-device.js";

const here = dirname(fileURLToPath(import.meta.url));
const S = globalThis.KrateSigning;
const MODULE = readFileSync(joinPath(here, "..", "src", "validator.wasm"));
const BOUNCE = new Uint8Array(readFileSync(joinPath(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));
const HUB = "https://hub.example";
const SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const enc = encodeURIComponent;

let current = null;
const before = globalThis.fetch;
// Every request the library makes, so a test can read its headers.
const sent = [];
globalThis.fetch = (url, init) => {
  if (String(url).startsWith(`${HUB}/`)) {
    sent.push({ url: String(url), method: (init && init.method) || "GET", headers: { ...((init && init.headers) || {}) } });
    return worker.fetch(new Request(url, init), current);
  }
  return before(url, init);
};

async function world(members) {
  const env = orgEnv({ ORG_LEASE_KEY: SEED });
  current = env;
  const r = await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } });
  assert.strictEqual(r.status, 201, JSON.stringify(r.body));
  const mod = await S.loadModule(MODULE);
  for (const [login, role] of members) {
    const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: login, role } });
    assert.strictEqual(inv.status, 201, JSON.stringify(inv.body));
    assert.strictEqual((await call(env, login, "POST", `/orgs/acme/invites/${inv.body.code}/accept`)).status, 200);
  }
  return { env, mod };
}

/// `who`'s browser: a device and a store of its own, in memory.
async function browserOf(name) {
  const store = S.memoryStore();
  return { store, device: await S.openDevice({ store, name }) };
}
const as = (env, who, mod, b, extra = {}) => ({ hub: HUB, token: tokenFor(env, who), slug: "acme", mod, device: b.device, store: b.store, ...extra });

test("a developer's browser asks to publish, is approved by the owner's browser, and sends the app with its note: it waits for a second person", async () => {
  const { env, mod } = await world([["bob", "reviewer"], ["carol", "developer"]]);
  const alice = await browserOf("Chrome on Mac");
  await S.setUp(as(env, "alice", mod, alice));

  // Carol's first send: her browser makes a release key and asks. Nobody
  // has approved it yet, so nothing is sent.
  const carol = await browserOf("Firefox on Windows");
  const steps = [];
  const first = await S.publish(as(env, "carol", mod, carol, { bytes: BOUNCE, name: "Stock", note: "First version", wait: 0, onStep: (s) => steps.push(s) }));
  assert.strictEqual(first.state, "waiting-for-publisher", JSON.stringify(first));
  assert.deepStrictEqual([first.publisher.state, first.publisher.can_sign, first.publisher.waiting_on], ["waiting", ["alice"], "a-holder-device"]);
  assert.deepStrictEqual(steps, ["Getting you ready to publish..."]);
  assert.deepStrictEqual((await call(env, "alice", "GET", "/orgs/acme/candidates")).body, [], "nothing was sent while she waits");
  const key = (await carol.store.get("release:acme")).public_key;
  assert.match(key, /^[0-9a-f]{64}$/);
  const asked = sent.find((r) => r.method === "POST" && r.url === `${HUB}/orgs/acme/publishers`);
  assert.ok(asked, "she asked at /publishers");
  // The browser is not registered as one of Acme's devices: it never asked
  // for the team data key.
  assert.ok(!sent.some((r) => r.method === "PUT" && r.url.includes("/keys/devices/") && r.headers.authorization === `Bearer ${tokenFor(env, "carol")}`), "a developer's browser is not made a device");

  // Alice's browser does its share, as when she opens the console.
  const did = await S.duties(as(env, "alice", mod, alice));
  assert.deepStrictEqual(did.approved, ["carol"]);

  // Carol sends again: signed with her release key and the slip, marked
  // as Acme's private app, with its name and note.
  sent.length = 0;
  const steps2 = [];
  const out = await S.publish(as(env, "carol", mod, carol, { bytes: BOUNCE, name: "Stock", note: "First version: imports the sheet. Prüfung ✓", onStep: (s) => steps2.push(s) }));
  assert.deepStrictEqual([out.state, out.solo, out.signer], ["waiting-for-approval", false, "release-key"], JSON.stringify(out));
  assert.deepStrictEqual(out.approvers, ["alice", "bob"]);
  assert.deepStrictEqual(steps2, ["Getting you ready to publish...", "Signing Stock...", "Sending it..."]);
  const up = sent.find((r) => r.method === "POST" && r.url === `${HUB}/orgs/acme/candidates`);
  assert.deepStrictEqual(
    [up.headers["x-krate-encoding"], up.headers["x-krate-name"], up.headers["x-krate-note"]],
    ["uri", "Stock", enc("First version: imports the sheet. Prüfung ✓")],
    "the upload carries the name and the release note",
  );
  assert.ok(up.headers["idempotency-key"], "and an idempotency key");
  const list = (await call(env, "bob", "GET", "/orgs/acme/candidates")).body;
  assert.deepStrictEqual(
    list.map((c) => [c.digest, c.state, c.uploaded_by, c.note, c.name]),
    // The name a version is listed by is its manifest's.
    [[out.digest, "waiting-for-approval", "github:carol", "First version: imports the sheet. Prüfung ✓", "Bounce"]],
  );
  // She cannot approve her own; Bob can.
  assert.strictEqual((await call(env, "carol", "POST", `/orgs/acme/candidates/${out.digest}/approve`, { json: {} })).status, 403);
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${out.digest}/approve`, { json: { accept: [] } })).status, 200);
  const got = await call(env, "bob", "GET", `/a/${out.digest}`);
  assert.strictEqual(got.status, 200);
  const judged = mod.judge(got.bytes).judgement;
  const root = (await call(env, "alice", "GET", "/orgs/acme")).body.org.root_key;
  assert.deepStrictEqual([judged.signature.state, judged.signature.chain && judged.signature.chain.root, judged.org_mark], ["valid", root, "acme"], "signed under Acme's root and marked as its private app");
  assert.strictEqual(judged.signature.public_key, key, "with carol's own release key");
});

test("an owner whose browser holds the signing key sends as the organisation; alone, it is released", async () => {
  const { env, mod } = await world([]);
  const alice = await browserOf("Chrome on Mac");
  await S.setUp(as(env, "alice", mod, alice));
  const out = await S.publish(as(env, "alice", mod, alice, { bytes: BOUNCE, name: "Bounce", holder: true }));
  assert.deepStrictEqual([out.state, out.solo, out.signer], ["released", true, "organisation"], JSON.stringify(out));
  assert.strictEqual(await alice.store.get("release:acme"), undefined, "no release key was made for a holder");
  assert.strictEqual(out.url, `${HUB}/a/${out.digest}`);
});

test("someone who may not publish is refused before a key is approved, and a withdrawn browser forgets its key", async () => {
  const { env, mod } = await world([["bob", "reviewer"], ["carol", "developer"]]);
  const alice = await browserOf("Chrome on Mac");
  await S.setUp(as(env, "alice", mod, alice));
  const bob = await browserOf("Safari on Mac");
  await assert.rejects(
    S.publish(as(env, "bob", mod, bob, { bytes: BOUNCE, name: "Bounce", wait: 0 })),
    (e) => e.reason === "role-cannot" && /cannot publish to acme/.test(e.message),
  );
  assert.deepStrictEqual((await call(env, "alice", "GET", "/orgs/acme/candidates")).body, []);

  const carol = await browserOf("Edge on Windows");
  await S.publish(as(env, "carol", mod, carol, { bytes: BOUNCE, wait: 0 }));
  await S.duties(as(env, "alice", mod, alice));
  const signedFirst = await S.publish(as(env, "carol", mod, carol, { bytes: BOUNCE, name: "Bounce" }));
  assert.strictEqual(signedFirst.state, "waiting-for-approval");
  // Carol stops publishing for a while, then comes back: her old key is
  // withdrawn, and her browser makes a new one on the next send.
  const old = (await carol.store.get("release:acme")).public_key;
  assert.strictEqual((await call(env, "alice", "PUT", `/orgs/acme/members/${enc("github:carol")}`, { json: { role: "reviewer" } })).status, 200);
  assert.strictEqual((await call(env, "alice", "PUT", `/orgs/acme/members/${enc("github:carol")}`, { json: { role: "developer" } })).status, 200);
  await assert.rejects(S.publish(as(env, "carol", mod, carol, { bytes: BOUNCE, wait: 0 })), (e) => e.reason === "publisher-withdrawn" && /nothing was sent/.test(e.message));
  assert.strictEqual(await carol.store.get("release:acme"), undefined, "the withdrawn key is forgotten");
  const again = await S.publish(as(env, "carol", mod, carol, { bytes: BOUNCE, wait: 0 }));
  assert.strictEqual(again.state, "waiting-for-publisher");
  assert.notStrictEqual((await carol.store.get("release:acme")).public_key, old, "a new key asks afresh");
});
