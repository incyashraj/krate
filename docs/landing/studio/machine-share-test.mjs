// A wall the person did not walk into has to say why (K-807).
//
// The free app is bound to the MACHINE as well as the account: the device
// hash is the second key, because an account is free to create and an
// account-only limit is an invitation to make another email. That stays.
//
// What was missing is the explanation. Somebody signs in on a second-hand
// or shared Mac, has made nothing, and their free app is already gone. The
// refusal then reads as a bug -- on the screen where somebody decides
// whether to trust us with money.
//
// The danger in the fix is the opposite mistake: telling a person who spent
// their own app that a machine did it. That is a lie, and it is the easier
// bug to write, so it is asserted here beside the case it exists for.
//
// The real functions are EXECUTED rather than pattern-matched. A
// source-text assertion about app.js dies the moment the phrase moves into
// a comment, and what matters here is the branch a person actually reads.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync("studio/ui/app.js", "utf8");

/** One function's body, from its opening brace to its matching close. */
function body(name) {
  const m = new RegExp(`(?:async )?function ${name}\\s*\\([^)]*\\)\\s*\\{`).exec(src);
  assert.ok(m, `${name} exists`);
  let depth = 0;
  const from = m.index + m[0].length - 1;
  for (let i = from; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) return src.slice(from, i + 1);
  }
  throw new Error(`${name} has no closing brace`);
}

// Lift the three functions out of app.js and run them against a state we
// control. FREE_MAKES is read from the source too, so this cannot quietly
// disagree with the shipped allowance -- the exact drift that has already
// been corrected more than once (K-216).
const FREE_MAKES = Number(/const FREE_MAKES = (\d+);/.exec(src)?.[1]);
assert.ok(Number.isFinite(FREE_MAKES), "FREE_MAKES is a number in app.js");
assert.equal(FREE_MAKES, 1, "one free app ever -- the 2026-09-19 ruling");

// `body` returns the braces alone, so the parameter list is supplied here.
// Every function under test takes none -- they read `state`.
const build = (name, deps) =>
  new Function(...Object.keys(deps), `return function ${name}()${body(name)}`)(
    ...Object.values(deps),
  );

let state = {};
const makesThisMonth = () => (state.planMakes && state.planMakes.n) || 0;
const machineShare = build("machineShare", { state });
const makesOwn = build("makesOwn", {
  makesThisMonth: () => makesThisMonth(),
  machineShare: () => machineShare(),
});
const machineSpentIt = build("machineSpentIt", {
  state,
  FREE_MAKES,
  makesOwn: () => makesOwn(),
  makesThisMonth: () => makesThisMonth(),
});
const machineNote = build("machineNote", { machineShare: () => machineShare() });

const set = (planMakes) => {
  // `state` is captured by the built functions, so mutate it in place
  // rather than rebinding -- a fresh object would leave them reading the
  // old one and every case would silently test the same thing.
  state.planMakes = planMakes;
};

// ---- the case this exists for -------------------------------------------
set({ n: 1, machine: 1 });
assert.equal(
  machineSpentIt(),
  true,
  "a free app spent by the machine alone must be explainable",
);
assert.match(
  machineNote(),
  /machine has already made/,
  "and the words must name the machine as the reason",
);

// ---- the lie it must not tell -------------------------------------------
set({ n: 1, machine: 0 });
assert.equal(
  machineSpentIt(),
  false,
  "a person who used their own app must not be told a machine did",
);

// Both spent it: these are the SAME make seen through two keys, so the
// person made it and the ordinary words are the true ones.
set({ n: 1, machine: 1 });
state.planMakes = { n: 2, machine: 1 };
assert.equal(
  machineSpentIt(),
  false,
  "somebody who has made an app of their own is not a stranger to this machine",
);

// ---- nothing to explain -------------------------------------------------
set({ n: 0, machine: 0 });
assert.equal(machineSpentIt(), false, "a fresh start says nothing");

// ---- an older hub, or an offline shell ----------------------------------
// No `machine` field at all. Absent must read as "nothing to explain"
// rather than as an excuse to invent one.
set({ n: 1 });
assert.equal(
  machineSpentIt(),
  false,
  "a hub that cannot say must not make the client guess",
);
set({ n: 1, machine: undefined });
assert.equal(machineSpentIt(), false, "and undefined is not an explanation");
// A string that coerces to a number. Two guards read this field --
// machineSpentIt and machineShare -- and junk only gets through when BOTH
// coerce, so a sabotage of one alone survives. Worth knowing before
// concluding this assertion is toothless: it was checked.
set({ n: 1, machine: "1" });
assert.equal(
  machineSpentIt(),
  false,
  "nor is a string -- a coerced value would be trusting junk on a money screen",
);

// ---- the words are for a person, not a log ------------------------------
set({ n: 1, machine: 1 });
assert.equal(
  machineNote(),
  "This machine has already made its free app.",
  "one app reads as one app, not as a count of 1",
);
set({ n: 3, machine: 3 });
assert.match(machineNote(), /already made 3 apps/, "and several read as several");

console.log("ok  a wall the person did not walk into says why, and no other wall does");
