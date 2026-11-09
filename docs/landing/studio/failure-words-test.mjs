// What a failed build tells the person, run against real engine output
// (K-892). The classifier is executed, not pattern-matched: the functions
// are lifted out of the shipped app.js and called.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const app = readFileSync("studio/ui/app.js", "utf8");
function lift(start, endMarker) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `${start} is in app.js`);
  const end = app.indexOf(endMarker, at);
  assert.ok(end > at, `the end of ${start} is found`);
  return app.slice(at, end + endMarker.length);
}
const src = [
  lift("const KRATE_OWN_PROSE", "];"),
  lift("function providerWords(", "\n}\n"),
  lift("function engineVerdict(", "\n}\n"),
  lift("function plainWords(", "\n}\n"),
].join("\n");
const plainWords = new Function(`${src}; return plainWords;`)();

const says = (stderr) => plainWords(new Error(stderr));

// The agent's repro: a compile error in a Studio workspace (every path
// contains ".krate"), rustc's "not found in this scope", the engine's
// final line. It used to say "Krate's engine is missing ... Reinstall".
const compile = [
  "   Compiling checklist v0.1.0 (/Users/x/.krate/studio/builds/s-1/checklist)",
  "error[E0425]: cannot find value `undefined_thing` in this scope",
  " --> src/lib.rs:3:18",
  "  |",
  "3 | fn zz() -> u32 { undefined_thing + 1 }",
  "  |                  ^^^^^^^^^^^^^^^ not found in this scope",
  "error: could not compile `checklist` (lib) due to 1 previous error",
  "error: the app could not be built",
].join("\n");
assert.match(says(compile), /did not compile/, says(compile));
assert.doesNotMatch(says(compile), /Reinstall|build tools/);

// A log that mentions cargo and Cargo.lock is not a missing toolchain.
const cargoLog = "Updating crates.io index\n  Locking 3 packages to latest compatible versions\n  (Cargo.lock)\nerror: the app could not be built";
assert.doesNotMatch(says(cargoLog), /build tools/, says(cargoLog));

// K-756: Krate's own advice under a compile failure ("Fix: ... The build uses
// rustup's toolchain and the wasm32-wasip1 target; run `krate doctor` if the
// target or toolchain looks wrong.", crates/cli build_fix) is not the error.
// Matching it told the founder "The build tools aren't set up yet" over 7
// compile errors. Each shape check-app's failure reaches Studio in: the Fix
// inline, after "FAILED at build", and as its own block.
{
  const errs = [
    "   Compiling sketch-painter v0.1.0 (/Users/y/.krate/studio/builds/s-9/sketch-painter)",
    "error[E0599]: no method named `stroke_path` found for struct `Canvas` in the current scope",
    "  --> src/lib.rs:88:14",
    "error[E0308]: mismatched types",
    "  --> src/lib.rs:120:20",
    "error: could not compile `sketch-painter` (lib) due to 7 previous errors",
  ].join("\n");
  const advice = "Fix the compiler errors above. The build uses rustup's toolchain and the wasm32-wasip1 target; run `krate doctor` if the target or toolchain looks wrong.";
  for (const text of [
    `${errs}\n\nFix: ${advice}`,
    `build failed: ${errs}\n\nFix: ${advice}`,
    `FAILED at build\n\n${errs}\n\nFix:\n${advice}`,
  ]) {
    assert.doesNotMatch(says(text), /build tools/, says(text));
    assert.match(says(text), /did not compile/, says(text));
  }
  // The shapes that really mean a missing toolchain still say so.
  assert.match(says("error: cargo-component build failed: the `wasm32-wasip1` target looks missing. Install it with `rustup target add wasm32-wasip1`."), /build tools/);
  assert.match(says("error[E0463]: can't find crate for `core`\n  = note: the `wasm32-wasip1` target may not be installed\nerror: the app could not be built"), /build tools/);
  console.log("ok  a compile error under Krate's toolchain advice is a compile error (K-756)");
}

// A port that stopped says what stopped it (IC-120, K-1237): exit 8 and a
// last line "Blocked on <requirement>: <what>. Workspace: <path>". It used
// to read "The build failed. Press Details", hiding a sentence the engine
// had already got right.
const portBlocked = (need, what) =>
  `This port failed: different behavior.\nerror: ${what}\nBlocked on ${need}: ${what}. Workspace: /Users/x/.krate/ports/inv-1`;
const wrong = says(portBlocked("primary-task", "case `invoice-total` printed \"lines=3\" where the original printed \"total=100\""));
assert.match(wrong, /did not do what your original does: case `invoice-total` printed "lines=3"/, wrong);
assert.doesNotMatch(wrong, /build failed|Workspace:/, wrong);
assert.match(says(portBlocked("agent", "the author command exited with code 3")), /AI tool stopped before the port/);
assert.doesNotMatch(says(portBlocked("agent", "the author command exited with code 3")), /code 3/);
assert.match(says(portBlocked("build", "the candidate did not compile after 2 repair attempt(s)")), /would not build/);
assert.match(says(portBlocked("source-unchanged", "the original source changed during the port")), /project changed while/);
assert.match(says(portBlocked("wall", "the permission wall refused it")), /^The port stopped: the permission wall refused it\./);

// What must still be recognised.
assert.match(says("error: could not run the Krate engine at /x"), /engine is missing/);
assert.match(says("...lots of log...\nerror: Failed to authenticate: OAuth session expired and could not be refreshed"),
  /not signed in/);
assert.match(says("error: the wasm32-wasip1 target is not installed; run rustup target add wasm32-wasip1"),
  /build tools/);
assert.match(says("error: claude hit its usage limit (429 Too Many Requests)"), /usage limit/);
// A log line that says "logged" (a build log does) is not a sign-in failure.
assert.doesNotMatch(says("wrote 3 files, logged the build to trace.jsonl\nerror: the app could not be built"),
  /not signed in/);
console.log("ok  a compile error is a compile error, not a broken install");
console.log("ok  sign-in, toolchain, quota and a missing engine are still recognised");
assert.match(says("error: the `claude` command is not installed, so Krate cannot use claude to write your app."),
  /No AI is connected/);
console.log("ok  a missing AI and a missing toolchain are told apart");

// K-897: text is cut by characters, never through the middle of an emoji.
{
  const clipSrc = app.slice(app.indexOf("function clip("), app.indexOf("\n}\n", app.indexOf("function clip(")) + 3);
  const clip = new Function(`${clipSrc}; return clip;`)();
  const request = "I want a habit tracker for my morning routine with streaks 🔥 and badges";
  const title = clip(request, 60);
  assert.doesNotThrow(() => JSON.parse(JSON.stringify(title)));
  assert.ok(!/[\uD800-\uDBFF]$/.test(title), "no lone high surrogate at the end");
  assert.ok(title.endsWith("🔥"), `the emoji is kept whole: ${title}`);
  assert.doesNotMatch(app, /title: firstRequest\.slice\(0, 60\)/, "the session title is not cut by UTF-16 units");
  console.log("ok  a title is cut by characters, never through an emoji");
}

// K-898: a malformed session is read as data, not trusted.
{
  const at = app.indexOf("function normalSession(");
  const src = app.slice(at, app.indexOf("\n}\n", at) + 3);
  const normalSession = new Function(`${src}; return normalSession;`)();
  const s = normalSession({ id: "s1", messages: [null, 3, { who: "YOU", body: "hi" }, { body: 5 }] });
  assert.equal(s.messages.length, 2, "null and non-objects are dropped");
  assert.equal(s.messages[0].who, "YOU");
  assert.equal(s.messages[1].who, "KRATE");
  assert.equal(s.messages[1].body, "5");
  assert.equal(normalSession(null), null);
  assert.deepEqual(normalSession({ id: "s2" }).messages, []);
  console.log("ok  one malformed session cannot blank Home");
}
// K-904: failures no retry can fix say what they are.
assert.match(says("error: this is not a Krate app, or the file is damaged"), /not a Krate app/);
assert.match(says("error: attached file /tmp/a.csv does not exist"), /no longer there/);
assert.match(says("error: /x/big.csv is 28.6 MB, over the 10 MB attachment limit."), /10 MB/);
assert.match(says("error: could not write /Volumes/ro/app.krate: Read-only file system (os error 30)"), /cannot save into that folder/);
assert.match(says("error: No space left on device (os error 28)"), /disk is full/);
// K-1243: check-app names the kind of failure (40c00c297), and `krate
// create` leads its error with the kind's sentence. A sandbox refusing the
// build cache says "Permission denied" too, and was told to pick another
// folder in Settings. Real shapes, from failed_check_message and check-app.
{
  const cacheRefused = "error: This computer stopped Krate from building the app. That is not a fault in the app's code, so Krate did not ask the AI to change it.\n\nbuild failed: error: failed to open /Users/x/.cargo/registry/cache: Operation not permitted (os error 1)\nPermission denied (os error 13)\n\nPut that right on this computer, then run the command again: the app's code is kept, and nothing in it needs to change. The agent's transcript is at /x/t.txt.";
  const env = says(cacheRefused);
  assert.match(env, /stopped Krate from building|could not build the app just now/, env);
  assert.doesNotMatch(env, /cannot save into that folder|Settings/, env);
  const failed = (lead) => `error: ${lead} The agent finished, but \`check-app\` does not pass yet:\n\nrun failed: the app exited with 3\n\nThe agent's transcript is at /x/t.txt.`;
  assert.match(says(failed("The app the AI wrote builds and opens, but it did not work when Krate tried it.")), /did not work when Krate tried it/);
  assert.match(says(failed("The app the AI wrote builds, but it does not ask for what it uses.")), /does not ask permission for what it uses/);
  assert.match(says(failed("The app the AI wrote builds, but it reaches for parts of the computer Krate does not give apps.")), /parts of the computer Krate does not give apps/);
  assert.match(says(failed("The app the AI wrote builds, but it ran too long when Krate tried it.")), /ran too long/);
  assert.match(says(failed("The app the AI wrote does not build yet.")), /did not compile/);
  // check-app's own output: --json's "kind", and the text headline.
  assert.match(says('{"ok":false,"stage":"build","kind":"environment","detail":"Permission denied (os error 13)","fix":""}'), /stopped Krate from building|could not build the app just now/);
  assert.match(says("FAILED at run\nThe code compiles and the app starts; it fails when Krate uses it the way a person would.\n\nexit 3"), /did not work when Krate tried it/);
  // A real folder that cannot be written is still that.
  assert.match(says("error: could not write /Volumes/ro/app.krate: Permission denied (os error 13)"), /cannot save into that folder/);
  console.log("ok  each kind of failed check is worded as itself, and a sandbox is not a folder (K-1243)");
}
// K-903: a healthy app with a behaviour complaint gets the change built.
assert.match(app, /return buildNow\(request, files \|\| \[\], true\);/, "diagnosis leads into the change");
console.log("ok  failures a retry cannot fix are named, and a behaviour complaint is built");
