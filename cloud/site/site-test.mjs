// krate.tech's Worker (cloud/site/site.js): the Claude Desktop downloads
// redirect to the hub's stable address (IC-1005), and nothing else does.
//
//   node cloud/site/site-test.mjs
import assert from "node:assert/strict";
import site, { desktopDownload } from "./site.js";

assert.equal(desktopDownload("/claude/krate-mac.mcpb"), "https://hub.krate.tech/claude/krate-mac.mcpb");
assert.equal(desktopDownload("/claude/krate-windows.mcpb"), "https://hub.krate.tech/claude/krate-windows.mcpb");
for (const other of ["/claude/", "/claude/krate-linux.mcpb", "/krate-mac.mcpb", "/claude/krate-mac.mcpb/x"]) {
  assert.equal(desktopDownload(other), null, other);
}
const noAssets = { ASSETS: { fetch: () => { throw new Error("a download address never reads an asset"); } } };
const r = await site.fetch(new Request("https://krate.tech/claude/krate-windows.mcpb"), noAssets);
assert.equal(r.status, 302);
assert.equal(r.headers.get("location"), "https://hub.krate.tech/claude/krate-windows.mcpb");

// The page itself is still an asset.
let asked = null;
const page = await site.fetch(new Request("https://krate.tech/claude/"), {
  ASSETS: { fetch: (req) => { asked = new URL(req.url).pathname; return new Response("<h1>page</h1>", { status: 200 }); } },
});
assert.equal(page.status, 200);
assert.equal(asked, "/claude/");
console.log("site-test: ok");
