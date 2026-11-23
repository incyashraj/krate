/* A failed build keeps its engine output, and only the newest 200 are kept.
 *
 * The build folder is deleted when a job ends and Fly holds minutes of
 * logs, so before this a failure left one line behind. On 2026-11-23 a
 * report named two builds that looped for 15 rounds and nothing on the
 * builder could say why. This drives keepFailedLog itself (lifted from the
 * source, as failure-words does: the builder is one file with no exports)
 * against a real folder, and checks the failure path calls it with the
 * whole engine output, not the 4,000-character tail.
 *
 *   node cloud/builder/test/failed-log.test.mjs
 */
import { readFileSync } from "node:fs";
import * as fsp from "node:fs/promises";
import { mkdtemp, readdir, readFile, utimes } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";

const src = readFileSync("cloud/builder/src/server.js", "utf8");
const m = /async function keepFailedLog\(id, log\)\s*\{/.exec(src);
assert.ok(m, "keepFailedLog exists in the builder");
let depth = 0;
let body = "";
for (let i = m.index + m[0].length - 1; i < src.length; i++) {
  if (src[i] === "{") depth++;
  else if (src[i] === "}" && --depth === 0) {
    body = src.slice(m.index, i + 1);
    break;
  }
}
const kept = Number(/const LOGS_KEPT = (\d+);/.exec(src)?.[1]);
assert.equal(kept, 200, "the builder keeps the newest 200 failed logs");

const dir = await mkdtemp(join(tmpdir(), "krate-failed-log-"));
const keepFailedLog = new Function(
  "stateWritable", "LOGS_DIR", "LOGS_KEPT", "mkdir", "writeFile", "readdir", "stat", "unlink", "join",
  `${body}; return keepFailedLog;`,
)(true, dir, kept, fsp.mkdir, fsp.writeFile, fsp.readdir, fsp.stat, fsp.unlink, join);

// One failure: its whole output is on disk under its id.
const engine = "round 1\nchecking it builds\nerror[E0308]: mismatched types\n".repeat(200);
await keepFailedLog("job-a", engine);
assert.equal(await readFile(join(dir, "job-a.log"), "utf8"), engine);
console.log("ok  a failed build's engine output is kept under its id");
await utimes(join(dir, "job-a.log"), 1, 1); // the oldest of all

// Past the limit, the oldest go and the newest stay.
for (let i = 0; i < kept + 5; i++) {
  await keepFailedLog(`job-${String(i).padStart(3, "0")}`, `log ${i}`);
  await utimes(join(dir, `job-${String(i).padStart(3, "0")}.log`), 1000 + i, 1000 + i);
}
await keepFailedLog("job-newest", "newest");
const left = (await readdir(dir)).filter((n) => n.endsWith(".log"));
assert.equal(left.length, kept, `only the newest ${kept} are kept, not ${left.length}`);
assert.ok(left.includes("job-newest.log"), "the newest stays");
assert.ok(!left.includes("job-a.log") && !left.includes("job-000.log"), "the oldest go");
console.log(`ok  only the newest ${kept} logs are kept`);

// Both failure paths keep the whole output, and say what the build cost.
const failures = [...src.matchAll(/job\.state = "failed";[\s\S]{0,400}?await audit\(\{ action: "failed"[^\n]*/g)].map((x) => x[0]);
assert.ok(failures.length >= 2, "both failure paths found");
for (const f of failures) {
  assert.match(f, /await keepFailedLog\(job\.id, log\)/, `a failure path keeps the log:\n${f}`);
  assert.match(f, /usd: job\.spend\.usd/, `a failure path records the cost:\n${f}`);
}
assert.match(src, /log = \(log \+ buf\.toString\(\)\)\.slice\(-LOG_BYTES\)/, "the log is the engine output, bounded");
console.log("ok  every failure keeps its log and records its cost");
