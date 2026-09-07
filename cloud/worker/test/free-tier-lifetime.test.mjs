/* Three free makes EVER, agreed by every layer that counts them (IC-639).
 *
 * Three layers kept this number: localStorage in the WebView, plan.json in
 * the shell, and mkacct:/mkdev: in the hub. Two of them keyed the count by
 * month and one did not, so on the first of every month the client offered
 * three fresh makes and the server refused them -- the same product telling
 * a person two different things.
 *
 * The shell and client decision logic is mirrored here, the way
 * plan-count.test.mjs mirrors the hub's: one file, no module boundary, and a
 * copy that drifts fails loudly. If plan_makes/plan_count_make in
 * studio/src/main.rs or makesLocal/makesThisMonth in studio/ui/app.js change,
 * change this too.
 *
 *   node cloud/worker/test/free-tier-lifetime.test.mjs
 */
let failed = 0;
const say = (label, got, want) => {
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}: ${got} (want ${want})`);
};

// Mirrors the shell's plan.json record and its two commands.
function shell() {
  let stored = { dev: "", month: "", n: 0 };
  const DEVICE = "this-machine";
  return {
    makes(seedN = 0, now = "2026-09") {
      let n = stored.dev === DEVICE ? stored.n : 0;
      n = Math.max(n, seedN);
      stored = { dev: DEVICE, month: now, n };
      return { month: now, n };
    },
    count(now = "2026-09") {
      const n = (stored.dev === DEVICE ? stored.n : 0) + 1;
      stored = { dev: DEVICE, month: now, n };
      return { month: now, n };
    },
  };
}

// Mirrors makesLocal(): no month comparison.
const makesLocal = (rec) => rec.n || 0;

const s = shell();
say("first make", s.count("2026-09").n, 1);
say("second", s.count("2026-09").n, 2);
say("third", s.count("2026-09").n, 3);

// The rollover. This is the case the old code got wrong: a new month reset
// the shell and the client to zero while the hub still said three.
say("next month does not refill", s.makes(0, "2026-10").n, 3);
say("and a make in the new month is the fourth", s.count("2026-10").n, 4);

// A year later, same answer.
say("a year later, still spent", s.makes(0, "2027-09").n, 4);

// The localStorage seed is an upgrade path, not a monthly reset: an old
// record from another month still counts.
say("an old-month local record still counts", makesLocal({ month: "2026-01", n: 2 }), 2);
say("an empty record is zero", makesLocal({}), 0);

// And the seed can only raise the count, never lower it.
const s2 = shell();
s2.count("2026-09");
s2.count("2026-09");
say("a stale seed cannot erase makes", s2.makes(0, "2026-09").n, 2);
say("a larger seed is honoured", s2.makes(5, "2026-09").n, 5);

if (failed) {
  console.error(`${failed} check(s) failed`);
  process.exit(1);
}
console.log("three ever, agreed by every layer");
