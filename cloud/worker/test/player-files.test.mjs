/* Every file scripts/build-web-player.sh puts in player-dist is one the hub
 * serves under /player/ (web-player.js PLAYER_FILES). A file the build
 * ships and the allow-list forgets is a 404 in production only -- the
 * proofs serve player-dist directly -- which is how sql.wasm nearly went.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/player-files.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { playerAsset } from "../src/web-player.js";

const here = dirname(fileURLToPath(import.meta.url));
const script = readFileSync(join(here, "..", "..", "..", "scripts", "build-web-player.sh"), "utf8");

/* The names the build writes into "$out": copied page files, renamed
 * copies, and wasm-bindgen's krate_web pair. */
function shipped() {
  const names = new Set(["krate_web.js", "krate_web_bg.wasm"]);
  for (const line of script.split("\\\n").join(" ").split("\n")) {
    if (!/^\s*cp\s/.test(line)) continue;
    const parts = line.trim().split(/\s+/).slice(1);
    const dest = parts.pop();
    if (/^"\$out"\/[^"]+$/.test(dest)) {
      names.add(dest.slice('"$out"/'.length));
    } else if (dest === '"$out"/') {
      for (const p of parts) names.add(p.slice(p.lastIndexOf("/") + 1).replace(/"$/, ""));
    }
  }
  return [...names];
}

test("the hub serves every file the Player build ships", async () => {
  const names = shipped();
  assert.ok(names.includes("sql.wasm") && names.includes("sqldb.js") && names.includes("hosts.js"), JSON.stringify(names));
  const env = { ASSETS: { fetch: async (req) => new Response(`// ${new URL(req.url).pathname}`) } };
  for (const name of names) {
    const url = new URL(`https://hub.example/player/${name}`);
    const res = await playerAsset(new Request(url), url, env);
    assert.equal(res.status, 200, `${name} is built into player-dist but the hub does not serve it`);
    assert.equal(res.headers.get("cross-origin-embedder-policy"), "require-corp", name);
  }
  const url = new URL("https://hub.example/player/sql.wasm");
  assert.match((await playerAsset(new Request(url), url, env)).headers.get("content-type"), /application\/wasm/);
});
