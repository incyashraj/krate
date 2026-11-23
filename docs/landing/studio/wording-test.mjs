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
// Each table is read to its OWN closing "];" -- slicing to the next
// function would swallow the tables that follow it, and their strings are
// not in studio/ui at all (they are set by app.js at runtime).
function tableOf(name) {
  const at = bridge.indexOf(`const ${name} = [`);
  assert.ok(at > 0, `${name} exists`);
  const end = bridge.indexOf("\n];", at);
  assert.ok(end > at, `${name} is closed`);
  const body = bridge.slice(at, end);
  return [...body.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]);
}

const pairs = tableOf("WEB_WORDING");
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

// 4. The touch rewrites. These shorten the composer's hint and
//    placeholder, which on a phone were cut mid-sentence and wrapped to a
//    stranded word. Their originals live in app.js, not the HTML, because
//    Studio writes them at runtime -- so that is where to check they are
//    still real.
const appJs = readFileSync("studio/ui/app.js", "utf8");

for (const name of ["TOUCH_WORDING", "TOUCH_PLACEHOLDERS"]) {
  const t = tableOf(name);
  assert.ok(t.length >= 2 && t.length % 2 === 0, `${name} holds whole pairs`);
  for (let i = 0; i < t.length; i += 2) {
    const wide = t[i], touch = t[i + 1];
    assert.ok(
      appJs.includes(wide) || ui.includes(wide),
      `${name} rewrites ${JSON.stringify(wide)}, which Studio no longer says`,
    );
    assert.ok(
      touch.length < wide.length,
      `${name}: the touch text is not shorter -- ${JSON.stringify(touch)} vs ${JSON.stringify(wide)}`,
    );
  }
}

// And it must be scoped to a phone. A touchscreen laptop reports
// `pointer: coarse` at 1440px, where the long sentence fits fine.
const fn = bridge.slice(
  bridge.indexOf("function speakTouchWording"),
  bridge.indexOf("function speakWeb("),
);
assert.match(fn, /pointer: coarse/, "the touch rewrite checks for touch");
assert.match(fn, /max-width: 860px/, "the touch rewrite is scoped to a phone too");

// A built app is announced as built (founder rule, 2026-11-23). The
// engine's request verdict stays in our records; no string a person can
// see says something "may be missing" or "is not what you asked for".
// Comments may name the old words (they explain why they are gone), so
// only code lines are read.
{
  const codeOf = (path) => readFileSync(path, "utf8").split("\n")
    .filter((l) => !/^\s*(\/\/|\*|\/\*)/.test(l)).join("\n");
  for (const path of ["studio/ui/app.js", "studio/ui/redesign.js", "docs/landing/studio/bridge.js", "cloud/builder/src/server.js", "studio/ui/index.html"]) {
    const code = codeOf(path);
    for (const words of [/may be missing/i, /not what you asked for/i, /Worth checking/]) {
      assert.ok(!words.test(code), `${path} shows a person ${words}`);
    }
  }
  // The builder hands the page no verdict for a built app.
  const server = readFileSync("cloud/builder/src/server.js", "utf8");
  assert.match(server, /verdict_detail: job\.result\.verdict === "ported" \? job\.result\.verdict_detail \|\| null : null/,
    "the status hands the page a port's note and nothing else");
}

// Phones (2026-11-23, an iPhone in Safari).
{
  const css = readFileSync("studio/ui/redesign.css", "utf8");
  // The App side's bar must contain its own drag layer (style.css
  // .titlebar::before, absolute inset 0): a static bar let it cover the
  // whole screen, so Chat, Run, Share and Back could not be pressed.
  const appBar = /#viewSession\[data-mob="app"\] \.panel-bar \{\s*position: (\w+)/.exec(css);
  assert.ok(appBar, "the phone App side styles its bar");
  assert.notEqual(appBar[1], "static", "the phone App bar is never static");
  // Each view is fixed to what the screen shows, moved by the visual
  // viewport, so the composer sits above Safari's bar and the keyboard.
  assert.match(css, /\.view \{\s*position: fixed; left: 0; right: 0;\s*top: var\(--vv-top, 0px\); bottom: var\(--vv-bottom, 0px\);/,
    "a phone's views follow the visual viewport");
  const rjs = readFileSync("studio/ui/redesign.js", "utf8");
  assert.match(rjs, /visualViewport\.addEventListener\("resize"/, "the views follow the keyboard");
  assert.match(rjs, /function alignAvatar\(/, "the mark is set on its first line by measurement");
  // Run on a phone: no blank tab opened ahead of the work.
  const run = bridge.slice(bridge.indexOf("async function runOnPhone"), bridge.indexOf("function backFromRun"));
  assert.ok(run.length > 100, "runOnPhone exists");
  assert.ok(!/window\.open/.test(run), "a phone's Run never opens a tab it cannot fill");
  assert.match(run, /location\.assign\(run\)/, "a phone's Run goes to the Player in this tab");
  assert.match(bridge, /if \(thisSystem\(\) === "phone"\) return runOnPhone\(app\);/, "open_app sends a phone there");
}

console.log("ok  a built app is shown as built: no verdict words reach the screen");
console.log("ok  phone layout: App bar contains its drag layer, views follow the visual viewport");
console.log("ok  Run on a phone goes to the Player in the same tab");
console.log("ok  the web rewrites Studio's desktop wording");
console.log("ok  every rewrite still matches a real string in the UI");
console.log("ok  the touch rewrites shorten real strings, on phones only");
