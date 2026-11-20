/* The person's local time in a browser tab (the web Player's hosts.js).
 *
 * IC-991: `krate:locale/info.utc-offset` answers minutes east of UTC in the
 * tab's own zone at an instant, daylight saving included, so an app's
 * "today" is the person's day and not the UTC one. Node honours a TZ set at
 * run time, which stands in for the browser's zone.
 *
 *   node --test cloud/worker/test/player-locale.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { buildImports } from "../../../crates/adapter-web/player/web/hosts.js";

const ctx = () => ({
  meta: { refusals: {}, imports: [] },
  grants: { allows: () => true },
  bridge: { call: () => ({ json: {} }) },
  ui: {}, kv: {}, ring: {}, output() {},
  notice() {},
});

// 2026-03-29 01:00 UTC: Zurich moves from +01:00 to +02:00.
const ZURICH_SPRING = 1_774_746_000_000n;

const inZone = (zone, f) => {
  const before = process.env.TZ;
  process.env.TZ = zone;
  try {
    return f();
  } finally {
    if (before === undefined) delete process.env.TZ;
    else process.env.TZ = before;
  }
};

test("a tab answers the offset of its own zone, daylight saving included", () => {
  const info = buildImports(ctx())["krate:locale/info"];
  inZone("Europe/Zurich", () => {
    assert.strictEqual(info.timezone(), "Europe/Zurich");
    assert.strictEqual(info.utcOffset(ZURICH_SPRING - 1n), 60);
    assert.strictEqual(info.utcOffset(ZURICH_SPRING), 120);
  });
  inZone("America/New_York", () => {
    // 2026-01-15 12:00 UTC and 2026-07-15 12:00 UTC.
    assert.strictEqual(info.utcOffset(1_768_478_400_000n), -300);
    assert.strictEqual(info.utcOffset(1_784_116_800_000n), -240);
  });
  inZone("Asia/Kolkata", () => {
    assert.strictEqual(info.utcOffset(ZURICH_SPRING), 330);
  });
  inZone("UTC", () => {
    assert.ok(Object.is(info.utcOffset(0n), 0), "UTC is 0, not -0");
  });
});

test("an instant past what a Date can hold answers UTC instead of NaN", () => {
  const info = buildImports(ctx())["krate:locale/info"];
  inZone("Europe/Zurich", () => {
    assert.strictEqual(info.utcOffset(18_446_744_073_709_551_615n), 0);
  });
});
