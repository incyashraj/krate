/* Two doors a stranger walk found shut (2026-09-21).
 *
 * K-785: the support form said the email was optional and the hub refused
 * without one. Signed in, the reply lands in the person's own thread, so
 * the address buys nothing; the refusal is for a signed-out sender only,
 * and it is the one plain sentence the form already shows.
 *
 * K-798: `curl -I https://hub.krate.tech/a/<id>` answered 404 while GET
 * answered 200. Link checkers and chat unfurlers ask with HEAD first, so
 * every shared app link read as dead. HEAD now answers exactly like GET
 * with no body, for the bytes, the desktop redirect and the misses alike.
 *
 * Driven against the real worker, not a mirror of it.
 *
 *   node --experimental-wasm-modules cloud/worker/test/support-and-head.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const BOUNCE = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));

function env() {
  const kv = new Map([
    // A GitHub account with no public email: the case the old rule broke.
    ["session:krs_alice", "u1"],
    ["user:u1", JSON.stringify({ id: "u1", login: "alice", name: "Alice", avatar_url: "", email: "" })],
  ]);
  const blobs = new Map();
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    BUNDLES: r2Mock(blobs),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
  };
}

const req = (path, init = {}) => new Request(`https://hub.example${path}`, init);
const REFUSAL = "An email address, so the answer can reach you.";

function support(body, headers = {}) {
  return req("/support/new", {
    method: "POST",
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
}

/* ---- support: no email needed once signed in --------------------------- */
{
  const e = env();
  const signedIn = await worker.fetch(
    support({ subject: "the build stalled", text: "it sat at 40% for ten minutes" }, { authorization: "Bearer krs_alice" }),
    e,
  );
  assert.strictEqual(signedIn.status, 200, await signedIn.clone().text());
  const { id } = JSON.parse(await signedIn.text());
  const ticket = JSON.parse(e._kv.get(`tick:${id}`));
  assert.strictEqual(ticket.userId, "u1", "the ticket is the account's, so the reply lands in its thread");

  // An address typed anyway is kept, since this account carries none.
  const typed = await worker.fetch(
    support({ subject: "another", text: "words", email: "Alice@Example.com" }, { authorization: "Bearer krs_alice" }),
    e,
  );
  assert.strictEqual(typed.status, 200);
  assert.strictEqual(JSON.parse(e._kv.get(`tick:${JSON.parse(await typed.text()).id}`)).email, "alice@example.com");

  // Signed out, the same request without an address is refused in the
  // sentence the form shows, and with one it goes through.
  const anon = await worker.fetch(support({ subject: "the build stalled", text: "it sat there" }), e);
  assert.strictEqual(anon.status, 400);
  assert.strictEqual(await anon.text(), REFUSAL);
  const anonWithEmail = await worker.fetch(support({ subject: "s", text: "t", email: "bob@example.com" }), e);
  assert.strictEqual(anonWithEmail.status, 200, await anonWithEmail.clone().text());

  // A dead session counts as signed out: the refusal, not a silent ticket
  // nobody can answer.
  const stale = await worker.fetch(support({ subject: "s", text: "t" }, { authorization: "Bearer krs_gone" }), e);
  assert.strictEqual(stale.status, 400);
  assert.strictEqual(await stale.text(), REFUSAL);
}

/* ---- HEAD /a/<id> answers like GET, without the body -------------------- */
{
  const e = env();
  const pub = await worker.fetch(req("/publish", { method: "POST", headers: { authorization: "Bearer krs_alice" }, body: BOUNCE }), e);
  assert.strictEqual(pub.status, 200, await pub.clone().text());
  const id = JSON.parse(await pub.text()).id;

  const get = await worker.fetch(req(`/a/${id}?dl=1`), e);
  assert.strictEqual(get.status, 200);
  const bytes = new Uint8Array(await get.arrayBuffer());

  const head = await worker.fetch(req(`/a/${id}?dl=1`, { method: "HEAD" }), e);
  assert.strictEqual(head.status, 200, "HEAD is the same door as GET");
  assert.strictEqual((await head.arrayBuffer()).byteLength, 0, "and carries no body");
  assert.strictEqual(head.headers.get("content-type"), get.headers.get("content-type"));
  assert.strictEqual(head.headers.get("content-disposition"), get.headers.get("content-disposition"));
  assert.strictEqual(head.headers.get("cache-control"), get.headers.get("cache-control"));
  assert.strictEqual(Number(head.headers.get("content-length")), bytes.byteLength, "the size a HEAD is asked for");
  assert.strictEqual(head.headers.get("access-control-allow-origin"), "*");

  // A desktop browser is walked to the receive page by HEAD too, so an
  // unfurler sees the same 302 a person would follow.
  const browser = { "user-agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/128.0 Safari/537.36", accept: "text/html" };
  const headBrowser = await worker.fetch(req(`/a/${id}`, { method: "HEAD", headers: browser }), e);
  const getBrowser = await worker.fetch(req(`/a/${id}`, { headers: browser }), e);
  assert.strictEqual(headBrowser.status, getBrowser.status);
  assert.strictEqual(headBrowser.headers.get("location"), getBrowser.headers.get("location"));

  // A miss is a miss either way, and still bodiless.
  const miss = await worker.fetch(req(`/a/${"0".repeat(64)}`, { method: "HEAD" }), e);
  assert.strictEqual(miss.status, 404);
  assert.strictEqual((await miss.arrayBuffer()).byteLength, 0);
}

console.log("OK -- support takes a signed-in report without an address and refuses a signed-out one in the form's own sentence; HEAD /a/<id> answers like GET with the size and no body, for the bytes, the redirect and a miss");
