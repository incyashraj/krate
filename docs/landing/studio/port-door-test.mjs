/* The port door shows the plan, and Port it is its Build it (2026-10-03).
 *
 * A person picks a project folder; the engine's read-only plan comes back
 * as `krate.port.plan.v1`; Studio turns it into one message. Two things
 * must be true of that message, and they are what this test pins:
 *   - a plan with no blocker names the work and offers Port it;
 *   - a plan with a blocker says the blocker in full and offers nothing,
 *     because a button under a wall is the button that wastes the AI.
 *
 * Runs the SHIPPED showPortPlan out of app.js against the engine's real
 * plan for a tkinter to-do app (evidence/ports/plan-tkinter-todo.json, captured
 * from `krate port --format json`), with the few collaborators it calls
 * stubbed and recorded.
 *
 *   node docs/landing/studio/port-door-test.mjs
 */
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
import vm from "node:vm";

const app = readFileSync("studio/ui/app.js", "utf8");
const start = app.indexOf("function showPortPlan(");
assert.ok(start > 0, "showPortPlan is in app.js");
const end = app.indexOf("\nasync function portNow(", start);
assert.ok(end > start, "portNow follows showPortPlan");
const fn = app.slice(start, end);

function run(plan) {
  const said = [];
  const staged = [];
  const ctx = {
    say: (who, body, files, extra) => said.push({ who, body, extra }),
    showPlanning: (...args) => staged.push(args),
    clearAnsweredActions: () => {},
    capWords: (c) => ({ "ui.window:create": "Open a window", "store.kv": "Save its own settings" }[c] || c),
    portNow: () => {},
    state: { session: { messages: [{}], portSource: null } },
  };
  vm.createContext(ctx);
  vm.runInContext(fn + "\nshowPortPlan(plan, source, name);", Object.assign(ctx, { plan, source: "/Users/someone/app", name: "app" }));
  return { said, staged, state: ctx.state };
}

// 1. The real tkinter plan: needs changes, no blocker -> Port it.
const plan = JSON.parse(readFileSync("evidence/ports/plan-tkinter-todo.json", "utf8"));
assert.equal(plan.verdict, "needs-changes", "the fixture is the engine's needs-changes plan");
{
  const { said, staged, state } = run(plan);
  assert.equal(said.length, 1, "one message");
  const m = said[0];
  assert.equal(m.who, "KRATE");
  assert.match(m.body, /python/i, "names the language");
  assert.match(m.body, /tkinter/i, "names the framework");
  assert.match(m.body, /rewrites it in Rust/, "says the port is a rewrite");
  assert.match(m.body, /changed nothing/, "says the folder was not touched");
  assert.match(m.body, /ask your permission to: open a window/, "the permissions, in words");
  assert.ok(m.extra && m.extra.actions && m.extra.actions.some((a) => a.label === "Port it" && a.primary), "Port it is the primary action");
  assert.equal(staged.length, 1, "the stage shows the plan is ready");
  assert.equal(staged[0][0], "The port plan is ready");
  assert.equal(state.session.portSource, "/Users/someone/app", "the source is remembered on the session, so a retry after a restart can port again");
  console.log("ok   a plan with no blocker names the work and offers Port it");
}

// 2. A blocker: said in full, with its file, and no button.
{
  const blocked = JSON.parse(JSON.stringify(plan));
  blocked.verdict = "unsupported";
  blocked.findings.push({
    id: "process",
    severity: "blocker",
    confidence: "high",
    title: "Spawns processes",
    detail: "Krate apps cannot start other programs.",
    evidence: [{ path: "src/run.py", line: 42 }],
    capability: null,
  });
  const { said, staged } = run(blocked);
  assert.equal(said.length, 1);
  const m = said[0];
  assert.match(m.body, /can't be ported as it is/);
  assert.match(m.body, /Spawns processes \(src\/run\.py:42\)/, "the blocker, with where it is");
  assert.match(m.body, /cannot start other programs/, "and the detail");
  assert.ok(!(m.extra && m.extra.actions && m.extra.actions.length), "no Port it under a wall");
  assert.equal(staged.length, 0, "nothing on the stage says the plan is ready");
  console.log("ok   a blocker is said in full and offers no button");
}

// 3. Sabotage: a renderer that offers Port it under a blocker must fail here.
{
  const sabotaged = fn.replace('if (plan.verdict === "unsupported" || blockers.length) {', "if (false) {");
  assert.notEqual(sabotaged, fn, "the sabotage landed");
  const ctx = { say: (w, b, f, extra) => { ctx.extra = extra; }, showPlanning: () => {}, clearAnsweredActions: () => {}, capWords: (c) => c, portNow: () => {}, state: { session: { messages: [{}] } } };
  vm.createContext(ctx);
  const blocked = JSON.parse(JSON.stringify(plan)); blocked.verdict = "unsupported";
  vm.runInContext(sabotaged + "\nshowPortPlan(plan, '/x', 'x');", Object.assign(ctx, { plan: blocked }));
  assert.ok(ctx.extra && ctx.extra.actions && ctx.extra.actions.length, "the sabotaged renderer offers Port it under a wall, which is what this test catches");
  console.log("ok   the test bites: a renderer that ignores blockers would offer Port it");
}
