/* A declared length is refused before the body is read (IC-833, test 1821).
 *
 * `publish` used to call `request.arrayBuffer()` and check the size
 * afterwards, so a client announcing 500 MB was allocated 500 MB before
 * being told the limit was 5 MiB. The ceiling was real and enforced after
 * the cost it exists to prevent.
 *
 * This drives the REAL worker rather than mirroring its logic. The other
 * tests in this directory mirror on purpose -- the worker is one file and
 * most of what they check is decision logic -- but a mirror cannot test
 * ORDERING. A copy that checks the header first would pass whatever the
 * product does, which is precisely the bug. So the handler is imported and
 * called, and the proof that the body was never read is that the real
 * request's arrayBuffer() was never invoked -- see `watched` below for why
 * the obvious probe does not work.
 *
 *   node cloud/worker/test/upload-bounds.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

const MAX = 5 * 1024 * 1024;

/* A KV that holds one signed-in session, which is all publish needs to get
 * past the door and reach the size check. */
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
  };
}

/* A request that records whether the HANDLER asked for its body.
 *
 * The first version of this used a ReadableStream that threw when pulled,
 * which proved nothing: measured, Node's own Request constructor drains the
 * stream a tick after construction whether or not anyone reads it, so the
 * flag was set before the handler ran and the probe reported the bug on
 * correct code.
 *
 * What only the handler can do is call arrayBuffer() or take the body
 * stream. Both are counted -- on the real Request, leaving every other
 * behaviour intact. The handler reads the stream since IC-833's bounded
 * read landed; counting arrayBuffer alone would then prove nothing. */
function watched(headers, bytes = new Uint8Array(8)) {
  const req = new Request("https://hub.example/publish", {
    method: "POST",
    headers,
    body: bytes,
  });
  let read = false;
  const real = req.arrayBuffer.bind(req);
  req.arrayBuffer = () => {
    read = true;
    return real();
  };
  const bodyGetter = Object.getOwnPropertyDescriptor(Request.prototype, "body").get;
  Object.defineProperty(req, "body", {
    get() {
      read = true;
      return bodyGetter.call(this);
    },
  });
  return { req, wasRead: () => read };
}

function readable(bytes, headers) {
  return new Request("https://hub.example/publish", {
    method: "POST",
    headers,
    body: bytes,
  });
}

const AUTH = { authorization: "Bearer krs_test", "content-type": "application/octet-stream" };

/* ---- an oversized declaration is refused without reading ---------------- */
{
  const { req, wasRead } = watched({ ...AUTH, "content-length": String(500 * 1024 * 1024) });
  const res = await worker.fetch(req, env());
  assert.strictEqual(res.status, 413, await res.text());
  assert.ok(
    !wasRead(),
    "the body was read before the declared length was judged -- that is the allocation this refuses",
  );
}

/* ---- the refusal is the same at one byte over, not only at 500 MB ------- */
{
  const { req, wasRead } = watched({ ...AUTH, "content-length": String(MAX + 1) });
  const res = await worker.fetch(req, env());
  assert.strictEqual(res.status, 413, await res.text());
  assert.ok(!wasRead(), "one byte over the limit must also refuse before reading");
}

/* ---- exactly at the limit is NOT refused by the header ------------------
 * The boundary matters: an off-by-one here would reject the largest legal
 * bundle. It does not have to succeed -- it is not a real .krate -- it just
 * must not be refused for its size.
 *
 * The header is set explicitly. Measured: Node's Request does NOT add a
 * content-length of its own, so the first version of this case sent none
 * and never reached the header check at all -- it exercised the body check
 * and reported the boundary as covered. Changing `>` to `>=` in the header
 * check passed it. */
{
  const body = new Uint8Array(MAX);
  const res = await worker.fetch(
    readable(body, { ...AUTH, "content-length": String(MAX) }),
    env(),
  );
  assert.notStrictEqual(res.status, 413, "a bundle exactly at the limit is not too large");
}

/* ---- a lying header does not buy a bigger body -------------------------
 * The header is the client's claim. An undercount must still meet the
 * check on the bytes that actually arrived. */
{
  const body = new Uint8Array(MAX + 1024);
  const res = await worker.fetch(
    readable(body, { ...AUTH, "content-length": "10" }),
    env(),
  );
  assert.strictEqual(res.status, 413, await res.text());
}

/* ---- a nonsense header is refused, not ignored --------------------------
 * Each value is asserted against what it actually means rather than lumped
 * together. "1e9" is the interesting one: it is a perfectly good integer
 * written in exponent form, so it is not nonsense -- it is a billion, and a
 * billion is too large. Asserting 400 for it, as the first version of this
 * did, would have been asserting a bug. */
for (const [bad, want, why] of [
  ["banana", 400, "not a number at all"],
  ["-1", 400, "a negative length is not a length"],
  ["4.5", 400, "a fractional byte count is not a length"],
  ["1e9", 413, "a valid integer in exponent form, and too large"],
]) {
  const { req } = watched({ ...AUTH, "content-length": bad });
  const res = await worker.fetch(req, env());
  assert.strictEqual(
    res.status, want,
    `content-length ${JSON.stringify(bad)} is ${why}: ${await res.text()}`,
  );
}

/* ---- no header at all still works -------------------------------------
 * Chunked uploads send no content-length. They must not be refused, and the
 * real check still applies to them. */
{
  const body = new Uint8Array(MAX + 1024);
  const res = await worker.fetch(readable(body, { ...AUTH }), env());
  assert.strictEqual(res.status, 413, "an oversized chunked body is still refused by the real length");
}
{
  const body = new Uint8Array(64); // small, not a .krate
  const res = await worker.fetch(readable(body, { ...AUTH }), env());
  assert.strictEqual(res.status, 422, "a small body reaches the bundle check, not the size one");
}

/* ---- an empty body is still an empty body ------------------------------ */
{
  const res = await worker.fetch(
    readable(new Uint8Array(0), { ...AUTH, "content-length": "0" }),
    env(),
  );
  assert.strictEqual(res.status, 400, await res.text());
}

/* ---- and none of this weakens the door --------------------------------
 * An oversized upload from someone who is not signed in is refused for not
 * being signed in. The size check must not become a way to probe the hub
 * without an account. */
{
  const { req } = watched({ "content-length": String(500 * 1024 * 1024) });
  const res = await worker.fetch(req, env());
  assert.strictEqual(res.status, 401, await res.text());
}

console.log(
  "OK -- a declared length is judged before the body is read, a lying header buys nothing, " +
    "the boundary is exact, and sign-in still comes first",
);
