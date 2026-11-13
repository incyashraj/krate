// Studio never sells anything (F-322, 2026-11-07).
//
// Krate has no paid plans. Studio used to carry a whole paid path behind a
// CHARGING flag that was false: a "$12/month" plan sheet, a limit wall with
// checkout buttons, a founding-200 offer, a priority-support tag, and two
// shell commands (billing_info, billing_checkout) that opened Stripe. It
// was unreachable only because a flag happened to be false -- protection
// by accident. It is gone now, and this keeps it gone: no price, no
// checkout and no billing command anywhere in Studio's UI or its web
// bridge, in code or in the markup a person sees.
//
// What stays is the free app on Krate's own AI key in a browser: one app
// and one change, told plainly on the web only (the desktop builds with the
// person's own AI and has no allowance at all), with the two free ways on.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const app = readFileSync("studio/ui/app.js", "utf8");
const bridge = readFileSync("docs/landing/studio/bridge.js", "utf8");
const html = readFileSync("studio/ui/index.html", "utf8");

/** Source with comments stripped, so a paragraph explaining what was
 *  removed is not mistaken for code that still does it. */
const bare = (src) => src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
/** Markup with its comments and scripts stripped: what a person can read. */
const visible = (src) => src.replace(/<!--[\s\S]*?-->/g, "").replace(/<script[\s\S]*?<\/script>/g, "");

const MONEY = /\$\d+\s*(?:\/|a |per )\s*(?:month|year|mo\b|yr\b)|\bcheckout\b|stripe|\bbilling_|\/billing\b|founding|upgrade for|per month|\/month|\/year|subscribe|pay by card|CHARGING|priority/i;
for (const [name, text] of [
  ["studio/ui/app.js", bare(app)],
  ["docs/landing/studio/bridge.js", bare(bridge)],
  ["studio/ui/index.html", visible(html)],
]) {
  const hit = MONEY.exec(text);
  assert.ok(!hit, `${name} sells nothing (found ${hit && JSON.stringify(text.slice(Math.max(0, hit.index - 60), hit.index + 40))})`);
}

// The fixture really carries the hazard: the pattern catches what used to
// be there, so a clean pass means the strings are gone, not that the test
// cannot see them.
for (const old of [
  `'<button class="btn btn-primary" id="buyMonthly">Upgrade for $12/month</button>'`,
  `const url = await invoke("billing_checkout", { plan });`,
  `const CHARGING = false;`,
  `<span class="plan-tag sup-priority hidden" id="supPriorityTag">priority</span>`,
]) {
  assert.ok(MONEY.test(old), `the check would catch: ${old}`);
}

/** One function's body, from its opening brace to its matching close. */
function body(name) {
  const m = new RegExp(`(?:async )?function ${name}\\s*\\([^)]*\\)\\s*\\{`).exec(app);
  assert.ok(m, `${name} exists`);
  let depth = 0;
  const from = m.index + m[0].length - 1;
  for (let i = from; i < app.length; i++) {
    if (app[i] === "{") depth++;
    else if (app[i] === "}" && --depth === 0) return bare(app.slice(from, i + 1));
  }
  throw new Error(`${name} has no closing brace`);
}

// The free-app words are the web's alone: the desktop has no allowance, so
// each surface that shows it tests `tauri` before it paints anything.
for (const name of ["maybeWelcome", "renderFreeCount", "openPlanSheet"]) {
  const text = body(name);
  const guard = text.search(/\btauri\b/);
  const paint = text.search(/textContent\s*=|innerHTML\s*=|classList\.remove\(\s*["']hidden/);
  assert.ok(guard > 0, `${name} tests \`tauri\` -- the desktop/web seam`);
  assert.ok(paint === -1 || guard < paint, `${name} decides on the desktop before it paints`);
}

// The free allowance is one app, from the constant the hub enforces.
assert.match(bare(app), /const FREE_MAKES = 1;/, "the free allowance is one app");

// Where the free app runs out, the sheet names the two free ways on.
const after = /<b>After that<\/b><span>([^<]*)<\/span>/.exec(html);
assert.ok(after, "the What-is-free sheet has its After-that row");
assert.match(after[1], /own API key/, "after the free app: your own API key");
assert.match(after[1], /Studio on your computer is free/, "or Studio on your computer, free with your own AI");

console.log("ok  no price, checkout or billing command anywhere in Studio");
console.log("ok  the free-app words are web-only, behind the desktop/web seam");
console.log("ok  after the free app, the two free ways on are named");
