/* No builder test binds a fixed port (issue #45).
 *
 * A test that listens on a number it picked itself fails with EADDRINUSE as
 * soon as parallel runs, or anything else on the machine, hold that number.
 * Hubs and builders listen on port 0 and read the real port back. This test
 * reads the sibling test files and fails if one of them pins a port again.
 */
import assert from "node:assert/strict";
import test from "node:test";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
// `.listen(8943`, `PORT = 8943`, `HUB_PORT = 8943`, `{ port: 8943 }`,
// `port: 8943`, and a literal port in a URL such as `127.0.0.1:8943`.
const FIXED = /\b\w*PORT\s*=\s*\d{2,5}\b|\bport\s*:\s*\d{2,5}\b|\.listen\(\s*\d{2,5}\b|(?:127\.0\.0\.1|localhost|0\.0\.0\.0):\d{2,5}\b/i;

test("no builder test binds a fixed port", () => {
  const offenders = [];
  for (const name of readdirSync(here)) {
    if (!name.endsWith(".test.mjs") || name === "no-fixed-ports.test.mjs") continue;
    const lines = readFileSync(join(here, name), "utf8").split("\n");
    lines.forEach((line, i) => {
      if (FIXED.test(line)) offenders.push(`${name}:${i + 1}: ${line.trim()}`);
    });
  }
  assert.deepEqual(offenders, [], "a test binds a fixed port; listen on 0 and read the real port instead:\n" + offenders.join("\n"));
});
