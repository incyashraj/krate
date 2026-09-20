/* A bug report goes through WITHOUT a sign-in (K-757).
 *
 * It did not, and the shape of that failure is worth stating plainly: the
 * person whose product just broke was asked to authenticate before he was
 * allowed to say so. An outside user hit exactly that -- his build failed,
 * he pressed "Send to support", and could not get through the login page
 * with GitHub, Google or email. The failure suppressed the report about
 * itself, and we heard about it only because he told the founder by hand.
 *
 * The identity was buying nothing. `from` was written into the metadata and
 * read by no code anywhere -- not the admin list, not the detail view. We
 * need the zip, not the name.
 *
 * This drives the REAL worker, not a mirror of its logic. A mirror would
 * have passed the whole time the product refused, because the mirror is
 * written by whoever is sure they know what the product does.
 *
 *   node cloud/worker/test/report-anonymous.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

/* A KV holding one signed-in session, so the named case has an identity to
 * find. The anonymous case simply sends no Authorization header. */
function env() {
  const kv = new Map([
    ["session:krs_test", "u1"],
    ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      list: async () => ({ keys: [], list_complete: true }),
    },
    BUNDLES: r2Mock(),
    _kv: kv,
  };
}

/* The smallest thing the endpoint accepts: it checks for the PK zip magic,
 * so the bytes have to start with it. */
function reportZip() {
  return new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0, 0, 0, 0]);
}

function post(headers = {}) {
  return new Request("https://hub.example/report", {
    method: "POST",
    headers: {
      "Content-Type": "application/zip",
      "X-Krate-Session": "s-1789877746276",
      "X-Krate-Version": "0.5.0",
      "X-Krate-Os": "macos",
      ...headers,
    },
    body: reportZip(),
  });
}

async function run() {
  /* 1. No sign-in at all. This is the case that was refused, and it is the
   *    case that matters: a person in trouble, with no account. */
  {
    const e = env();
    const res = await worker.fetch(post(), e);
    assert.notStrictEqual(
      res.status, 401,
      "a bug report must never be refused for want of a sign-in -- that is the bug",
    );
    assert.strictEqual(res.status, 200, `expected the report to be accepted, got ${res.status}`);
    const body = await res.json();
    assert.ok(body.ok && body.id, `the sender must get a reference back, got ${JSON.stringify(body)}`);

    // And it must actually be stored, not merely acknowledged. A 200 that
    // drops the zip would pass a status-only assertion and lose the report.
    const meta = JSON.parse(e._kv.get(`report:${body.id}`));
    assert.strictEqual(meta.from, "anonymous", `an unsigned report is from anonymous, got ${meta.from}`);
    assert.strictEqual(meta.krate, "0.5.0", "the version must survive -- it is half the diagnosis");
    assert.strictEqual(meta.os, "macos", "the OS must survive");
    assert.strictEqual(
      meta.session, "s-1789877746276",
      "the session id must survive, or the report cannot be tied to the failure",
    );
  }

  /* 2. A signed-in person is still identified. Anonymity is the floor, not
   *    the ceiling: a report we can reply to is worth more. */
  {
    const e = env();
    const res = await worker.fetch(post({ Authorization: "Bearer krs_test" }), e);
    assert.strictEqual(res.status, 200, "a signed-in report must still be accepted");
    const body = await res.json();
    const meta = JSON.parse(e._kv.get(`report:${body.id}`));
    assert.strictEqual(meta.from, "alice", `a signed-in report keeps its author, got ${meta.from}`);
    assert.strictEqual(meta.name, "Alice", "and their display name");
  }

  /* 3. A bad token must not become a 401 by the back door. Someone with a
   *    stale session is in the same position as someone with none -- in
   *    trouble, trying to tell us -- and must not be stopped either. */
  {
    const e = env();
    const res = await worker.fetch(post({ Authorization: "Bearer krs_expired" }), e);
    assert.strictEqual(
      res.status, 200,
      `a stale sign-in must fall back to anonymous, not refuse the report (got ${res.status})`,
    );
    const meta = JSON.parse(e._kv.get(`report:${(await res.json()).id}`));
    assert.strictEqual(meta.from, "anonymous", "an unverifiable token is treated as no token");
  }

  /* 4. The bounds still hold. Dropping the sign-in must not drop the
   *    checks that keep this endpoint from being a free object store. */
  {
    const e = env();
    const notAZip = new Request("https://hub.example/report", {
      method: "POST",
      headers: { "Content-Type": "application/zip" },
      body: new Uint8Array([1, 2, 3, 4]),
    });
    const res = await worker.fetch(notAZip, e);
    assert.strictEqual(res.status, 422, `a non-zip must still be refused, got ${res.status}`);
  }
  {
    const e = env();
    const empty = new Request("https://hub.example/report", {
      method: "POST",
      headers: { "Content-Type": "application/zip" },
      body: new Uint8Array([]),
    });
    const res = await worker.fetch(empty, e);
    assert.strictEqual(res.status, 413, `an empty body must still be refused, got ${res.status}`);
  }

  console.log("ok -- a report sends without a sign-in, keeps its author when there is one, and the bounds still hold");
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
