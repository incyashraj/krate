// A failed build's words as Claude reads them in a chat (2026-11-20 live
// test): the first app built through the connector failed on `.floor()` in a
// no_std app, the answer opened with cargo's progress lines, and its fix said
// "run `krate doctor`" to someone with no Krate on their machine.
import { test } from "node:test";
import assert from "node:assert/strict";
import { compilerWords, remoteFix } from "../src/mcp.js";

// What the builder answered for build 3e931cf771e69819cf69248baa6bd3f1.
const LIVE = `cargo-component build failed with exit status: 101

stdout:

stderr:
Generating bindings for habit-tracker (src/bindings.rs)
    Updating crates.io index
     Locking 9 packages to latest compatible versions
 Downloading crates ...
  Downloaded cfg-if v1.0.5
  Downloaded libm v0.2.16
   Compiling libm v0.2.16
   Compiling krate v0.5.5 ({KRATE_SDK}/crates/bindings-rust)
   Compiling habit-tracker v0.1.0-dev (.)
error[E0599]: no method named \`floor\` found for type \`f32\` in the current scope
   --> src/lib.rs:356:51
    |
356 |         let c = ((x - (r.x + 16.0)) / lay.cell()).floor();
    |                                                   ^^^^^ method not found in \`f32\``;

const LIVE_FIX =
  "Fix the compiler errors above. The build uses rustup's toolchain and the wasm32-wasip1 target; run `krate doctor` if the target or toolchain looks wrong.";

test("the compiler's error comes first, without cargo's progress lines", () => {
  const words = compilerWords(LIVE);
  assert.ok(!/Downloaded|Compiling|Locking|Updating|Generating bindings/.test(words), words);
  assert.ok(words.includes("error[E0599]: no method named `floor`"), "the error itself stays");
  assert.ok(words.includes("src/lib.rs:356:51"), "and where it is");
  assert.ok(words.length < LIVE.length / 2 + 200, "noticeably shorter");
});

test("a missing float method names the one import that provides it", () => {
  const fix = remoteFix(compilerWords(LIVE), LIVE_FIX);
  assert.match(fix, /use krate::prelude::\*;/);
  assert.match(fix, /`floor`/);
  assert.ok(!/krate doctor/.test(fix), "no advice a chat user cannot follow");
});

test("an untyped float literal is the same case", () => {
  const fix = remoteFix("error[E0599]: no method named `sqrt` found for type `{float}` in the current scope", LIVE_FIX);
  assert.match(fix, /use krate::prelude::\*;/);
});

test("a method the prelude does not give keeps the general advice, without krate doctor", () => {
  const fix = remoteFix("error[E0599]: no method named `to_bits_le` found for type `f32` in the current scope", LIVE_FIX);
  assert.ok(!/prelude/.test(fix), fix);
  assert.ok(!/krate doctor/.test(fix), fix);
  assert.match(fix, /Fix the compiler errors above/);
});

test("a floor on the app's own type is not the float case", () => {
  const fix = remoteFix("error[E0599]: no method named `floor` found for struct `Grid` in the current scope", LIVE_FIX);
  assert.ok(!/prelude/.test(fix), fix);
});

test("a specific fix from the check passes through untouched", () => {
  const own = "Declare `net.http` in manifest.toml.";
  assert.equal(remoteFix("error: something else", own), own);
});
