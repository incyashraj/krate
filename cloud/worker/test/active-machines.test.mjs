/* The denominator: how many machines were active each day (K-243, IC-428).
 *
 * Every adoption number is a count of events. Without a per-day machine
 * count, "38,356 opens" cannot be told apart from one CI runner in a loop --
 * and that is not hypothetical, it is what our own numbers look like: 461
 * distinct installs against 38,356 opens is 83 opens per machine.
 *
 * The old active_installs_by_day was built from KV keys that stopped being
 * written on 2026-08-10, so it has been frozen for a month while the event
 * counts kept climbing. The replacement reads the same Analytics Engine
 * dataset the events come from, counting DISTINCT index1 (the install id).
 *
 * It needs its OWN query, which is the thing worth testing: the actions
 * query groups by day, action and outcome, so its DISTINCT is per group and
 * summing those counts one machine once per action it took.
 *
 * The logic is mirrored here rather than imported, the same as
 * plan-count.test.mjs: the worker is one file with no module boundary. If
 * liveStats in cloud/worker/src/index.js changes, change this too.
 *
 *   node cloud/worker/test/active-machines.test.mjs
 */
import assert from "node:assert";

// One day, one machine, three actions. This is the shape that makes the
// naive answer wrong.
const ACTION_ROWS = [
  { day: "2026-09-01", action: "open", outcome: "ok", n: 3, installs: 1 },
  { day: "2026-09-01", action: "install", outcome: "ok", n: 1, installs: 1 },
  { day: "2026-09-01", action: "make", outcome: "ok", n: 1, installs: 1 },
];

// What a query grouped by day alone returns for the same traffic.
const ACTIVE_ROWS = [{ day: "2026-09-01", machines: 1 }];

// The aggregation from liveStats, mirrored.
function foldActions(rows) {
  const byDay = {};
  let installs = 0;
  for (const row of rows) {
    const day = (byDay[row.day] ||= {});
    const key = row.outcome === "failed" ? `${row.action}-failed` : row.action;
    day[key] = (day[key] || 0) + Number(row.n);
    if (row.action === "install") installs += Number(row.installs);
  }
  return { byDay, installs };
}

function foldActive(rows) {
  const byDay = {};
  for (const row of rows) byDay[row.day] = Number(row.machines);
  return byDay;
}

// The naive version: sum the per-group DISTINCT from the actions query.
function naiveMachines(rows) {
  return rows.reduce((total, row) => total + Number(row.installs), 0);
}

const { byDay, installs } = foldActions(ACTION_ROWS);
const active = foldActive(ACTIVE_ROWS);

assert.deepStrictEqual(
  byDay["2026-09-01"],
  { open: 3, install: 1, make: 1 },
  "actions still fold per day and outcome",
);
assert.strictEqual(installs, 1, "one install on the day");

// The point of the separate query.
assert.strictEqual(
  active["2026-09-01"],
  1,
  "one machine was active, however many actions it took",
);
assert.strictEqual(
  naiveMachines(ACTION_ROWS),
  3,
  "summing the actions query's per-group DISTINCT counts the same machine " +
    "once per action -- which is exactly why the count needs its own query",
);
assert.notStrictEqual(
  naiveMachines(ACTION_ROWS),
  active["2026-09-01"],
  "if these ever agree the fixture has stopped testing anything",
);

// And the ratio the adoption record exists to publish.
const opens = byDay["2026-09-01"].open;
assert.strictEqual(
  opens / active["2026-09-01"],
  3,
  "three opens from one machine reads as 3 per machine, not 3 people",
);

// A day the query returned nothing for must not become a division by zero
// or a fake zero: absent is absent.
assert.strictEqual(
  active["2026-08-30"],
  undefined,
  "a day with no row is unknown, never 0 -- 0 machines with opens on it " +
    "would be a claim, and a false one",
);

console.log("OK -- active machines are counted once per day, whatever they did");
