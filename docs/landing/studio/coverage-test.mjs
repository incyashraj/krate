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
// The markup and the stylesheet, for the claims that live in them: a button
// whose two icons both ship in the HTML, and the class that picks between
// them. Asserting those in app.js alone would pass over a missing icon.
const html = readFileSync("studio/ui/index.html", "utf8");
const css = readFileSync("studio/ui/style.css", "utf8");

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
  // To the end of the command, not a fixed window. A 900-character slice
  // broke the moment create_app grew a few lines at the top, and read as a
  // missing guard rather than as a test that had stopped looking far enough.
  const next = bridge.indexOf("\n  async ", at + 10);
  const body = bridge.slice(at, next > 0 ? next : at + 6000);
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

// The button below the box is Send or Stop, whichever the moment calls for.
//
// People reach for that exact spot to stop a running thing now, because
// every chat tool puts stop there. Krate's Stop was a small word inside
// the build card, which is not where anyone looks. So while a build runs
// and the box is empty the arrow becomes a square and the button stops
// the build; type one character and it is a send button again.
assert.match(app, /function syncSendMode\(\)/,
  "one button, two jobs, decided in one place");
const mode = app.slice(app.indexOf("function syncSendMode()"),
  app.indexOf("function syncSendMode()") + 2200);
assert.match(mode, /const stopMode = busyHere\(\) && !typed;/,
  "it is a stop button only while a build runs here and nothing is typed");
// Untrimmed on purpose, and this is the assertion that keeps it that way.
// With trim() a box holding four spaces still showed the square, so
// somebody who typed spaces and aimed at Send killed their own build.
assert.match(mode, /const typed = box\.value\.length > 0;/,
  "and a box holding only spaces counts as typed-into, not as empty");
assert.ok(!/const typed = box\.value\.trim\(\)/.test(mode),
  "never trimmed: trimming makes a visibly-typed box look empty");
// The icons both live in the markup. Swapping innerHTML instead would
// lose whichever icon a later re-render did not put back.
assert.match(html, /class="send-arrow"/, "the arrow ships in the button");
assert.match(html, /class="send-square"/, "and so does the square");
assert.match(css, /\.send\.stopping \.send-square \{ display: block; \}/,
  "the class picks which icon shows");

// Enter is not the same press as the button.
//
// The square is aimed at. Enter on an empty box is a habit, and wiping
// out a running build by reflex is not a thing to build in.
const enter = app.slice(app.indexOf('$("prompt").addEventListener("keydown"'));
assert.match(enter.slice(0, 700), /classList\.contains\("stopping"\)\) return;/,
  "Enter on an empty box during a build does nothing");

// Words typed mid-build: ask, do not guess.
//
// It always queued them, so anyone who had changed their mind watched a
// build they no longer wanted run to the end. Two honest options.
assert.match(html, /id="midSheet"/, "the question has somewhere to appear");
assert.match(html, /id="midStopBtn"/, "stop it and use this instead");
assert.match(html, /id="midWaitBtn"/, "or wait, then do this");
assert.match(app, /function askMidBuild\(text\)/, "and something asks it");
assert.match(app, /askMidBuild\(text\);/,
  "the mid-build path asks instead of queueing silently");

// A replaced build is a redirect, not an ending. Without this the
// transcript picked up a bare "stopped" line under the clearer sentence,
// the Stopped card flashed for a second offering "Resume build" for work
// the person had just told us to throw away, and failedRequest kept
// pointing at the replaced request.
assert.match(app, /state\.replacing = true;/, "a replacement says so");
const fail = app.slice(app.indexOf("function failBuild(why, request)"));
assert.match(fail.slice(0, 2600), /const redirecting = why === "stopped" && state\.replacing;/,
  "and failBuild knows a redirect from an ending");
assert.match(fail.slice(0, 2600), /if \(redirecting\) return;/,
  "so no Stopped card paints over the build that is replacing it");

// The question expires with the build it asks about. If the build
// finished while the sheet sat open, the sheet was asking about
// something that no longer existed.
assert.match(mode, /!sheet\.classList\.contains\("hidden"\) && !busyHere\(\)/,
  "an open mid-build question closes itself when the build ends");

console.log("ok  the send button doubles as stop, and says which it is");
console.log("ok  typing mid-build asks rather than guessing");

// And the box it invites typing into is actually typable.
//
// This is the assertion that matters most in this group. The button said
// "type to send instead" over a composer `beginBuild` had set disabled,
// so a real person could not type a character during a v1 build -- only
// a script setting .value could, which is exactly what the browser tests
// were doing, and why it looked like it worked. The whole mid-build
// question was unreachable for the case it was built for.
const begin = app.slice(app.indexOf("function beginBuild(title, expect)"));
assert.match(begin.slice(0, 1100), /box\.disabled = false;/,
  "the composer stays open during a build");
assert.ok(!/box\.disabled = true;/.test(begin.slice(0, 1100)),
  "beginBuild never locks the box it tells people to type in");

console.log("ok  the composer is open while a build runs");

// The browser's back button goes up one screen, not out of Studio.
//
// Studio swaps screens by class and wrote nothing to history, so back from
// a session did not go up a level -- it LEFT, because the previous entry
// was whatever page the person was on before arriving. On a phone that is
// the edge swipe, which people do constantly and without thinking.
//
// It belongs in the bridge: the desktop shell has no browser back button.
assert.match(bridge, /function keepBrowserBackHonest\(\)/,
  "the browser's back button is handled");
const back = bridge.slice(bridge.indexOf("function keepBrowserBackHonest()"));
assert.match(back.slice(0, 3000), /history\.pushState\(\{ krateView: name \}/,
  "each screen change writes an entry");
assert.match(back.slice(0, 3500), /addEventListener\("popstate"/,
  "and back is listened for");
// Not in the URL. A ?view= would be shareable, which sounds better until
// somebody sends a friend a link to a session only their browser has.
assert.ok(!/location\.search|location\.hash\s*=/.test(back.slice(0, 3500)),
  "the screen rides in history.state, not in a shareable URL");
// Back must not land on the sign-in screen for somebody already signed in.
assert.match(back.slice(0, 3000), /NO_ENTRY = new Set\(\["gate", "onboard"\]\)/,
  "the gate and onboarding are not places back can return into");

console.log("ok  browser back goes up one screen");

// "Run it", in a tab, for somebody who has never installed Krate.
//
// It used to download the .krate and say "Double-click the file on your
// Mac, Windows or Linux". For a person with no Krate, double-clicking does
// NOTHING -- no handler, no window, no error saying why. That is the first
// thing somebody does after waiting minutes for their app, and it
// dead-ended in silence.
//
// A tab cannot find out whether this computer has Krate, so it does not
// guess: it asks once and remembers.
assert.match(html, /id="firstRunSheet"/, "the question has somewhere to appear");
assert.match(html, /id="frHaveBtn"/, "I already have Krate");
assert.match(html, /id="frNeedBtn"/, "I do not have Krate yet");
const open = bridge.slice(bridge.indexOf("async open_app("));
assert.match(open.slice(0, 900), /if \(!hasKrateAlready\(\)\) \{\s*askFirstRun\(app\);/,
  "Run it asks before handing over a file that may not open");
// Asked ONCE. A dialog on every press would be worse than the dead end.
assert.match(bridge, /const HAS_KRATE_KEY = "krate\.has\.player\.v1";/,
  "the answer is remembered");
// Per browser, not per account: it is a fact about the COMPUTER, and the
// same account on a work laptop and a home desktop needs two answers.
assert.match(
  bridge.slice(bridge.indexOf("function hasKrateAlready()")),
  /localStorage\.getItem\(HAS_KRATE_KEY\)/,
  "remembered on this computer, not on the account",
);
// Storage can throw or come back empty. Asking again is survivable;
// assuming they have Krate sends them back to the silent double-click.
//
// Scoped to the catch block itself and stripped of comments. A window that
// merely CONTAINS "return false" passed while the catch said `return true`
// -- the sabotage run caught that, which is the whole reason for one.
{
  const fn = bridge.slice(
    bridge.indexOf("function hasKrateAlready()"),
    bridge.indexOf("function rememberHasKrate"),
  );
  const catchAt = fn.indexOf("catch (e) {");
  assert.ok(catchAt > 0, "hasKrateAlready handles blocked storage");
  const block = fn
    .slice(catchAt, fn.indexOf("}", catchAt))
    .replace(/\/\/[^\n]*/g, "");
  assert.match(block, /return false;/,
    "blocked storage asks again rather than assuming");
  assert.ok(!/return true;/.test(block),
    "and never assumes Krate is there");
}

// The person who does not have it gets their app AND the player, in that
// order, so the file is already waiting when the install finishes.
const wire = bridge.slice(bridge.indexOf("function wireFirstRun()"));
assert.match(wire.slice(0, 2000), /krate\.tech\/open\//,
  "and is sent to the page that already picks the right build for their OS");
// A phone gets no download at all: nothing there can open it, and a file
// that cannot open is worse than a sentence saying so.
assert.match(wire.slice(0, 2000), /thisSystem\(\) !== "phone"/,
  "a phone is told, not handed a file it can never open");
assert.match(bridge, /function thisSystem\(\)/, "which computer this is");

console.log("ok  a first-time person is asked, not dead-ended");

// A file attached in a tab reaches the AI on every path that asks it for
// something, not just some of them.
//
// There are three: the first build, the plan before a build, and a change
// to an app that already exists. A person who attaches a screenshot and
// then presses Build has attached it to the build; if only the planning
// path carried it, the file would be silently dropped for anyone who
// never used Plan -- and nothing would say so.
for (const cmd of ["create_app", "plan_request", "revise_app"]) {
  const at = bridge.indexOf(`async ${cmd}(`);
  assert.ok(at > 0, `${cmd} exists`);
  // To the end of the command, not a fixed window. create_app carries the
  // attachments 55 lines in, and a 1200-character slice missed it and read
  // as a missing feature.
  const next = bridge.indexOf("\n  async ", at + 10);
  const body = bridge.slice(at, next > 0 ? next : at + 6000);
  assert.match(body, /attachments: attachmentsFor\(attachments\)/,
    `${cmd} sends the files the person attached`);
}
assert.match(bridge, /const MAX_ATTACH_BYTES = 10 \* 1024 \* 1024;/,
  "and there is a size the page refuses before the upload starts");

console.log("ok  attachments reach the AI on all three paths");

// A refusal from the shell is an answer, not a failed build.
//
// Every refusal the bridge raises -- Plan mode, a paste that is too long, a
// file that is too big -- came out through `failBuild`, which paints "That
// one didn't come together" over the sentence, offers Try again, and offers
// to report an issue. So a person in Plan mode, a setting THEY chose and
// the browser remembers across visits, met a broken-looking build and an
// invitation to file a bug about their own preference.
//
// The wall already had this shape. Refusals now share it: settle the
// build's furniture, say the words, stay on idle.
const caughtRefusal = app.slice(app.indexOf("} else if (err && err.refusal) {"));
assert.ok(
  app.includes("} else if (err && err.refusal) {"),
  "a refusal is handled before the build-failure card",
);
{
  // Scoped to this branch alone. A fixed window ran past the closing brace
  // into the `} else {` beside it, found that arm's failBuild and called
  // the fix broken -- the branch ends where the next arm begins.
  const end = caughtRefusal.indexOf("} else {");
  assert.ok(end > 0, "the refusal branch is followed by the failure arm");
  const branch = caughtRefusal.slice(0, end);
  assert.match(branch, /show\("idle"\)/, "a refusal leaves the person on idle");
  assert.ok(!/failBuild\(/.test(branch),
    "and never paints the failure card over it");
  assert.ok(!/retryBtn/.test(branch),
    "and offers no Try again for a thing that did not fail");
}
// The refusal must be handled BEFORE the generic failure, or the order
// makes the branch unreachable.
{
  const refusalAt = app.indexOf("} else if (err && err.refusal) {");
  const failAt = app.indexOf("failBuild(plainWords(err), request);");
  assert.ok(refusalAt > 0 && failAt > 0 && refusalAt < failAt,
    "the refusal branch comes before the build-failure card");
}

// Plan mode answers with the switch, not with directions to it.
//
// "Switch to Build in the box below" tells somebody to go and do the thing
// they thought they had just done, on a setting that persists, so they meet
// it on every build until they find the control.
assert.match(bridge, /err\.planMode = true;/,
  "the Plan-mode refusal says which refusal it is");
assert.match(caughtRefusal.slice(0, 1800), /err\.planMode && typeof window\.setWebMode === "function"/,
  "and Studio offers a button that changes the setting");
assert.match(caughtRefusal.slice(0, 1800), /window\.setWebMode\("build"\);\s*\n\s*make\(request\);/,
  "which switches the mode and then builds what they asked for");

console.log("ok  a refusal reads as an answer, and Plan mode offers the switch");

// "For someone new to Krate" tells the truth before the click, not after.
//
// That option makes a gift: one double-clickable file that installs the
// player and then opens the app. The krate binary makes it, on the sender's
// own machine, so a tab cannot -- and the sheet said so only AFTER the
// person had picked the option, picked an operating system, and waited.
// Three clicks to reach "made in Studio on your computer", on a screen
// where nothing had hinted at it.
//
// The link beside it already does this job here, so the option says that
// instead, and the OS buttons that lead nowhere are removed.
assert.match(bridge, /function tellTheTruthAboutTheGift\(\)/,
  "the browser rewrites the gift option");
{
  const gift = bridge.slice(bridge.indexOf("function tellTheTruthAboutTheGift()"));
  assert.match(gift.slice(0, 1800), /btn\.disabled = true;/,
    "it is not a button here, so it does not behave like one");
  assert.match(gift.slice(0, 1800), /os\.remove\(\)/,
    "and the operating-system buttons that lead nowhere are gone");
  assert.match(gift.slice(0, 1800), /share a link instead/i,
    "it names the thing that does work from a tab");
}
// Rewritten from the bridge, not from Studio's markup: the desktop makes
// the gift perfectly well, and changing the markup would lie there instead.
assert.match(html, /id="sendWrapOs"/,
  "the desktop still has its operating-system buttons");

console.log("ok  the gift option is honest in a browser");

// Details reads an app that is on screen, even after a reload.
//
// `jobResult` is this TAB's memory of the build it ran, so a refresh
// empties it. Details then said "There is no app to read yet" about a
// finished app that was still showing, with its permissions printed on the
// done card right behind the sheet. That is the screen where somebody
// decides whether to trust an app, so an error there is the worst possible
// answer to "what is this allowed to do".
//
// The sessions carry the same `asks` and `size`, saved when the build
// finished. This is the K-366 cure applied to the one path that had not
// learned it: an app that is showing is never refused as "not made here".
{
  const info = bridge.slice(bridge.indexOf("async app_info("));
  const end = info.indexOf("\n  /* The failed request");
  const body = info.slice(0, end > 0 ? end : 3000);
  assert.match(body, /localSessions\(\)/,
    "Details falls back to the saved sessions, not just this tab's memory");
  // Matched on the path, so somebody reading an app they made last week is
  // not shown this morning's permissions.
  assert.match(body, /String\(s\.result\.path \|\| ""\) === wanted/,
    "and matches the app it was asked about");
  assert.ok(
    body.indexOf("localSessions()") < body.indexOf('refuse("There is no app to read yet.")'),
    "the refusal is the last resort, after the sessions have been looked at",
  );
}

console.log("ok  Details survives a reload");

// Every element a top-level listener binds to actually exists in the markup.
//
// app.js wires its buttons at the top level, so `$("someBtn").addEventListener`
// on an element that is not there throws during boot -- and everything
// defined AFTER that line never runs. There is no error on screen. The
// person just finds that half of Studio does nothing, with no way to tell
// which half or why.
//
// Measured, by accident: a stale copy of the bridge removed #changeDirBtn,
// app.js died at that line, and the theme module 1,200 lines below it never
// installed. The Appearance buttons looked wired and were dead. It took a
// console read to find, on a screen that showed nothing wrong.
//
// So: every id bound unguarded at the top level must be in index.html.
{
  const bound = [...app.matchAll(/^\$\("([A-Za-z0-9_]+)"\)\.addEventListener/gm)]
    .map((m) => m[1]);
  assert.ok(bound.length > 20, `found ${bound.length} top-level listeners -- the scan broke`);
  const missing = bound.filter((id) => !html.includes(`id="${id}"`));
  assert.deepEqual(
    missing,
    [],
    `these ids are wired at the top level of app.js but are not in `
      + `index.html, so app.js throws while booting and every feature `
      + `defined after that line silently never installs:\n`
      + missing.map((id) => `  - ${id}`).join("\n")
      + `\n\nEither add the element, or bind it with ?. if it is genuinely `
      + `optional.`,
  );
}

console.log(`ok  every top-level listener has an element to bind to`);

// A dropped connection says so, in every wording a browser uses for one.
//
// plainWords matched `network|offline|dns|connect` -- what a desktop engine
// says, and none of what a BROWSER says. Chrome throws "Failed to fetch",
// Firefox "NetworkError when attempting to fetch resource", Safari "Load
// failed". None of those match, so losing wifi mid-build fell through to
// the generic line: "The build failed. Press Details for the engine output"
// -- about a build that was fine, on a screen with no engine output, told
// to somebody whose connection dropped.
{
  const words = app.slice(app.indexOf("function plainWords(err)"));
  // To the function's own closing brace at column 0. Cutting at the next
  // "\nfunction " ended the window early, because plainWords is followed by
  // a comment block before the next top-level function -- and the generic
  // line this test compares against sits past that cut, so the ordering
  // check compared against -1 and failed on correct code.
  const body = words.slice(0, words.indexOf("\n}\n") + 3);
  for (const phrase of ["failed to fetch", "networkerror", "load failed"]) {
    assert.ok(
      body.toLowerCase().includes(phrase),
      `plainWords does not recognise "${phrase}", which is how a browser `
        + `reports a dropped connection`,
    );
  }
  // And it must be recognised before the fall-through, or the branch never
  // runs. Compared against the final RETURN, not the first mention of that
  // sentence: an earlier comment quotes the same words, so matching the
  // text found the comment and failed on correct code.
  const netAt = body.indexOf("failed to fetch");
  const fallThrough = body.lastIndexOf('return "The build failed.');
  assert.ok(netAt > 0, "the connection check is in plainWords");
  assert.ok(fallThrough > 0, "plainWords still has a last-resort line");
  assert.ok(
    netAt < fallThrough,
    "a dropped connection is recognised before the fall-through",
  );
}

console.log("ok  a dropped connection is not blamed on the app");

// Open on a version chip gives THAT version, under a name that says which.
//
// Every chip's Open and Share called openApp/openSendSheet with no
// argument, so both read `state.session.result` -- the LATEST build. After
// three changes, pressing Open on v1 downloaded v3, under the same filename
// as v1 and v2, which the browser silently renames to (1) and (2). A person
// with three copies in their downloads folder had no way to tell which was
// which, and the one they double-clicked was a coin toss.
//
// The session keeps one result and overwrites it each build, so the version
// is only knowable at the moment the chip settles. It captures it there.
{
  const settle = app.slice(app.indexOf("function settleChipOk("));
  const body = settle.slice(0, settle.indexOf("\nfunction settleChipBad"));
  assert.match(body, /const mine = app && app\.result && app\.result\.path/,
    "the chip captures its own version's file as it settles");
  assert.match(body, /openApp\(mine, version\)/, "Open acts on that version");
  assert.match(body, /openSendSheet\(mine, version\)/, "and so does Share");
  // The `app` argument was passed in and never read before this. If it goes
  // unused again the capture is gone and both buttons silently follow the
  // newest build, which is exactly how this shipped.
  assert.ok(body.includes("app.result.path"),
    "the session passed to the chip is actually read");
}
// Every button inside the share sheet acts on the version the sheet was
// opened for, not on whatever is newest when the button is pressed.
assert.match(app, /state\.sharing = \(which && which\.path\) \? which : null;/,
  "the share sheet pins the version it was opened for");
for (const fn of ["async function sendCard()", '$("sendRawBtn").addEventListener']) {
  const at = app.indexOf(fn);
  assert.ok(at > 0, `${fn} exists`);
  assert.match(app.slice(at, at + 400), /state\.sharing \|\| currentApp\(\)/,
    `${fn} shares the pinned version, not the newest`);
}
// And the file says which version it is.
assert.match(bridge, /\$\{stem\} v\$\{version\}\.krate/,
  "an older version downloads under a name that names it");

console.log("ok  a version chip opens its own version");

// A status message does not wear the share row's heading.
//
// showActionError writes into the share row, whose heading says "Here's
// your link. Send it to anyone" and whose Copy button is right beside it.
// So pressing Open put "Downloaded. Double-click the file on your Mac,
// Windows or Linux" under "Here's your link", with Copy offering to copy
// that sentence to somebody. Measured: shareHead read "Here's your link",
// shareLink held the download sentence, and Copy was live.
{
  const fn = app.slice(app.indexOf("function showActionError(err)"));
  const body = fn.slice(0, fn.indexOf("\n}\n"));
  assert.match(body, /head\.textContent = "";/,
    "a status message clears the share heading");
  assert.match(body, /copy\.classList\.add\("hidden"\)/,
    "and hides Copy, because a status is not something to send");
}
// And the paths that put a REAL link there put Copy back, or the fix above
// would silently cost the feature it protects.
{
  const publish = app.indexOf('$("shareHead").textContent = "Here\'s your link. Send it to anyone";');
  assert.ok(publish > 0, "the publish path sets the share heading");
  assert.match(app.slice(publish, publish + 400), /shareCopyBtn"\)\?\.classList\.remove\("hidden"\)/,
    "the publish path restores Copy");
  const shared = app.indexOf('$("shareHead").textContent = "Anyone with this link can open it";');
  assert.ok(shared > 0, "the share path sets its own heading");
  assert.match(app.slice(shared, shared + 400), /shareCopyBtn"\)\?\.classList\.remove\("hidden"\)/,
    "the share path restores Copy");
}

console.log("ok  a status message is not offered as a link to send");
