/* The browser Player's side of the hub (IC-956 Phase 1): each release's web
 * pack made once at publish by the real transpiler (crates/web-transpile
 * compiled to wasm), served at /a/<hash>/web behind exactly /a/'s checks,
 * the Player page cross-origin isolated, and a sign-in that comes back to
 * the app the tab was opening.
 *
 *   scripts/build-hub-validator.sh && scripts/build-web-player.sh
 *   node --experimental-wasm-modules --test cloud/worker/test/web-player.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, acme, fixture, worker } from "./org-tools.mjs";
import { sha256Hex } from "./r2-mock.mjs";
import { WEB_PACK_TAG } from "../src/web-player.js";

const here = dirname(fileURLToPath(import.meta.url));
const BOUNCE = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));

/* The Player's files, as Workers static assets would hold them. */
function withAssets(env) {
  const files = { "/run.html": "<!doctype html><title>Player</title>", "/player.js": "export {}" };
  env.ASSETS = {
    fetch: async (request) => {
      const path = new URL(request.url).pathname;
      return files[path] ? new Response(files[path]) : new Response("not found", { status: 404 });
    },
  };
  return env;
}

function unpack(bytes) {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const blen = dv.getUint32(0, true);
  const bundle = bytes.subarray(4, 4 + blen);
  const hlen = dv.getUint32(4 + blen, true);
  const header = JSON.parse(new TextDecoder().decode(bytes.subarray(8 + blen, 8 + blen + hlen)));
  return { bundle, header, files: bytes.subarray(8 + blen + hlen) };
}

test("a publish makes the release's web pack once, and /a/<hash>/web serves it with the bundle", async () => {
  const env = withAssets(orgEnv());
  let packPuts = 0;
  const put = env.BUNDLES.put;
  env.BUNDLES.put = (k, ...a) => ((packPuts += String(k).startsWith("web/") ? 1 : 0), put(k, ...a));
  const hash = await sha256Hex(BOUNCE);
  const first = await call(env, "alice", "POST", "/publish", { bytes: BOUNCE });
  assert.strictEqual(first.status, 200, JSON.stringify(first.body));
  assert.ok(env.BUNDLES._blobs.has(`web/${WEB_PACK_TAG}/${hash}`), "made at publish");
  const again = await call(env, "alice", "POST", "/publish", { bytes: BOUNCE });
  assert.strictEqual(again.status, 200);
  assert.strictEqual(packPuts, 1, "the same bytes are never transpiled twice");

  const res = await call(env, null, "GET", `/a/${hash}/web`);
  assert.strictEqual(res.status, 200, JSON.stringify(res.body));
  assert.strictEqual(res.headers.get("cross-origin-resource-policy"), "same-origin");
  const { bundle, header, files } = unpack(res.bytes);
  assert.deepStrictEqual([...bundle], [...BOUNCE], "the bundle exactly as stored");
  assert.strictEqual(header.ok, true);
  assert.ok(header.files.some((f) => f.name === "app.js"));
  const wasm = header.files.filter((f) => f.name.endsWith(".wasm"));
  assert.ok(wasm.length > 0 && wasm.every((f) => f.embedded && header.modules.includes(f.sha256)), "every core file is a module of the component");
  assert.strictEqual(files.length, header.files.reduce((n, f) => n + f.size, 0));
  assert.strictEqual(packPuts, 1, "serving it makes nothing");
});

test("a release published before the Player gets its pack on its first open", async () => {
  const env = withAssets(orgEnv());
  const hash = await sha256Hex(BOUNCE);
  assert.strictEqual((await call(env, "alice", "POST", "/publish", { bytes: BOUNCE })).status, 200);
  env.BUNDLES._blobs.delete(`web/${WEB_PACK_TAG}/${hash}`);
  const res = await call(env, null, "GET", `/a/${hash}/web`);
  assert.strictEqual(res.status, 200);
  assert.ok(env.BUNDLES._blobs.has(`web/${WEB_PACK_TAG}/${hash}`));
  assert.strictEqual((await call(env, null, "GET", `/a/${"0".repeat(64)}/web`)).status, 404);
});

test("an organisation's private release opens in a tab only for whom /a/ would hand it", async () => {
  const env = withAssets(orgEnv());
  await acme(env);
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  const digest = up.body.digest;
  assert.ok(env.BUNDLES._blobs.has(`web/${WEB_PACK_TAG}/${digest}`), "made when the candidate arrives");
  assert.strictEqual((await call(env, "bob", "POST", `/orgs/acme/candidates/${digest}/approve`)).status, 200);

  const nobody = await call(env, null, "GET", `/a/${digest}/web`);
  assert.deepStrictEqual([nobody.status, nobody.body.error], [403, "not-signed-in"]);
  const stranger = await call(env, "stranger", "GET", `/a/${digest}/web`);
  assert.deepStrictEqual([stranger.status, stranger.body.error], [403, "not-a-member"]);
  assert.ok(!JSON.stringify(stranger.body).includes("acme"), "a stranger learns no organisation (R8)");
  const member = await call(env, "dan", "GET", `/a/${digest}/web`);
  assert.strictEqual(member.status, 200);
  assert.match(member.headers.get("cache-control"), /private, no-store/);
  assert.ok(unpack(member.bytes).header.ok);
});

test("the Player page and its files are cross-origin isolated; nothing else is served there", async () => {
  const env = withAssets(orgEnv());
  const page = await worker.fetch(new Request(`https://hub.example/run/${"a".repeat(64)}`), env);
  assert.strictEqual(page.status, 200);
  assert.strictEqual(page.headers.get("cross-origin-opener-policy"), "same-origin");
  assert.strictEqual(page.headers.get("cross-origin-embedder-policy"), "require-corp");
  assert.match(page.headers.get("content-security-policy"), /script-src 'self' blob: 'wasm-unsafe-eval'/);
  assert.match(page.headers.get("content-type"), /text\/html/);
  const js = await worker.fetch(new Request("https://hub.example/player/player.js"), env);
  assert.strictEqual(js.headers.get("cross-origin-embedder-policy"), "require-corp");
  assert.match(js.headers.get("content-type"), /javascript/);
  for (const path of ["/player/secret.txt", "/player/../index.js", "/run/not-a-hash"]) {
    const res = await worker.fetch(new Request(`https://hub.example${path}`), env);
    assert.strictEqual(res.status, 404, path);
  }
  // Without the built Player the page says what to run, never a blank page.
  delete env.ASSETS;
  const bare = await worker.fetch(new Request(`https://hub.example/run/${"a".repeat(64)}`), env);
  assert.strictEqual(bare.status, 503);
  assert.match(await bare.text(), /build-web-player/);
});

test("a sign-in the Player started comes back to that app's page on this hub, with its nonce", async () => {
  const env = withAssets(orgEnv());
  const hash = "b".repeat(64);
  const nonce = "n0nce-n0nce-n0nce-1";
  env._kv.set("email:tok1", JSON.stringify({ email: "erin@example.com", from: `player:${hash}`, nonce, app_nonce: "" }));
  const res = await worker.fetch(new Request("https://hub.example/login/email/verify?t=tok1"), env);
  assert.strictEqual(res.status, 302);
  const location = res.headers.get("location");
  assert.ok(location.startsWith(`/run/${hash}#`), location);
  const hand = new URLSearchParams(location.split("#")[1]);
  assert.strictEqual(hand.get("nonce"), nonce);
  assert.ok(hand.get("token"));
  // A `from` that names anything else is an ordinary web sign-in.
  env._kv.set("email:tok2", JSON.stringify({ email: "erin@example.com", from: "player:https://evil.example", nonce, app_nonce: "" }));
  const other = await worker.fetch(new Request("https://hub.example/login/email/verify?t=tok2"), env);
  assert.ok(other.headers.get("location").startsWith("https://krate.tech/login/done/"), other.headers.get("location"));
});

test("a company app can go on a home screen as itself: its manifest, its icon, and an address that opens the current release", async () => {
  const env = withAssets(orgEnv());
  const hash = "c".repeat(64);
  const get = (path) => worker.fetch(new Request(`https://hub.example${path}`), env);
  // The organisation's address for an app serves the same page.
  const page = await get("/run/org/acme/dev.acme.inspections");
  assert.strictEqual(page.status, 200);
  assert.strictEqual(page.headers.get("cross-origin-embedder-policy"), "require-corp");
  for (const bad of ["/run/org/Acme/x", "/run/org/acme/", "/run/org/acme/a/b", `/run/${hash}/other.json`, "/run/org/a/x"]) {
    assert.strictEqual((await get(bad)).status, 404, bad);
  }
  const res = await get("/run/org/acme/dev.acme.inspections/app.webmanifest?name=" + encodeURIComponent("Site inspections\u0007<b>"));
  assert.strictEqual(res.status, 200);
  assert.match(res.headers.get("content-type"), /application\/manifest\+json/);
  const m = await res.json();
  assert.deepStrictEqual(
    [m.id, m.start_url, m.scope, m.display, m.name, m.short_name],
    ["/run/org/acme/dev.acme.inspections", "/run/org/acme/dev.acme.inspections", "/run/", "standalone", "Site inspectionsb", "Site"],
  );
  assert.ok(m.icons.some((i) => i.type === "image/png" && i.src === "/player/krate-app-icon.png"));
  const svgIcon = m.icons.find((i) => i.type === "image/svg+xml");
  const icon = await get(svgIcon.src);
  assert.strictEqual(icon.status, 200);
  assert.match(icon.headers.get("content-type"), /image\/svg\+xml/);
  const svg = await icon.text();
  assert.match(svg, />S<\/text><\/svg>$/, "the name's initial");
  // A name cannot write into the icon.
  const evil = await (await get(`/run/${hash}/icon.svg?name=${encodeURIComponent("\"/><script>1</script>")}`)).text();
  assert.ok(!evil.includes("<script"), evil);
  const one = await (await get(`/run/${hash}/app.webmanifest`)).json();
  assert.deepStrictEqual([one.start_url, one.name], [`/run/${hash}`, "Krate app"]);
});

test("a sign-in started from an organisation's app address comes back to it", async () => {
  const env = withAssets(orgEnv());
  const nonce = "n0nce-n0nce-n0nce-2";
  env._kv.set("email:tok3", JSON.stringify({ email: "erin@example.com", from: "player:org/acme/dev.acme.inspections", nonce, app_nonce: "" }));
  const res = await worker.fetch(new Request("https://hub.example/login/email/verify?t=tok3"), env);
  assert.ok(res.headers.get("location").startsWith("/run/org/acme/dev.acme.inspections#"), res.headers.get("location"));
  for (const from of ["player:org/acme/../x", "player:org/ACME/x", "player:org/acme/x/y"]) {
    env._kv.set("email:tok4", JSON.stringify({ email: "erin@example.com", from, nonce, app_nonce: "" }));
    const other = await worker.fetch(new Request("https://hub.example/login/email/verify?t=tok4"), env);
    assert.ok(other.headers.get("location").startsWith("https://krate.tech/login/done/"), `${from}: ${other.headers.get("location")}`);
  }
});
