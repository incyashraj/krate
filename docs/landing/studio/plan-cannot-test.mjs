// The plan step's third answer (K-1243). `krate plan` answers a question
// ({"ask"}), a plan ({"plan"}), or, since 36c5efc21, a refusal:
//   {"cannot": "Krate cannot build that: ... Try instead: ...", "instead", "reason", "limit"}
// Studio read only the first two, so a refusal fell through to the build --
// in Plan mode too -- and `create` refused it a second later. The plan step
// is executed here, lifted out of the shipped app.js, against stubs.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const app = readFileSync("studio/ui/app.js", "utf8");
function lift(start) {
  const at = app.indexOf(start);
  assert.ok(at >= 0, `${start} is in app.js`);
  const end = app.indexOf("\n}\n", at);
  assert.ok(end > at, `the end of ${start} is found`);
  return app.slice(at, end + 3);
}
const src = ["function cannotMessage(", "function showCannot(", "function planMessage(", "function planAnsweredAway(", "async function runPlanInner("].map(lift).join("\n");

const REFUSAL = {
  cannot: "Krate cannot build that: a Krate app cannot sign in to your Google account. Try instead: an inbox you paste messages into, sorted by sender.",
  instead: "an inbox you paste messages into, sorted by sender",
  reason: "a Krate app cannot sign in to your Google account",
  limit: "third-party-account",
};

function world(mode, answer) {
  const els = {};
  const el = (id) => (els[id] ||= { id, value: "", placeholder: "", disabled: true, textContent: "", focus() {} });
  const S = { id: "s1", title: "read my gmail", messages: [] };
  const log = { said: [], built: 0, submitted: [], shown: [], persisted: 0, saved: [] };
  const state = { session: S, planning: { request: "an app that reads my Gmail inbox", files: [], rounds: 0 }, agent: "claude" };
  const deps = {
    $: el, state,
    ensureUsableAgent: async () => {},
    invoke: async () => JSON.stringify(answer),
    planContext: () => state.planning.request,
    say: (who, body, files, extra) => { log.said.push({ who, body, extra }); S.messages.push({ who, body }); },
    showPlanning: () => log.shown.push("planning"),
    finishPlanningAndBuild: () => { log.built += 1; },
    composerMode: () => mode,
    clearAnsweredActions: () => {},
    capWords: (c) => c,
    persist: () => { log.persisted += 1; },
    persistSession: (s) => log.saved.push(s),
    setIdleNote: () => {},
    show: (what) => log.shown.push(what),
    unlockComposer: (p) => { el("prompt").disabled = false; el("prompt").placeholder = p; },
    submitInSession: () => log.submitted.push(el("prompt").value),
    wallActions: () => [], resumeWhenKeyed: () => {}, runPlan: () => {},
    looksLikeAQuestion: () => false, plainWords: (e) => String(e),
    window: {},
  };
  const names = Object.keys(deps);
  const fns = new Function(...names, `${src}; return { runPlanInner, planAnsweredAway };`)(...names.map((n) => deps[n]));
  return { ...fns, log, state, S, el };
}

for (const mode of ["plan", "build"]) {
  const w = world(mode, REFUSAL);
  await w.runPlanInner();
  assert.equal(w.log.built, 0, `${mode} mode: a refusal starts no build`);
  const said = w.log.said.find((m) => m.who === "KRATE");
  assert.ok(said, `${mode} mode: the refusal is said`);
  assert.match(said.body, /^Krate cannot build that: a Krate app cannot sign in to your Google account\./, said.body);
  assert.equal((said.body.match(/inbox you paste messages into/g) || []).length, 1, "the suggestion is said once, not twice");
  const acts = said.extra.actions || [];
  assert.deepEqual(acts.map((a) => a.label), ["Make this instead"], "one action, and it is not Build it");
  assert.equal(w.state.planning, null, "nothing is left waiting to be built");
  assert.equal(w.el("send").disabled, false, "the box is free to type in");
  acts[0].run();
  assert.deepEqual(w.log.submitted, [REFUSAL.instead], "Make this instead sends the suggestion as the next request");
}
console.log("ok  a refusal from the plan step is said, with Make this instead, and nothing is built (Plan and Build mode)");

// A refusal without a suggestion still says it, with no button.
{
  const w = world("plan", { cannot: "Krate cannot build that: it would need a GPU compute shader." });
  await w.runPlanInner();
  assert.equal(w.log.built, 0);
  const said = w.log.said.find((m) => m.who === "KRATE");
  assert.match(said.body, /GPU compute shader/);
  assert.deepEqual(said.extra.actions, []);
}
console.log("ok  a refusal with nothing to offer instead has no button");

// Answered while the person was in another session: written there, with no
// plan kept to resume (which would bring Build it back on reopening).
{
  const w = world("plan", REFUSAL);
  const P = { request: "an app that reads my Gmail inbox", rounds: 0 };
  w.planAnsweredAway(w.S, P, REFUSAL, null);
  const last = w.S.messages[w.S.messages.length - 1];
  assert.match(last.body, /Krate cannot build that/);
  assert.notEqual(last.kind, "ask", "not recorded as a question, so reopening adds no Build it");
  assert.equal(w.S.planningSaved, null);
}
console.log("ok  a refusal that lands in another session is kept there, with no Build it to come back");

// The plan and the question arms still work.
{
  const w = world("plan", { plan: "A habit tracker.", points: ["A tick box per habit"], name: "Habits" });
  await w.runPlanInner();
  assert.equal(w.log.built, 0, "Plan mode waits on a plan");
  assert.ok(w.log.said.some((m) => /Here's what I'll build/.test(m.body)));
  const b = world("build", { plan: "A habit tracker.", points: [], name: "Habits" });
  await b.runPlanInner();
  assert.equal(b.log.built, 1, "Build mode builds a plan straight away");
}
console.log("ok  a plan is still a plan");
