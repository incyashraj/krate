// The desktop never sells anything.
//
// A build on somebody's own machine uses their own AI and costs Krate
// nothing, so the allowance, its wall and every price belong to the WEB
// alone. That rule has drifted three times: the UI said "three apps a
// month" while the shipped constant was one (K-216), the free-count chip
// computed `3 - n` under a constant of 1, and the plan sheet with its
// "$12/month" button was reachable from two desktop sidebar buttons --
// harmless only because a CHARGING flag happened to be false, which is
// protection by accident rather than by decision.
//
// So this checks the SEAM, not the flag: every money surface must be
// behind the `tauri` test that distinguishes the desktop from a tab, and
// must stay behind it on the day CHARGING flips.
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

/** A function body with its comments stripped, so a long explanation
 *  above a guard cannot look like code that ran before it. Measuring raw
 *  characters made this test fail on a guard that was in the right place
 *  under a comment saying why -- the wrong thing to punish. */
function code(name) {
  return body(name)
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "")
    .replace(/\n\s*\n/g, "\n");
}

// Each money surface, and the guard that must come before it does
// anything. The guard is checked EARLY -- within the first few statements
// of real code -- because a check after the sheet is already painted is
// not a guard.
for (const name of ["maybeWelcome", "renderFreeCount", "openPlanSheet"]) {
  const text = code(name);
  const guard = text.search(/\btauri\b/);
  assert.ok(guard > 0, `${name} tests \`tauri\` -- the desktop/web seam`);
  assert.ok(
    guard < 300,
    `${name} tests \`tauri\` early, before it paints anything (found at ${guard} of code)`,
  );
}

// The cap in make() is web-only, and says so with !tauri rather than by
// leaning on CHARGING being false.
const make = code("make");
assert.match(
  make,
  /let overCap = !tauri/,
  "the free-app cap in make() is web-only (!tauri)",
);

// No price string may sit in a function the desktop can run to completion.
// openPlanSheet is allowed to exist on the desktop -- the referral block is
// worth keeping -- but it must return before any pricing is dressed on.
// Read from the same comment-stripped text the offsets come from: a
// mention of dressPlanSheet inside the comment that EXPLAINS the guard is
// not code that runs, and an earlier cut of this test failed on exactly
// that.
const plan = code("openPlanSheet");
const tauriReturn = /if \(tauri\)[\s\S]{0,400}?return;/.exec(plan);
assert.ok(tauriReturn, "openPlanSheet returns early on the desktop");
assert.ok(
  !/dressPlanSheet|\$12|checkout/.test(plan.slice(0, tauriReturn.index + tauriReturn[0].length)),
  "nothing priced runs before the desktop return",
);

// Nothing may be written onto the free-count chip before the desktop
// guard decides whether it should exist at all. A character budget alone
// did not catch a paint inserted just above the guard -- both fitted
// inside it -- so the order of the two is what is asserted.
{
  const text = code("renderFreeCount");
  const guard = text.search(/\btauri\b/);
  const paint = text.search(/textContent\s*=|innerHTML\s*=/);
  assert.ok(
    paint === -1 || guard < paint,
    "renderFreeCount decides on the desktop BEFORE it writes to the chip",
  );
}

// The free count is computed from the constant, never from a literal. "3"
// next to FREE_MAKES is how the wording drifted from the decision before.
const free = code("renderFreeCount");
assert.ok(
  !/\b3\s*-\s*n\b/.test(free),
  "the remaining-free count uses FREE_MAKES, not a hardcoded 3",
);
// The constant itself, read from the file with its comments stripped for
// the same reason: the paragraph above it discusses "3 free apps" as the
// mistake it was, and a search over raw source would find that.
const bare = src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
assert.match(bare, /const FREE_MAKES = 1;/, "the free allowance is one app");

console.log("ok  every money surface is behind the desktop/web seam");
console.log("ok  the desktop cannot reach a price, whatever CHARGING says");
console.log("ok  the free count comes from the constant, not a literal");
