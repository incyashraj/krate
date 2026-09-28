// In a tab, bridge.js and app.js are two classic scripts in one page, so
// they share one global scope. A `const` or `let` declared in both throws
// when the second loads -- and that second script is all of Studio, so the
// page was a blank tab (caught in the browser: MODE_KEY, declared by both).
// A `function` declared in both is worse: no error, the later one silently
// replaces the earlier. So no top-level name may be declared by both.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const top = (file) => {
  const src = readFileSync(file, "utf8");
  const names = new Set();
  const re = /^(?:async\s+)?(?:function\*?\s+|const\s+|let\s+|var\s+|class\s+)([A-Za-z_$][\w$]*)/gm;
  for (const m of src.matchAll(re)) names.add(m[1]);
  return names;
};
const app = top("studio/ui/app.js");
const bridge = top("docs/landing/studio/bridge.js");
assert.ok(app.size > 200 && bridge.size > 50, "both scripts were read");
const both = [...app].filter((n) => bridge.has(n)).sort();
assert.deepEqual(both, [], `declared at the top level of both app.js and bridge.js: ${both.join(", ")}`);
console.log(`ok  app.js (${app.size} names) and bridge.js (${bridge.size}) share no top-level name`);
