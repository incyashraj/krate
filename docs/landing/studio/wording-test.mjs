// Studio's own copy says "on this computer", which is true of a desktop
// Studio and false in a browser tab: on the web the apps live in the
// account and follow the person to any machine they sign in on. The bridge
// rewrites those sentences.
//
// The rewrite matches the desktop string exactly, so this test exists to
// catch the quiet failure -- somebody edits the wording in studio/ui and
// the bridge's entry stops matching, leaving the desktop sentence on the
// website with nothing reporting it.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const bridge = readFileSync("docs/landing/studio/bridge.js", "utf8");
const ui = readFileSync("studio/ui/index.html", "utf8");

// 1. The table exists and is wired into the web's startup.
assert.match(bridge, /const WEB_WORDING = \[/, "the wording table exists");
assert.match(bridge, /speakWebWording\(\);/, "the rewrite runs on the web");

// 2. Every desktop string the table promises to rewrite is really in the
//    UI. A stale entry rewrites nothing and hides that it does nothing.
const table = bridge.slice(
  bridge.indexOf("const WEB_WORDING = ["),
  bridge.indexOf("function speakWebWording"),
);
const pairs = [...table.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]);
assert.ok(pairs.length >= 8, `expected at least four pairs, found ${pairs.length / 2}`);

for (let i = 0; i < pairs.length; i += 2) {
  const desktop = pairs[i];
  const web = pairs[i + 1];
  assert.ok(
    ui.includes(desktop),
    `the bridge rewrites ${JSON.stringify(desktop)}, but studio/ui/index.html no longer says it`,
  );
  assert.ok(
    !/on this computer/.test(web),
    `the replacement still says "on this computer": ${JSON.stringify(web)}`,
  );
}

// 3. Nothing user-facing is left saying it. The check is scoped to the
//    strings the table covers, so a NEW one shows up here as a count that
//    no longer matches rather than as a pass.
const uiHits = (ui.match(/on this computer/g) || []).length;
const covered = pairs.filter((_, i) => i % 2 === 0)
  .reduce((n, d) => n + (d.includes("on this computer") ? 1 : 0), 0);
// One of the UI's mentions is a source comment explaining the phrasing
// choice, not copy anyone reads.
const comments = (ui.match(/-- "this computer", not "this Mac"/g) || []).length;
assert.equal(
  uiHits - comments,
  covered,
  `studio/ui has ${uiHits - comments} live "on this computer" strings but the bridge covers ${covered}` +
    " -- a new one needs an entry in WEB_WORDING",
);

console.log("ok  the web rewrites Studio's desktop wording");
console.log("ok  every rewrite still matches a real string in the UI");
