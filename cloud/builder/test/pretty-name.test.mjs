// The name a web build gets when the engine did not name it: the card
// title and the download's file name. Lifted out of the shipped server and
// run, not re-typed.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync(new URL("../src/server.js", import.meta.url), "utf8");
const at = src.indexOf("function prettyName(");
assert.ok(at >= 0, "prettyName is in server.js");
const end = src.indexOf("\n}\n", at);
const prettyName = new Function(`${src.slice(at, end + 2)}; return prettyName;`)();

const requests = [
  "a pomodoro timer with a gentle chime",
  "make me a habit tracker for the week",
  "a small toolbox: generate UUIDs, hashes",
  "build a recipe box",
  "a notes app with <tags> and |pipes|",
  "",
];
for (const request of requests) {
  const got = prettyName(request);
  assert.doesNotMatch(got, /\s(a|an|the|and|or|with|for|of|to|in|on|at|by|from|that|which|so)$/i,
    `"${request}" -> "${got}" ends on a joining word`);
  assert.doesNotMatch(got, /[\\/*<>|:?"]/, `"${got}" holds a character Windows refuses in a file name`);
}
assert.equal(prettyName("a pomodoro timer with a gentle chime"), "Pomodoro timer");
assert.equal(prettyName("make me a habit tracker for the week"), "Habit tracker");
assert.equal(prettyName("a small toolbox: generate UUIDs, hashes"), "Small toolbox");
assert.equal(prettyName("build a recipe box"), "Recipe box");
assert.equal(prettyName("a notes app with <tags> and |pipes|"), "Notes app with tags");
assert.equal(prettyName("a"), "A", "a lone word is kept rather than emptied");
assert.equal(prettyName(""), "Your app");
console.log("ok  a web build's fallback name is short, whole, and a legal file name");
