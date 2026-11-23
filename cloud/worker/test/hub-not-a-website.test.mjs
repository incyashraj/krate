/* The hub keeps search engines out, and its front door sends people to the site.
 *
 * Google crawled hub.krate.tech's API addresses (/, /meta/, /run/, /saml/,
 * /scim/v2/, jwks.json) and listed them in Search Console as pages it could
 * not index (2026-11-23). The hub had no robots.txt and "/" answered 404.
 * A developer's own name (alice.krate.tech) is a different host with its
 * own pages, and must not be closed by this.
 *
 *   node --experimental-wasm-modules cloud/worker/test/hub-not-a-website.test.mjs
 */
import assert from "node:assert/strict";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

const kv = new Map();
const env = {
  APPS: {
    get: async (k) => kv.get(k) ?? null,
    put: async (k, v) => { kv.set(k, v); },
    delete: async (k) => { kv.delete(k); },
    list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
  },
  BUNDLES: r2Mock(new Map()),
  PUBLIC_BASE: "https://hub.krate.tech",
};
const get = (url) => worker.fetch(new Request(url), env, { waitUntil() {} });

{
  const r = await get("https://hub.krate.tech/robots.txt");
  assert.equal(r.status, 200);
  assert.equal(await r.text(), "User-agent: *\nDisallow: /\n");
  console.log("ok  the hub tells crawlers it is not a website");
}
{
  const r = await get("https://hub.krate.tech/");
  assert.equal(r.status, 301);
  assert.equal(r.headers.get("location"), "https://krate.tech/");
  console.log("ok  the hub's front door sends people to krate.tech");
}
{
  const r = await get("https://hub.krate.tech/health");
  assert.notEqual(r.status, 301, "API routes are untouched");
  console.log("ok  API routes answer as before");
}
