/* A link to an older version says a newer one exists (IC-976,
 * cloud/worker/src/receive.js newerVersionLine).
 *
 * Someone holding the link to v1 of an app that is now at v3 lands on a
 * page about v1. One quiet line says so and links the app's own link,
 * which opens v3. A current build, and a build that is no version of
 * anything, say nothing.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/receive-versions.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call } from "./org-tools.mjs";
import { newerVersionLine } from "../src/receive.js";

const here = dirname(fileURLToPath(import.meta.url));
const build = (v) => new Uint8Array(readFileSync(join(here, "fixtures", `notes-${v}.krate`)));
const hubEnv = () => {
  const env = orgEnv({ PUBLIC_BASE: "https://hub.example" });
  env.USAGE = { writeDataPoint() {} };
  return env;
};
const publish = (env, who, v, headers = {}) =>
  call(env, who, "POST", "/publish", { bytes: build(v), headers: { "x-krate-name": "Notes", ...headers } });
const PHONE = {
  "user-agent": "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
  accept: "text/html,application/xhtml+xml",
};

test("an older version's page says a newer one is current, and links the app", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  await publish(env, "alice", "0.2.0");
  const v3 = await publish(env, "alice", "0.3.0");
  assert.equal(v3.body.version, 3, JSON.stringify(v3.body));

  const said = await newerVersionLine(env, v1.body.id, "https://hub.example/");
  assert.match(said, /This is v1\. There is a newer version, v3\./);
  assert.ok(said.includes(`href="${v1.body.url}"`), `links the app's own link: ${said}`);
  assert.match(said, />Open v3</);
  assert.equal(await newerVersionLine(env, v3.body.id, "https://hub.example"), "", "the current version says nothing");

  // Made current again by its owner, v1 is no longer older than anything.
  const line = v1.body.url.split("/a/")[1];
  await call(env, "alice", "POST", `/lines/${line}/current`, { json: { version: 1 } });
  assert.equal(await newerVersionLine(env, v1.body.id, "https://hub.example"), "");
});

test("a build that is no version of an app says nothing", async () => {
  const env = hubEnv();
  const quiet = await publish(env, "alice", "0.1.0", { "x-krate-unlisted": "1", "x-krate-quiet": "1" });
  assert.equal(await newerVersionLine(env, quiet.body.id, "https://hub.example"), "");
  assert.equal(await newerVersionLine(env, "0".repeat(64), "https://hub.example"), "");
});

// The phone's landing page is drawn in index.js (mobileLanding), which this
// change does not touch: it carries the line once mobileLanding prints
// `await newerVersionLine(env, hash, base)` under the app's name.
test("a phone landing on an older version's link sees the line", async () => {
  const env = hubEnv();
  const v1 = await publish(env, "alice", "0.1.0");
  await publish(env, "alice", "0.2.0");
  const page = await call(env, null, "GET", `/a/${v1.body.id}`, { headers: PHONE });
  assert.equal(page.status, 200);
  assert.match(page.headers.get("content-type") || "", /text\/html/, "the phone gets its page");
  assert.ok(/There is a newer version, v2/.test(String(page.body)), "the page says v2 is out");
});
