/* Public ownership by principal (K-1085, the rest).
 *
 * A Krate session's `login` is the part of an email address before the @,
 * so the email account bob@evil.example publishes as "bob", exactly as the
 * GitHub user bob does. Ownership used to compare that text: the
 * look-alike could re-list bob's bytes, retire his gallery rows, move his
 * moving name /c/bob/<app> (and bob.krate.tech/<app>) to its own bytes,
 * unpublish or deface his listings, attach source to them, and appear on
 * his developer page. Now everything is owned by `github:<login>` (GitHub
 * vouched) or `account:<user-id>` (an email or Google account), developer
 * namespaces are GitHub-only, and a legacy row with no owner reads as
 * `github:<author_login>`.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/ownership-principals.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import "./github-stub.mjs";
import worker from "../src/index.js";
import { r2Mock, sha256Hex } from "./r2-mock.mjs";
import { planAttribution, applyPlan, readNamespace } from "../scripts/attribute-owners.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const V1 = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));
const V2 = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-stored.krate")));
const V3 = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-signed.krate")));
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0]);

// GitHub bob signs in with a GitHub token (gho_bob, test/github-stub.mjs).
// The look-alike is an email account whose login is also "bob".
const BOB = "Bearer gho_bob";
const LOOKALIKE = "Bearer krs_lookalike";

function env(extra = {}) {
  const kv = new Map([
    ["session:krs_lookalike", "u-look"],
    ["user:u-look", JSON.stringify({ id: "u-look", login: "bob", name: "bob", email: "bob@evil.example", providers: ["email"] })],
    ["session:krs_eve", "u-eve"],
    ["user:u-eve", JSON.stringify({ id: "u-eve", login: "eve", name: "eve", email: "eve@example.com", providers: ["email"] })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => {
        kv.set(k, v);
      },
      delete: async (k) => {
        kv.delete(k);
      },
      list: async ({ prefix = "" } = {}) => ({
        keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })),
        list_complete: true,
      }),
    },
    BUNDLES: r2Mock(),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
    ...extra,
  };
}

const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
async function publish(e, auth, bytes, name = "Notes") {
  const res = await worker.fetch(req("/publish", { method: "POST", headers: { authorization: auth, "x-krate-name": name }, body: bytes }), e);
  return { status: res.status, body: await res.json().catch(() => null) };
}
const listing = (e, hash) => JSON.parse(e._kv.get(`app:${hash}`) || "null");
const channel = (e, key) => JSON.parse(e._kv.get(`channel:${key}`) || "null");

test("a look-alike email account cannot re-list, retire, or move bob's moving name", async () => {
  const e = env();
  const mine = await publish(e, BOB, V1);
  assert.strictEqual(mine.status, 200);
  assert.strictEqual(listing(e, mine.body.id).owner, "github:bob");
  assert.strictEqual(mine.body.channel, "https://hub.example/c/bob/notes");

  // Re-list: the same bytes, sent again by the look-alike.
  const copy = await publish(e, LOOKALIKE, V1);
  assert.match(copy.body.note, /already published/);
  assert.strictEqual(listing(e, mine.body.id).owner, "github:bob", "the listing and its owner stand");

  // Retire and move: different bytes under the same name.
  const theirs = await publish(e, LOOKALIKE, V2);
  assert.strictEqual(theirs.status, 200);
  assert.strictEqual(listing(e, theirs.body.id).owner, "account:u-look");
  assert.strictEqual(listing(e, theirs.body.id).author_url, "", "no GitHub profile for an account GitHub never vouched for");
  assert.ok(listing(e, mine.body.id), "bob's gallery row is not retired");
  assert.strictEqual(theirs.body.channel, undefined, "an email account has no moving name");
  assert.strictEqual(channel(e, "bob/notes").current, mine.body.id, "/c/bob/notes still names bob's bytes");
  const resolved = await worker.fetch(req("/c/bob/notes"), e);
  assert.ok((resolved.headers.get("location") || "").includes(mine.body.id) || resolved.status === 200);

  // Bob's own next version still retires his old row and moves his name.
  const next = await publish(e, BOB, V3);
  // A version an update replaced keeps its row and link, off the shelf (IC-976).
  assert.strictEqual(listing(e, mine.body.id).superseded, true, "superseded by its owner's next version");
  assert.strictEqual(next.body.line, mine.body.line, "bob's next version is on bob's line");
  assert.notStrictEqual(theirs.body.line, mine.body.line, "the look-alike's app is a line of its own");
  assert.ok(listing(e, theirs.body.id), "the look-alike's row is not bob's to retire either");
  assert.strictEqual(channel(e, "bob/notes").current, next.body.id);
});

test("a look-alike cannot unpublish, change images, appeal, or attach source and notices", async () => {
  const e = env({ KRATE_BUILDER_SECRET: "s", BIG_PUBLISHERS: "bob" });
  const mine = await publish(e, BOB, V1);
  const hash = mine.body.id;
  for (const [method, path, body] of [
    ["DELETE", `/app/${hash}`],
    ["POST", `/shot/${hash}`, PNG],
    ["POST", `/icon/${hash}`, PNG],
  ]) {
    const res = await worker.fetch(req(path, { method, headers: { authorization: LOOKALIKE }, body }), e);
    assert.strictEqual(res.status, 403, `${method} ${path}`);
  }
  assert.ok(listing(e, hash), "still listed");
  // Source and notices: the big tier is for GitHub logins in BIG_PUBLISHERS,
  // and "bob" from an email account is not the GitHub login bob.
  for (const kind of ["source", "notices", "bundle"]) {
    const res = await worker.fetch(
      req("/publish/big/start", {
        method: "POST",
        headers: { authorization: LOOKALIKE, "content-type": "application/json" },
        body: JSON.stringify({ kind, size: 10, sha256: "a".repeat(64), for: hash }),
      }),
      e,
    );
    assert.strictEqual(res.status, 403, kind);
  }
  const real = await worker.fetch(
    req("/publish/big/start", {
      method: "POST",
      headers: { authorization: BOB, "content-type": "application/json" },
      body: JSON.stringify({ kind: "notices", size: 10, sha256: "a".repeat(64), for: hash }),
    }),
    e,
  );
  assert.strictEqual(real.status, 200, "bob himself still can");
  // And the other way: the look-alike's own app (author_login "bob" too) is
  // not GitHub bob's to attach to, remove or deface.
  const theirs = await publish(e, LOOKALIKE, V2, "Other");
  const attach = await worker.fetch(
    req("/publish/big/start", {
      method: "POST",
      headers: { authorization: BOB, "content-type": "application/json" },
      body: JSON.stringify({ kind: "source", size: 10, sha256: "b".repeat(64), for: theirs.body.id }),
    }),
    e,
  );
  assert.strictEqual(attach.status, 403, "source for an app bob does not own");
  const shot = await worker.fetch(req(`/shot/${theirs.body.id}`, { method: "POST", headers: { authorization: BOB }, body: PNG }), e);
  assert.strictEqual(shot.status, 403);
  const drop = await worker.fetch(req(`/app/${theirs.body.id}`, { method: "DELETE", headers: { authorization: BOB } }), e);
  assert.strictEqual(drop.status, 403);
  // Appeal of a takedown.
  e._kv.set(`takedown:${hash}`, JSON.stringify({ reason: "x", scope: "listing", at: 1 }));
  const appeal = await worker.fetch(
    req(`/takedown/${hash}/appeal`, { method: "POST", headers: { authorization: LOOKALIKE, "content-type": "application/json" }, body: JSON.stringify({ text: "mine" }) }),
    e,
  );
  assert.strictEqual(appeal.status, 403);
});

test("a developer page shows only what its GitHub owner owns, and an email account has none", async () => {
  const e = env({ PUBLIC_BASE: "https://hub.krate.tech" });
  const mine = await publish(e, BOB, V1);
  const theirs = await publish(e, LOOKALIKE, V2, "Other");
  const page = await worker.fetch(new Request("https://bob.krate.tech/", { headers: { accept: "text/html" } }), e);
  const html = await page.text();
  assert.strictEqual(page.status, 200);
  assert.ok(html.includes("Notes"), "bob's app is on his page");
  assert.ok(!html.includes("Other") && !html.includes(theirs.body.id.slice(0, 12)), "the look-alike's is not");
  // An email account publishing as "eve" gets no eve.krate.tech.
  const eve = await publish(e, "Bearer krs_eve", V3, "Eve's");
  assert.strictEqual(eve.status, 200);
  const evePage = await worker.fetch(new Request("https://eve.krate.tech/", { headers: { accept: "text/html" } }), e);
  assert.strictEqual(evePage.status, 404);
  // The owner principal is not part of the public gallery.
  const gallery = await (await worker.fetch(new Request("https://hub.krate.tech/apps"), e)).text();
  assert.ok(!gallery.includes("account:") && !gallery.includes("u-look"), "account ids stay private");
  // /my/apps: each sees their own.
  const look = await (await worker.fetch(new Request("https://hub.krate.tech/my/apps", { headers: { authorization: LOOKALIKE } }), e)).json();
  assert.deepStrictEqual(look.apps.map((a) => a.hash), [theirs.body.id]);
  const bob = await (await worker.fetch(new Request("https://hub.krate.tech/my/apps", { headers: { authorization: BOB } }), e)).json();
  assert.deepStrictEqual(bob.apps.map((a) => a.hash), [mine.body.id]);
});

test("a legacy row with no owner can be acted on only by its vouched GitHub owner", async () => {
  const e = env();
  // Written as the hub wrote them before owners existed.
  const hash = await sha256Hex(V1);
  await e.BUNDLES.put(hash, V1, { sha256: hash });
  e._kv.set(`app:${hash}`, JSON.stringify({ name: "Notes", author_login: "bob", author: "bob", published: 1, size: V1.length }));
  e._kv.set(`owner:${hash}`, JSON.stringify({ login: "bob", at: 1 }));
  e._kv.set("channel:bob/notes", JSON.stringify({ login: "bob", name: "Notes", slug: "notes", current: hash, moved_at: 1, history: [{ hash, at: 1 }] }));

  const shot = await worker.fetch(req(`/shot/${hash}`, { method: "POST", headers: { authorization: LOOKALIKE }, body: PNG }), e);
  assert.strictEqual(shot.status, 403);
  const gone = await worker.fetch(req(`/app/${hash}`, { method: "DELETE", headers: { authorization: LOOKALIKE } }), e);
  assert.strictEqual(gone.status, 403);
  const theirs = await publish(e, LOOKALIKE, V2);
  assert.ok(listing(e, hash), "the legacy row is not retired by a look-alike");
  assert.strictEqual(channel(e, "bob/notes").current, hash, "nor its channel moved");
  assert.strictEqual(theirs.body.channel, undefined);
  // GitHub bob: the legacy row is his.
  const ok = await worker.fetch(req(`/shot/${hash}`, { method: "POST", headers: { authorization: BOB }, body: PNG }), e);
  assert.notStrictEqual(ok.status, 403);
  const removed = await worker.fetch(req(`/app/${hash}`, { method: "DELETE", headers: { authorization: BOB } }), e);
  assert.strictEqual(removed.status, 200);
});

/* ---- the attribution script ----------------------------------------- */

/* A fixture namespace with every case the report must cover. */
function fixtureNamespace() {
  const kv = new Map();
  const put = (k, v) => kv.set(k, typeof v === "string" ? v : JSON.stringify(v));
  put("user:g1", { id: "g1", login: "incyashraj", providers: ["github", "google"] });
  put("user:g2", { id: "g2", login: "alice", providers: ["github"] });
  put("user:e1", { id: "e1", login: "carol", providers: ["email"] });
  put("user:e2", { id: "e2", login: "dan", providers: ["google"] });
  put("user:e3", { id: "e3", login: "dan", providers: ["email"] });
  put("user:e4", { id: "e4", login: "alice", providers: ["email"] });
  put("user:e5", { id: "e5", login: "frank", providers: ["email", "github"] });
  put("app:h1", { author_login: "incyashraj", name: "A" }); // github
  put("app:h2", { author_login: "carol", name: "B" }); // account
  put("app:h3", { author_login: "dan", name: "C" }); // ambiguous: two email accounts
  put("app:h4", { author_login: "alice", name: "D" }); // ambiguous: GitHub and email
  put("app:h5", { author_login: "gone", name: "E" }); // no current account
  put("app:h6", { author_login: "carol", name: "F", owner: "account:e1" }); // already
  put("app:h7", { author_login: "frank", name: "G" }); // GitHub linked later: account
  put("owner:h2", { login: "carol", at: 1 }); // account
  put("channel:incyashraj/a", { login: "incyashraj", slug: "a", current: "h1" }); // github
  put("channel:carol/b", { login: "carol", slug: "b", current: "h2" }); // review
  put("bigversions:incyashraj:a", '["h1"]');
  put("session:krs_x", "g1"); // not a record this decides
  const store = {
    kv,
    puts: [],
    async list(prefix) {
      return [...kv.keys()].filter((k) => k.startsWith(prefix));
    },
    async get(key) {
      return kv.get(key) ?? null;
    },
    async put(key, value) {
      store.puts.push(key);
      kv.set(key, value);
    },
  };
  return store;
}

test("the attribution report covers every case and writes nothing", async () => {
  const ns = fixtureNamespace();
  const plan = planAttribution(await readNamespace(ns));
  const by = Object.fromEntries(plan.rows.map((r) => [r.key, r]));
  assert.deepStrictEqual(
    Object.fromEntries(Object.entries(by).map(([k, r]) => [k, [r.case, r.owner, r.write]])),
    {
      "app:h1": ["github", "github:incyashraj", false],
      "app:h2": ["account", "account:e1", true],
      "app:h3": ["ambiguous", null, false],
      "app:h4": ["ambiguous", null, false],
      "app:h5": ["no-account", "github:gone", false],
      "app:h6": ["already", "account:e1", false],
      "app:h7": ["account", "account:e5", true],
      "owner:h2": ["account", "account:e1", true],
      "channel:incyashraj/a": ["github", "github:incyashraj", false],
      "channel:carol/b": ["review", "account:e1", false],
      "bigversions:incyashraj:a": ["github", "github:incyashraj", false],
    },
  );
  assert.strictEqual(by["app:h3"].held_by.length, 2, "an ambiguous row says who holds the login");
  assert.deepStrictEqual(plan.summary, {
    accounts: 7,
    github_accounts: 2,
    records: 11,
    github: 3,
    account: 3,
    no_account: 1,
    already: 1,
    ambiguous: 2,
    review: 1,
    no_login: 0,
    writes: 3,
  });
  assert.deepStrictEqual(ns.puts, [], "the report wrote nothing");
});

test("--apply writes only the account rows, re-checked, and the hub then obeys them", async () => {
  const ns = fixtureNamespace();
  const plan = planAttribution(await readNamespace(ns));
  // A row changed between the report and the write is left alone.
  ns.kv.set("app:h7", JSON.stringify({ author_login: "someone-else", name: "G" }));
  const done = await applyPlan(ns, plan);
  assert.deepStrictEqual(
    done.map((d) => [d.key, d.result]),
    [
      ["app:h2", "written"],
      ["app:h7", "changed-since-report"],
      ["owner:h2", "written"],
    ],
  );
  assert.deepStrictEqual(ns.puts, ["app:h2", "owner:h2"]);
  assert.strictEqual(JSON.parse(ns.kv.get("app:h2")).owner, "account:e1");
  assert.strictEqual(JSON.parse(ns.kv.get("app:h2")).name, "B", "nothing else in the row changes");
  // Applying again writes nothing.
  const again = await applyPlan(ns, planAttribution(await readNamespace(ns)));
  assert.deepStrictEqual(again, []);

  // Through the hub: an email account's legacy row, attributed, is its own.
  const e = env();
  const hash = await sha256Hex(V1);
  await e.BUNDLES.put(hash, V1, { sha256: hash });
  e._kv.set(`app:${hash}`, JSON.stringify({ name: "Eve's", author_login: "eve", published: 1, size: V1.length }));
  const before = await worker.fetch(req(`/app/${hash}`, { method: "DELETE", headers: { authorization: "Bearer krs_eve" } }), e);
  assert.strictEqual(before.status, 403, "before attribution, a legacy row is github:eve's");
  const hubNs = {
    list: async (prefix) => [...e._kv.keys()].filter((k) => k.startsWith(prefix)),
    get: async (k) => e._kv.get(k) ?? null,
    put: async (k, v) => e._kv.set(k, v),
  };
  await applyPlan(hubNs, planAttribution(await readNamespace(hubNs)));
  assert.strictEqual(JSON.parse(e._kv.get(`app:${hash}`)).owner, "account:u-eve");
  const after = await worker.fetch(req(`/app/${hash}`, { method: "DELETE", headers: { authorization: "Bearer krs_eve" } }), e);
  assert.strictEqual(after.status, 200, "after it, eve's account can remove its own app");
});
