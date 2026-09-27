// A page load must never start a build (K-885).
//
// Studio's desktop test hook asks `autorun` for a request and builds
// whatever comes back. In a tab it borrowed open_app, whose status word
// "asking" was then typed into the box and built on every visit -- at the
// person's expense on a bring-your-own-key account. In a tab the answer is
// nothing, always.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");
const at = src.indexOf("async autorun(");
assert.ok(at > 0, "autorun exists in the bridge");
// The body, to its closing brace, with comments stripped: a phrase in a
// comment must not satisfy or fail this.
let depth = 0, end = -1;
for (let i = src.indexOf("{", at); i < src.length; i++) {
  if (src[i] === "{") depth++;
  if (src[i] === "}" && --depth === 0) { end = i; break; }
}
const body = src.slice(at, end + 1).replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
assert.match(body, /return null;/, "autorun answers nothing in a tab");
assert.doesNotMatch(body, /open_app|COMMANDS\./, "autorun never borrows another command's answer");
console.log("ok  a page load never starts a build");
