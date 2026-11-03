/* A .krate file opened in the tab (/run/file): the hub serves the page and
 * the manifest that lets an installed Player take .krate files, and never
 * reads a file shared to it. The page itself is proved in a browser by
 * crates/adapter-web/player/open-file-proof.mjs.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/open-file.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { RUN_PATH, FILE_PAGE, playerPage, playerAsset, fileShareFallback } from "../src/web-player.js";

const env = { ASSETS: { fetch: async (req) => new Response(`// ${new URL(req.url).pathname}`) } };
const get = (path) => {
  const url = new URL(`https://hub.example${path}`);
  return path.startsWith("/player/") ? playerAsset(new Request(url), url, env) : playerPage(new Request(url), url, env);
};

test("/run/file is a Player address, and the groups others read keep their numbers", () => {
  for (const path of ["/run/file", "/run/file/", "/run/file/app.webmanifest", "/run/file/icon.svg"]) {
    const m = RUN_PATH.exec(path);
    assert.ok(m, path);
    assert.equal(m[1], undefined, path);
    assert.equal(m[2], undefined, path);
    assert.equal(m[3], undefined, path);
  }
  assert.equal(RUN_PATH.exec("/run/file/app.webmanifest")[4], "app.webmanifest");
  const hash = "a".repeat(64);
  assert.equal(RUN_PATH.exec(`/run/${hash}`)[1], hash);
  assert.equal(RUN_PATH.exec("/run/org/acme/notes")[3], "notes");
  assert.equal(RUN_PATH.exec("/run/files"), null);
  assert.equal(RUN_PATH.exec("/run/file/x"), null);
});

test("the file page is the Player page, cross-origin isolated", async () => {
  const res = await get("/run/file");
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "// /run.html");
  assert.equal(res.headers.get("cross-origin-embedder-policy"), "require-corp");
  assert.equal(res.headers.get("cross-origin-opener-policy"), "same-origin");
});

test("the file manifest opens .krate files and takes them shared, within /run/file", async () => {
  const res = await get("/run/file/app.webmanifest");
  assert.match(res.headers.get("content-type"), /application\/manifest\+json/);
  const m = await res.json();
  assert.equal(m.id, FILE_PAGE);
  assert.equal(m.scope, FILE_PAGE);
  assert.equal(m.start_url, FILE_PAGE);
  assert.deepEqual(m.file_handlers, [{ action: FILE_PAGE, accept: { "application/x-krate": [".krate"] } }]);
  assert.equal(m.share_target.action, FILE_PAGE);
  assert.equal(m.share_target.method, "POST");
  assert.equal(m.share_target.enctype, "multipart/form-data");
  assert.equal(m.share_target.params.files[0].name, "file");
  assert.ok(m.share_target.params.files[0].accept.includes(".krate"));
});

test("the page's transpiler and its service worker are served, the worker for /run/file only", async () => {
  const t = await get("/player/transpiler.wasm");
  assert.equal(t.status, 200);
  assert.match(t.headers.get("content-type"), /application\/wasm/);
  const sw = await get("/player/file-sw.js");
  assert.equal(sw.status, 200);
  assert.equal(sw.headers.get("service-worker-allowed"), "/run/file");
  assert.equal((await get("/player/notify-sw.js")).headers.get("service-worker-allowed"), null);
});

test("a share that reaches the hub is sent to the page, its body never read", async () => {
  let pulled = 0;
  const body = new ReadableStream({ pull(c) { pulled += 1; c.enqueue(new Uint8Array(1024)); if (pulled === 64) c.close(); } }, { highWaterMark: 0 });
  const res = await fileShareFallback(new Request("https://hub.example/run/file", { method: "POST", body, duplex: "half" }));
  assert.equal(res.status, 303);
  assert.equal(res.headers.get("location"), "/run/file?share=lost");
  assert.ok(pulled < 4, `the body was read: ${pulled} chunks`);
});
