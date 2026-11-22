/* Studio's interface updates on the hub (IC-998, src/studio-ui.js).
 *
 * The hub holds the bundle and its signature; only an admin may replace
 * them, never with an older one; anyone may read them, because every
 * Studio checks the signature itself.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/studio-ui.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";
import "./github-stub.mjs";

function env() {
  const kv = new Map();
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
    },
    BUNDLES: r2Mock(new Map()),
    PUBLIC_BASE: "https://hub.example",
    KRATE_ADMINS: "boss",
  };
}
const hit = async (e, path, { method = "GET", token, json } = {}) => {
  const headers = {};
  if (token) headers.authorization = `Bearer ${token}`;
  if (json) headers["content-type"] = "application/json";
  const r = await worker.fetch(new Request(`https://hub.example${path}`, { method, headers, body: json ? JSON.stringify(json) : undefined }), e, { waitUntil() {} });
  return { status: r.status, text: await r.text() };
};
const bundle = (serial) => JSON.stringify({ format: 1, serial, commit: "abc", commands: [], files: { "index.html": "PHA+" } });
const SIG = "ab".repeat(64);

test("an admin ships a bundle and every Studio can read it", async () => {
  const e = env();
  assert.equal(JSON.parse((await hit(e, "/studio-ui/latest")).text).serial, 0, "nothing shipped yet");
  const put = await hit(e, "/admin/studio-ui", { method: "PUT", token: "gho_boss", json: { bundle: bundle(200), sig: SIG } });
  assert.equal(put.status, 200, put.text);
  const latest = JSON.parse((await hit(e, "/studio-ui/latest")).text);
  assert.deepEqual([latest.serial, latest.commit], [200, "abc"]);
  assert.equal((await hit(e, "/studio-ui/bundle")).text, bundle(200), "the exact bytes that were signed");
  assert.equal((await hit(e, "/studio-ui/bundle.sig")).text, SIG);
});

test("nobody else may ship one, and an older one never replaces a newer", async () => {
  const e = env();
  const stranger = await hit(e, "/admin/studio-ui", { method: "PUT", token: "gho_mallory", json: { bundle: bundle(300), sig: SIG } });
  assert.equal(stranger.status, 404);
  assert.equal((await hit(e, "/studio-ui/bundle")).status, 404);
  await hit(e, "/admin/studio-ui", { method: "PUT", token: "gho_boss", json: { bundle: bundle(300), sig: SIG } });
  const older = await hit(e, "/admin/studio-ui", { method: "PUT", token: "gho_boss", json: { bundle: bundle(250), sig: SIG } });
  assert.equal(older.status, 409, older.text);
  const bad = await hit(e, "/admin/studio-ui", { method: "PUT", token: "gho_boss", json: { bundle: bundle(400), sig: "nothex" } });
  assert.equal(bad.status, 400);
});
