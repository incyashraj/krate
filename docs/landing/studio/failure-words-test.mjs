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
// K-903: a healthy app with a behaviour complaint gets the change built.
assert.match(app, /return buildNow\(request, files \|\| \[\], true\);/, "diagnosis leads into the change");
console.log("ok  failures a retry cannot fix are named, and a behaviour complaint is built");
