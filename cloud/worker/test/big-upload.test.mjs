/* The big-file path (IC-945): an app over the publish door's 5 MiB, uploaded
 * in parts through the Worker into R2, judged on the build service, and
 * listed only on a passing verdict.
 *
 * Drives the real worker with an R2 mock that holds multipart uploads to
 * R2's own rules, and a KV that counts what it is asked to do -- the free
 * tier allows 1,000 writes a day, so a part must cost none.
 *
 *   node cloud/worker/test/big-upload.test.mjs
 */
import assert from "node:assert";
import "./github-stub.mjs";
import worker from "../src/index.js";
import { r2Mock, sha256Hex } from "./r2-mock.mjs";

const MiB = 1024 * 1024;
const SECRET = "test-builder-secret";

function env({ allow = "alice", secret = SECRET } = {}) {
  const kv = new Map([
    ["session:krs_alice", "u1"],
    ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
    ["session:krs_bob", "u2"],
    ["user:u2", JSON.stringify({ login: "bob", name: "Bob", avatar_url: "" })],
  ]);
  const ops = { get: 0, put: 0, delete: 0, list: 0 };
  return {
    APPS: {
      get: async (k) => {
        ops.get++;
        return kv.get(k) ?? null;
      },
      put: async (k, v) => {
        ops.put++;
        kv.set(k, v);
      },
      delete: async (k) => {
        ops.delete++;
        kv.delete(k);
      },
      list: async ({ prefix = "" } = {}) => {
        ops.list++;
        return {
          keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })),
          list_complete: true,
        };
      },
    },
    BUNDLES: r2Mock(),
    PUBLIC_BASE: "https://hub.example",
    KRATE_BUILDER_SECRET: secret,
    BIG_PUBLISHERS: allow,
    BIG_PART_BYTES: String(5 * MiB),
    _kv: kv,
    _ops: ops,
  };
}

const req = (method, path, { body, headers = {} } = {}) =>
  new Request(`https://hub.example${path}`, { method, body, headers });

function noise(n, seed = 1) {
  const out = new Uint8Array(n);
  let x = seed >>> 0 || 1;
  for (let i = 0; i < n; i++) {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    out[i] = x & 0xff;
  }
  return out;
}

/// What the build service answers for a passing bundle: the fields the
/// listing reads from a judgement.
function judgement(name = "Big") {
  return {
    manifest: {
      app: { id: "dev.krate.big", name, version: "1.0.0" },
      capabilities: [{ cap: "ui.window:create", rationale: "the game", required: true }],
    },
    signature: { state: "absent", public_key: null },
    archive: "a".repeat(64),
    execution: "e".repeat(64),
    project: null,
    release: null,
    derived_from: null,
    validator: "krate-bundle test",
    wit: "w".repeat(64),
  };
}

async function start(e, bytes, extra = {}, who = "gho_alice") {
  const res = await worker.fetch(
    req("POST", "/publish/big/start", {
      headers: { authorization: `Bearer ${who}`, "content-type": "application/json" },
      body: JSON.stringify({
        kind: "bundle",
        size: bytes.byteLength,
        sha256: await sha256Hex(bytes),
        name: "Big",
        ...extra,
      }),
    }),
    e,
  );
  return { res, body: res.ok ? await res.clone().json() : await res.clone().text() };
}

async function sendParts(e, up, bytes) {
  const etags = [];
  for (let n = 1; n <= up.parts; n++) {
    const part = bytes.subarray((n - 1) * up.part_size, Math.min(bytes.byteLength, n * up.part_size));
    const res = await worker.fetch(
      req("PUT", `/publish/big/${up.id}/part/${n}`, {
        headers: { "x-krate-upload": up.token, "content-length": String(part.byteLength) },
        body: part,
      }),
      e,
    );
    assert.strictEqual(res.status, 200, await res.clone().text());
    etags.push({ part: n, etag: (await res.json()).etag });
  }
  return etags;
}

const finish = (e, up, parts) =>
  worker.fetch(
    req("POST", `/publish/big/${up.id}/finish`, {
      headers: { "x-krate-upload": up.token, "content-type": "application/json" },
      body: JSON.stringify({ parts }),
    }),
    e,
  );

const verdict = (e, id, body, secret = SECRET) =>
  worker.fetch(
    req("POST", `/internal/big/${id}/verdict`, {
      headers: { "x-krate-builder": secret, "content-type": "application/json" },
      body: JSON.stringify(body),
    }),
    e,
  );

async function publishBig(e, bytes, name = "Big") {
  const { res, body: up } = await start(e, bytes, { name });
  assert.strictEqual(res.status, 200, String(up));
  const parts = await sendParts(e, up, bytes);
  assert.strictEqual((await finish(e, up, parts)).status, 202);
  const done = await verdict(e, up.id, {
    ok: true,
    sha256: await sha256Hex(bytes),
    judgement: judgement(name),
  });
  return { up, status: await done.json() };
}

/* ---- off without the builder secret, and closed to anyone not allowed -- */
{
  const bytes = noise(12 * MiB);
  assert.strictEqual((await start(env({ secret: "" }), bytes)).res.status, 503);
  const closed = await start(env({ allow: "" }), bytes);
  assert.strictEqual(closed.res.status, 403);
  assert.match(closed.body, /open to a few publishers/);
  assert.strictEqual((await start(env(), bytes, {}, "krs_nobody")).res.status, 401);
}

/* ---- the whole path, and what it costs in KV --------------------------- */
{
  const e = env();
  const bytes = noise(12 * MiB + 123, 7);
  const hash = await sha256Hex(bytes);
  // The big tier is for GitHub logins (K-1085), and the first sign-in with
  // a GitHub token caches the identity: one write a returning publisher
  // does not pay again within six hours. Warm it so the count below is
  // the publish itself.
  await worker.fetch(req("POST", "/publish/big/start", { headers: { authorization: "Bearer gho_alice" }, body: "{}" }), e);
  const before = { ...e._ops };
  const { res, body: up } = await start(e, bytes);
  assert.strictEqual(res.status, 200, String(up));
  assert.strictEqual(up.parts, 3, "12 MiB in 5 MiB parts");
  assert.strictEqual(up.part_size, 5 * MiB);

  // A part costs no KV operation at all: the token carries what it needs.
  const atParts = { ...e._ops };
  const parts = await sendParts(e, up, bytes);
  assert.deepStrictEqual(e._ops, atParts, "parts touch no KV");

  // Not served, not listed, while uploading or judging.
  const notYet = async () => {
    const r = await worker.fetch(req("GET", `/a/${hash}?dl=1`), e);
    assert.strictEqual(r.status, 404, "nothing is served before the verdict");
    assert.strictEqual(e._kv.get(`app:${hash}`), undefined, "nothing is listed before the verdict");
  };
  await notYet();

  // Finish needs every part, each once.
  assert.strictEqual((await finish(e, up, parts.slice(0, 2))).status, 400);
  assert.strictEqual((await finish(e, up, [parts[0], parts[0], parts[2]])).status, 400);
  const fin = await finish(e, up, parts);
  assert.strictEqual(fin.status, 202);
  assert.strictEqual((await fin.json()).status, "judging");
  await notYet();

  // Only the build service reads the upload or answers for it.
  assert.strictEqual((await worker.fetch(req("GET", `/internal/big/${up.id}/bytes`), e)).status, 404);
  const read = await worker.fetch(
    req("GET", `/internal/big/${up.id}/bytes`, { headers: { "x-krate-builder": SECRET } }),
    e,
  );
  assert.strictEqual(read.status, 200);
  assert.strictEqual(await sha256Hex(new Uint8Array(await read.arrayBuffer())), hash);
  assert.strictEqual((await verdict(e, up.id, { ok: true, sha256: hash }, "wrong")).status, 404);
  await notYet();

  const done = await (await verdict(e, up.id, { ok: true, sha256: hash, judgement: judgement() })).json();
  assert.strictEqual(done.status, "listed", JSON.stringify(done));
  // The app's own link (IC-976), and this build's pinned short link.
  assert.match(done.url, /^https:\/\/hub\.example\/a\/v[0-9a-f]{11}$/);
  assert.match(done.version_url, /^https:\/\/hub\.example\/a\/[0-9a-f]{12}$/);

  const listing = JSON.parse(e._kv.get(`app:${hash}`));
  assert.strictEqual(listing.big, true);
  assert.strictEqual(listing.size, bytes.byteLength);
  assert.strictEqual(listing.author_login, "alice");
  assert.deepStrictEqual(listing.capabilities.map((c) => c.cap), ["ui.window:create"]);

  const got = await worker.fetch(req("GET", `/a/${hash}?dl=1`), e);
  assert.strictEqual(got.status, 200);
  assert.strictEqual(got.headers.get("x-krate-tier"), "big");
  assert.strictEqual(got.headers.get("content-length"), String(bytes.byteLength));
  assert.strictEqual(await sha256Hex(new Uint8Array(await got.arrayBuffer())), hash);

  // Start 3 (the record, two day counters), finish 1, verdict 8 (owner,
  // address, listing, alias, channel, versions, the app's line (IC-976),
  // record): 12, against the free tier's 1,000 a day and the tier's 10 big
  // publishes a day.
  const writes = e._ops.put - before.put;
  assert.ok(writes <= 12, `a big publish costs at most 12 KV writes, not ${writes}`);

  // The status says where it went, to the token's holder.
  const status = await worker.fetch(
    req("GET", `/publish/big/${up.id}`, { headers: { "x-krate-upload": up.token } }),
    e,
  );
  assert.strictEqual((await status.json()).status, "listed");
}

/* ---- a part of the wrong size, and a token that is not this upload's --- */
{
  const e = env();
  const bytes = noise(11 * MiB, 3);
  const { body: up } = await start(e, bytes);
  const part = bytes.subarray(0, 5 * MiB - 1);
  const short = await worker.fetch(
    req("PUT", `/publish/big/${up.id}/part/1`, {
      headers: { "x-krate-upload": up.token, "content-length": String(part.byteLength) },
      body: part,
    }),
    e,
  );
  assert.strictEqual(short.status, 400);
  const [body, sig] = up.token.split(".");
  const claims = JSON.parse(Buffer.from(body, "base64url").toString());
  const forged = `${Buffer.from(JSON.stringify({ ...claims, size: 1 })).toString("base64url")}.${sig}`;
  for (const token of [forged, "", "x.y", up.token.slice(0, -2) + "AA"]) {
    const res = await worker.fetch(
      req("PUT", `/publish/big/${up.id}/part/1`, {
        headers: { "x-krate-upload": token, "content-length": String(5 * MiB) },
        body: bytes.subarray(0, 5 * MiB),
      }),
      e,
    );
    assert.strictEqual(res.status, 401, `a bad token is refused: ${token.slice(0, 12)}`);
  }
  // Another upload's token does not open this one.
  const { body: other } = await start(e, noise(6 * MiB, 9));
  const cross = await worker.fetch(
    req("PUT", `/publish/big/${up.id}/part/1`, {
      headers: { "x-krate-upload": other.token, "content-length": String(5 * MiB) },
      body: bytes.subarray(0, 5 * MiB),
    }),
    e,
  );
  assert.strictEqual(cross.status, 401);
}

/* ---- a refusal removes the upload, and says why ------------------------ */
{
  const e = env();
  const bytes = noise(6 * MiB, 5);
  const { body: up } = await start(e, bytes);
  const parts = await sendParts(e, up, bytes);
  await finish(e, up, parts);
  assert.ok(e.BUNDLES._blobs.has(`big/${up.id}`));
  const out = await (
    await verdict(e, up.id, { ok: false, sha256: await sha256Hex(bytes), problem: "bundle has no manifest.toml" })
  ).json();
  assert.strictEqual(out.status, "refused");
  assert.match(out.problem, /no manifest\.toml/);
  assert.ok(!e.BUNDLES._blobs.has(`big/${up.id}`), "a refused upload is removed");
  assert.strictEqual(e._kv.get(`bigkey:${await sha256Hex(bytes)}`), undefined);

  // Bytes that are not the declared ones are refused whatever the verdict.
  const other = noise(6 * MiB, 6);
  const { body: up2 } = await start(e, other);
  await finish(e, up2, await sendParts(e, up2, other));
  const lied = await (await verdict(e, up2.id, { ok: true, sha256: "0".repeat(64), judgement: judgement() })).json();
  assert.strictEqual(lied.status, "refused");
  assert.match(lied.problem, /not the ones the upload declared/);
}

/* ---- the quota: three big apps a day per publisher --------------------- */
{
  const e = env();
  for (let i = 0; i < 3; i++) {
    assert.strictEqual((await start(e, noise(6 * MiB, 20 + i))).res.status, 200);
  }
  const fourth = await start(e, noise(6 * MiB, 30));
  assert.strictEqual(fourth.res.status, 429);
  assert.match(fourth.body, /3 big apps a day/);
}

/* ---- the size ceiling, named --------------------------------------------- */
{
  const e = env();
  const res = await worker.fetch(
    req("POST", "/publish/big/start", {
      headers: { authorization: "Bearer gho_alice", "content-type": "application/json" },
      body: JSON.stringify({ kind: "bundle", size: 1024 * MiB + 1, sha256: "a".repeat(64) }),
    }),
    e,
  );
  assert.strictEqual(res.status, 413);
  assert.match(await res.text(), /opens from the file on a desktop/);
}

/* ---- the latest two versions of a big app are kept --------------------- */
{
  const e = env();
  const versions = [noise(6 * MiB, 41), noise(6 * MiB, 42), noise(6 * MiB, 43)];
  const hashes = [];
  for (const v of versions) {
    const { status } = await publishBig(e, v, "Veloren");
    assert.strictEqual(status.status, "listed", JSON.stringify(status));
    hashes.push(await sha256Hex(v));
  }
  assert.strictEqual(e._kv.get(`bigkey:${hashes[0]}`), undefined, "the oldest version's bytes go");
  assert.strictEqual(e._kv.get(`app:${hashes[0]}`), undefined);
  assert.strictEqual((await worker.fetch(req("GET", `/a/${hashes[0]}?dl=1`), e)).status, 404);
  for (const h of hashes.slice(1)) {
    assert.strictEqual((await worker.fetch(req("GET", `/a/${h}?dl=1`), e)).status, 200, "the latest two stay");
  }
  const kept = JSON.parse(e._kv.get("bigversions:alice:veloren"));
  assert.deepStrictEqual(kept, [hashes[2], hashes[1]]);
}

/* ---- source attaches to the publisher's own listing, and is served ----- */
{
  const e = env();
  const app = noise(6 * MiB, 51);
  await publishBig(e, app, "Veloren");
  const hash = await sha256Hex(app);
  const source = noise(300 * 1024, 52);
  const ask = (who) =>
    worker.fetch(
      req("POST", "/publish/big/start", {
        headers: { authorization: `Bearer ${who}`, "content-type": "application/json" },
        body: JSON.stringify({ kind: "source", for: hash, size: source.byteLength, sha256: "b".repeat(64) }),
      }),
      e,
    );
  e.BIG_PUBLISHERS = "alice,bob";
  assert.strictEqual((await ask("gho_bob")).status, 403, "only the app's publisher attaches its source");
  const { res, body: up } = await start(e, source, { kind: "source", for: hash });
  assert.strictEqual(res.status, 200, String(up));
  assert.strictEqual(up.parts, 1, "a small file is one part");
  await finish(e, up, await sendParts(e, up, source));
  assert.strictEqual((await worker.fetch(req("GET", `/a/${hash}/source`), e)).status, 404, "not before its verdict");
  const out = await (await verdict(e, up.id, { ok: true, sha256: await sha256Hex(source) })).json();
  assert.strictEqual(out.status, "listed", JSON.stringify(out));
  assert.strictEqual(out.url, `https://hub.example/a/${hash}/source`);
  const got = await worker.fetch(req("GET", `/a/${hash}/source`), e);
  assert.strictEqual(got.status, 200);
  assert.match(got.headers.get("content-disposition"), /veloren-source\.tar\.zst/);
  assert.strictEqual(await sha256Hex(new Uint8Array(await got.arrayBuffer())), await sha256Hex(source));

  // Unpublishing the app removes its big bytes and its source too.
  const gone = await worker.fetch(
    req("DELETE", `/app/${hash}`, { headers: { authorization: "Bearer gho_alice" } }),
    e,
  );
  assert.strictEqual(gone.status, 200);
  assert.deepStrictEqual(
    [...e.BUNDLES._blobs.keys()].filter((k) => k.startsWith("big/") && !k.includes("\u0000")),
    [],
    "no big upload is left behind",
  );
  assert.strictEqual(e._kv.get("bigversions:alice:veloren"), undefined);
}

/* ---- finish asks the build service to judge, with its secret ----------- */
{
  const e = env();
  e.KRATE_BUILDER_URL = "https://builder.example";
  const asked = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    // The sign-in (a stubbed GitHub token) is not the build service.
    if (String((url && url.url) || url).startsWith("https://api.github.com/")) return realFetch(url, init);
    asked.push({ url: String(url), secret: init.headers["x-krate-builder"], body: JSON.parse(init.body) });
    return new Response("{}", { status: 202 });
  };
  try {
    const bytes = noise(6 * MiB, 61);
    const { body: up } = await start(e, bytes);
    await finish(e, up, await sendParts(e, up, bytes));
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.strictEqual(asked.length, 1);
  assert.strictEqual(asked[0].url, "https://builder.example/judge");
  assert.strictEqual(asked[0].secret, SECRET);
  assert.strictEqual(asked[0].body.kind, "bundle");
}

console.log("big-upload: ok");
