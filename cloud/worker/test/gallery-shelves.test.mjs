// The gallery's first shelves: "Start here" holds the apps chosen in
// GALLERY_PICKS, in that order, and the newest shelf says "New this week"
// only when its newest app is from this week.
import assert from "node:assert/strict";
import test from "node:test";
import { orgEnv, call } from "./org-tools.mjs";

const DAY = 86400_000;
const hex = (n) => n.toString(16).repeat(64).slice(0, 64);

async function gallery(env, apps) {
  for (const [n, name, age] of apps) {
    await env.APPS.put(`app:${hex(n)}`, JSON.stringify({ name, published: Date.now() - age, category: "apps" }));
  }
  const res = await call(env, null, "GET", "/apps");
  assert.equal(res.status, 200, JSON.stringify(res.body));
  return res.body.shelves;
}

test("Start here leads with the picked apps in order, and an old newest shelf is just Newest", async () => {
  const env = orgEnv({ GALLERY_PICKS: `${hex(2).slice(0, 12)}, ${hex(1).slice(0, 12)}, nothex, ${hex(9).slice(0, 12)}` });
  const shelves = await gallery(env, [[1, "Pad", 40 * DAY], [2, "Markdown Editor", 30 * DAY], [3, "When Asked What", 14 * DAY]]);
  assert.equal(shelves[0].id, "picks");
  assert.equal(shelves[0].label, "Start here");
  assert.deepEqual(shelves[0].apps.map((a) => a.id), [hex(2), hex(1)], "in the chosen order; a bad or unknown pick is skipped");
  assert.equal(shelves[1].id, "new");
  assert.equal(shelves[1].label, "Newest", "the newest app is two weeks old");
});

test("with no picks there is no Start here, and a fresh newest shelf says New this week", async () => {
  const env = orgEnv();
  const shelves = await gallery(env, [[4, "Diff", 2 * DAY]]);
  assert.equal(shelves[0].id, "new");
  assert.equal(shelves[0].label, "New this week");
});
