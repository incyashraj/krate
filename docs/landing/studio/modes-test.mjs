// Plan mode is a promise: choosing it must never produce an app. Checked
// against the shipped bridge, because a UI-only guard would be walked past
// by the chat's own "just build it" escape hatch.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");

// The guard is the FIRST thing create_app does, before the token check and
// before anything is sent to the build service.
const at = src.indexOf("async create_app(");
assert.ok(at > 0, "create_app exists");
const head = src.slice(at, at + 900);
assert.match(head, /webMode\(\)\s*===\s*"plan"/, "create_app checks the mode");
const guardAt = head.indexOf('webMode() === "plan"');
const builderAt = head.indexOf('builder("/build"');
assert.ok(guardAt > 0 && (builderAt === -1 || guardAt < builderAt),
  "the mode is checked before anything reaches the build service");

// Build is the default: a person who never touches the toggle gets today's
// behaviour, not a surprise.
assert.match(src, /localStorage\.getItem\(MODE_KEY\) === "plan" \? "plan" : "build"/,
  "anything but an explicit plan choice means build");

// And the plan's chosen shape reaches the build service, which is what
// makes a browser build fast rather than cold.
assert.match(src, /shape: starterShape \|\| ""/, "the starter shape is forwarded");

console.log("ok  plan mode cannot build");
console.log("ok  build is the default");
console.log("ok  the plan's shape reaches the build");
