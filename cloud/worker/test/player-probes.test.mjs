/* Haptics, motion and unlock in a browser tab (the web Player's hosts.js).
 *
 * An app probes these with `available()` and adapts. A tab has none of the
 * three, so the probe must answer false and no notice may be shown: the
 * notice says "works in Krate on a computer", which a tab cannot promise.
 * The player.js notice is only reached through the host's `notice` callback
 * (app-worker.js posts it), so asserting that callback is never called covers
 * that path.
 *
 *   node --test cloud/worker/test/player-probes.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { buildImports } from "../../../crates/adapter-web/player/web/hosts.js";

const probe = (f) => {
  try {
    return f();
  } catch (e) {
    return `threw: ${e.message}`;
  }
};

// The hub's table for a tab: what each function answers when the tab lacks it.
const refusals = {
  "krate:ui/haptics": { available: { value: false }, play: { trap: true }, impact: { trap: true } },
  "krate:ui/motion": { available: { value: false }, read: { value: null } },
  "krate:ui/unlock": { available: { value: false }, check: { throw: "variant", case: "unavailable", words: true } },
};

const ctx = (notices) => ({
  meta: { refusals, imports: [] },
  grants: { allows: () => true },
  bridge: { call: () => ({ json: {} }) },
  ui: {}, kv: {}, ring: {}, output() {},
  notice: (iface, fname) => notices.push([iface, fname]),
});

test("a tab answers the haptics, motion and unlock probes with false and shows no notice", () => {
  const notices = [];
  const imports = buildImports(ctx(notices));

  assert.strictEqual(probe(() => imports["krate:ui/haptics"].available()), false);
  assert.strictEqual(probe(() => imports["krate:ui/motion"].available()), false);
  assert.strictEqual(probe(() => imports["krate:ui/unlock"].available()), false);

  // The other calls answer the way a phone without the feature does.
  assert.strictEqual(probe(() => imports["krate:ui/haptics"].play("light")), undefined);
  assert.strictEqual(probe(() => imports["krate:ui/haptics"].impact(0.5)), undefined);
  assert.strictEqual(probe(() => imports["krate:ui/motion"].read()), undefined);
  assert.deepStrictEqual(probe(() => imports["krate:ui/unlock"].check("Unlock your vault")), {
    tag: "unavailable",
    val: "This device cannot check it is you.",
  });

  assert.deepStrictEqual(notices, [], "no notice for these three");
});
