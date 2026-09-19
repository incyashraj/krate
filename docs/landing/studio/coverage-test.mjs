// Every command Studio's UI can reach in a tab has a browser answer.
//
// Studio's interface talks to its shell through one function, and the
// bridge is that function's browser implementation. When the UI gains a
// command the bridge does not answer, nothing fails to build and no test
// goes red -- the button simply returns a rejected promise, and whatever
// catch block happens to be around it decides what the person is told.
//
// That has already happened twice. The Shared screen showed "That part of
// Studio needs the app on your computer" over a gallery that works
// perfectly in a tab. Worse, the refusal is not phrased as a refusal by
// the time it reaches some screens: `plainWords` classifies errors on
// provider vocabulary, the refusal matches none of it, and so a person who
// typed "it won't open" was told "The build failed. Press Details for the
// engine output" -- about an app that had built fine, on a sheet with no
// Details button.
//
// So this test reads both files and compares them. A command may be
// missing from the bridge only if it is named here, with the reason.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const app = readFileSync("studio/ui/app.js", "utf8");
const bridge = readFileSync("docs/landing/studio/bridge.js", "utf8");

// What the UI asks for: every invoke("name") in Studio's own JavaScript.
const asked = new Set(
  [...app.matchAll(/\binvoke\(\s*"([a-z_0-9]+)"/g)].map((m) => m[1]),
);
assert.ok(asked.size > 30, `found ${asked.size} commands in app.js -- the scan broke`);

// What the browser answers: the keys of the bridge's COMMANDS table.
const table = bridge.slice(bridge.indexOf("const COMMANDS = {"));
const answered = new Set(
  [...table.matchAll(/^  (?:async )?([a-z_0-9]+)\s*[:(]/gm)].map((m) => m[1]),
);
assert.ok(answered.size > 40, `found ${answered.size} bridge commands -- the scan broke`);

// Commands with no browser answer, each with the reason it is acceptable.
//
// Every entry here is a thing a browser genuinely cannot do, NOT a thing
// nobody got round to. A command that a tab could serve does not belong on
// this list; it belongs in the bridge.
const DESKTOP_ONLY = {
  // Both read the local build workspace -- the agent's transcript, the
  // generated lib.rs, the Cargo.toml. A browser build happens on the build
  // service and leaves nothing on the person's machine to collect.
  report_collect: "reads the local build workspace, which a tab does not have",
  report_send: "sends what report_collect gathered; unreachable without it",
  // Runs the .krate through the local engine to see what it does. A tab
  // has no engine and no file to run.
  diagnose_app: "runs the app through the local engine",
  // The AI is ours on the web and always ready, so there is no per-agent
  // sign-in and no installed-tool state to report.
  sign_in_agent: "signs into a locally installed AI; the web AI is ours",
  terminal_status: "reports on a local terminal install",
  // Tags the local session directory the desktop keeps beside the app.
  agent_session_tag: "tags a local session directory",
  // Pulls sessions into the desktop's own store; the browser reads the
  // hub directly in sessions_list.
  sessions_pull: "pulls into the desktop store; the web reads the hub live",
};

const missing = [...asked].filter((c) => !answered.has(c)).sort();
const unexplained = missing.filter((c) => !(c in DESKTOP_ONLY));
assert.deepEqual(
  unexplained,
  [],
  `these commands are reachable in a tab with no browser answer:\n` +
    unexplained.map((c) => `  - ${c}`).join("\n") +
    `\n\nEither add them to COMMANDS in bridge.js, or, if a browser truly ` +
    `cannot do them, add them to DESKTOP_ONLY in this test with the reason.`,
);

// The list must not rot the other way either: an entry that has since been
// bridged is a stale excuse, and the next person reads it as a rule.
const stale = Object.keys(DESKTOP_ONLY).filter((c) => answered.has(c));
assert.deepEqual(stale, [], `bridged now, so remove from DESKTOP_ONLY: ${stale}`);

// A refusal must be recognisable AS a refusal by the time Studio words it.
// Without this flag `plainWords` falls through to its build-failure line.
assert.match(
  bridge,
  /function refuse\(message\)[\s\S]{0,400}?err\.refusal = true/,
  "refuse() flags the error so plainWords does not call it a failed build",
);
assert.match(
  app,
  /if \(err && err\.refusal\) return raw;/,
  "plainWords passes a refusal through in its own words",
);

// The Details sheet divides the size it is given by 1024. The build
// service sends "85 KB". Handing that over produced "NaN KB".
assert.match(bridge, /function bytesOfPretty/, "the pretty size is parsed back to bytes");
assert.match(
  bridge.slice(bridge.indexOf("async app_info(")),
  /shape\(r\.asks \|\| \[\], bytesOfPretty\(r\.size\)\)/,
  "app_info reports bytes, not the pretty string",
);

// And it can read an app somebody ELSE published. The gallery's detail
// page is where a stranger decides whether to download something, and
// what it asks for is the whole basis of that decision. Reading only
// `bridge.jobResult` -- the app THIS tab built -- left that page saying
// "Could not read this app right now" under "WHAT IT IS ALLOWED TO DO".
assert.match(
  bridge.slice(bridge.indexOf("async app_info(")),
  /hub\(`\/meta\/\$\{id\[1\]\}`\)/,
  "a published app's permissions come from the hub",
);

console.log(`ok  all ${asked.size} UI commands are answered or explained`);
console.log(`ok  ${Object.keys(DESKTOP_ONLY).length} desktop-only commands, each with a reason`);
console.log("ok  a refusal reaches the person as a refusal");
console.log("ok  the Details sheet gets a number it can divide");

// The allowance wall is an ANSWER, not a failure.
//
// The builder answers 402 with {wall:true, download:true} when somebody has
// used the app Krate funded. The bridge re-throws that with the flags
// attached; Studio must branch on them BEFORE `failBuild`. Without the
// branch the person was shown "That one didn't come together. The build
// failed. Press Details for the engine output" and a "Try again" button --
// about a build that never ran, for a limit the card never named.
assert.match(
  bridge,
  /wall\.wall = true;[\s\S]{0,200}?wall\.download = Boolean\(parsed\.download\)/,
  "the bridge flags a wall so Studio can tell it from a failure",
);
const caught = app.slice(app.indexOf("  } catch (err) {", app.indexOf("finishBuild(result);")));
const wallAt = caught.indexOf("err && err.wall");
const failAt = caught.indexOf("failBuild(plainWords(err)");
assert.ok(wallAt > 0, "Studio branches on the wall flag");
assert.ok(wallAt < failAt, "the wall is handled BEFORE the build-failure card");
// And it must clear the build's own furniture, or the live chip sits at
// "building" for ever beside a message saying no build is happening.
const wallBlock = caught.slice(wallAt, failAt);
assert.match(wallBlock, /state\.buildChip\.remove\(\)/, "the wall removes the live build chip");
assert.match(wallBlock, /unlockComposer\(/, "the wall unlocks the composer");

console.log("ok  the allowance wall reads as an answer, not a failed build");

/* ---- the three waits a person actually feels ---------------------------- */

// 1. Back must not wait on the network.
//
// The handler awaited `persist()`, which on the web is a hub POST, before
// it navigated -- so Back took as long as the round trip (measured: 1,512ms
// against a hub lagging 1.5s, against 5ms once the await was dropped). The
// save still happens; `session_save` writes local storage synchronously
// before the request goes out, so nothing can be lost by leaving.
const backHandler = app.slice(app.indexOf('$("backBtn").addEventListener'), app.indexOf('$("backBtn").addEventListener') + 700);
assert.ok(backHandler.length > 100, "the back button handler was found");
assert.doesNotMatch(backHandler, /await\s+persist\(\)/,
  "Back must not await the hub save; it makes the button take a round trip");
assert.match(backHandler, /persist\(\);/, "but it must still save");
assert.match(
  bridge.slice(bridge.indexOf("async session_save(")),
  /saveLocalSessions\(list\);[\s\S]{0,200}?await hub\("\/sessions"/,
  "session_save writes locally BEFORE the hub, which is what makes not awaiting safe",
);

// 2. Home must paint before the session list arrives.
assert.match(bridge, /async sessions_local\(\)/, "the browser has an instant session read");
assert.match(app, /invoke\("sessions_local"\)/, "and Home uses it before the networked one");

// 3. The sign-in screen must not be shown to somebody already signed in.
//
// `viewGate` is the only view without `hidden` in the HTML, and the scripts
// are at the END of the body, so the browser paints the sign-in page and
// keeps it up until a 290 KB app.js has loaded and booted. Measured with
// app.js arriving 1.8s late: visible for 1,801ms before this, 21ms after.
//
// The lift must NOT hang off window.load, which fires before app.js has
// booted and was the first wrong answer here.
const gateBlock = bridge.slice(bridge.indexOf("const GATE_CSS"), bridge.indexOf("const GATE_CSS") + 2600);
assert.match(gateBlock, /#viewGate \{ visibility: hidden; \}/, "the gate is hidden while boot decides");
assert.match(gateBlock, /if \(bridge\.token\)/, "and only for somebody already signed in");
assert.match(gateBlock, /MutationObserver/, "the rule lifts when Studio marks the gate hidden");
assert.doesNotMatch(gateBlock, /addEventListener\("load"/,
  "window.load fires before app.js boots, so it cannot be the signal");
assert.match(gateBlock, /setTimeout\(lift, \d+\)/,
  "and a fallback lifts it anyway, or a failed boot leaves a blank page");

// No dashes as punctuation in anything a person reads.
//
// " -- " renders as a long dash in the browser and the founder does not
// want it in the product's own voice. Comments and CLI flags are exempt:
// this checks quoted strings only, outside comment lines.
for (const [name, src] of [["app.js", app], ["bridge.js", bridge]]) {
  const offenders = [];
  src.split("\n").forEach((line, i) => {
    if (/^\s*(\/\/|\*|\/\*)/.test(line)) return;
    for (const m of line.match(/"[^"]* -- [^"]*"/g) || []) offenders.push(`${name}:${i + 1} ${m}`);
  });
  assert.deepEqual(offenders, [], `dashes left in user-facing text:\n${offenders.join("\n")}`);
}

console.log("ok  Back does not wait on the network");
console.log("ok  Home paints before the hub answers");
console.log("ok  the sign-in screen is never shown to somebody signed in");
console.log("ok  no dashes in user-facing text");

// The done card's caption must not cover the app it is captioning.
//
// The strip is drawn OVER the bottom of the preview, which is what makes
// the card one screenshot-able object. It also means the bottom of the
// app's own window is behind it, and the bottom of a window is where its
// buttons are. Measured on a pomodoro timer at 1280x820: the strip
// covered the bottom 63px of a 344px picture at 88% opacity, cutting its
// Start button in half.
//
// The fix is room for the strip on the rule that SIZES the image. A
// separate later rule does not work: this sheet opens with
// `* { margin: 0 }`, and a universal reset beats a compound selector, so
// an override silently did nothing.
const css = readFileSync("studio/ui/style.css", "utf8");
const shotImg = css.slice(css.indexOf(".shot-stage img {"));
const rule = shotImg.slice(0, shotImg.indexOf("}"));
assert.match(rule, /margin-bottom:\s*\d+px/,
  "the preview reserves room for the caption strip drawn over it");
const reserved = Number((rule.match(/margin-bottom:\s*(\d+)px/) || [])[1]);
assert.ok(reserved >= 63,
  `the strip wraps to two lines and measures 63px; ${reserved}px still covers the app`);

console.log("ok  the done card's caption does not cover the app");

// Publishing has its own words for what went wrong.
//
// `plainWords` classifies BUILD failures on provider vocabulary --
// compilers, agents, quotas, toolchains. Publishing shares none of it, so
// every publish error fell through to that function's last resort: "The
// build failed. Press Details for the engine output", shown on a sheet
// with no Details button, about a build that was not running. The app was
// already made; only the upload failed.
assert.match(app, /function publishWords\(err\)/,
  "publishing classifies its own failures");
assert.doesNotMatch(
  app,
  /\$\("pubNote"\)\.textContent = plainWords\(/,
  "no publish path words its failure as a failed build",
);
assert.match(
  app,
  /function publishWords[\s\S]{0,900}?if \(err && err\.refusal\) return/,
  "and a refusal still passes through in its own words",
);

console.log("ok  a failed publish does not claim the build failed");

/* ---- what people actually do ------------------------------------------- */

// A long paste is refused here, not after a round trip.
//
// The build service works from 2,000 characters and refuses more in three
// places. Nothing in the page stopped it, so 50,008 characters travelled
// all the way there to come back as a bare failure. Measured: 0 requests
// sent now, and the person reads why immediately.
assert.match(bridge, /const MAX_REQUEST_CHARS = 2000;/,
  "the page knows the build service's limit");
for (const [cmd, word] of [["create_app", "request"], ["plan_request", "request"], ["revise_app", "change"]]) {
  const at = bridge.indexOf(`async ${cmd}(`);
  assert.ok(at > 0, `${cmd} exists`);
  const body = bridge.slice(at, at + 900);
  assert.ok(
    body.includes(`tooLong(${word}, "${word}")`),
    `${cmd} refuses a ${word} longer than the service will take`,
  );
}

// Two tabs, one account. Somebody builds in one and switches back to the
// other, which was showing the world as it was before: no new app, and a
// composer still offering to make the first one. With one free app that
// reads as the app having vanished. The wall itself was never at risk --
// it is counted on the hub against the account and the device.
assert.match(bridge, /window\.addEventListener\("storage"/,
  "a tab notices work done in another tab");
assert.match(
  bridge.slice(bridge.indexOf('addEventListener("storage"')),
  /event\.key !== "krate-sessions"/,
  "and only repaints for the sessions it shares",
);

console.log("ok  a long paste is refused before it is sent");
console.log("ok  two tabs stay in step");

// A build somebody STOPPED is not a build that failed.
//
// Stopping showed the failure card's reporting offers -- "Report an
// issue" and "we read what failed, and it makes Krate better" -- which
// asks a person to report their own decision as a defect. The timeline
// chip also wrote "v1 failed", and the timeline is where that record
// persists: scrolling back later should not find your own Stop recorded
// as a failure.
assert.match(app, /function showFailReporting\(on\)/,
  "the failure card can hide its reporting offers");
assert.match(
  app,
  /unlockComposer\("Changed your mind\?[^"]*"\);\s*(\/\/[^\n]*\n\s*)*showFailReporting\(false\)/,
  "a deliberate stop hides them",
);
assert.match(app, /const what = stopped \? "stopped" : "failed";/,
  "and the timeline chip says which it was");

// Try again on a failed build goes straight back to the build.
//
// It called `make`, which runs the conversation gate, so a retry showed
// the same plan again and asked the person to press "Build it" a second
// time for a request they had already approved. Measured after the fix:
// 0 plan calls, 1 build call.
const retry = app.slice(app.indexOf('$("retryBtn").addEventListener'));
assert.match(retry.slice(0, 1200), /\(The agreed plan:/,
  "a retry recognises a request whose plan was already agreed");
assert.match(retry.slice(0, 1200), /buildNow\(again/,
  "and rebuilds it rather than re-planning it");

console.log("ok  a stop is not recorded as a failure");
console.log("ok  Try again rebuilds instead of re-planning");
