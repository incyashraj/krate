// Session times are seconds on every side (K-938). The web bridge wrote
// milliseconds in two places, so its copy of a session always looked newer
// and won the merge: a rename or a new message saved from Studio vanished
// on the next list. The merge and the time helpers are lifted out of the
// shipped bridge and run, not re-typed.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");
function lift(start, endMarker) {
  const at = src.indexOf(start);
  assert.ok(at >= 0, `${start} is in bridge.js`);
  const end = src.indexOf(endMarker, at);
  assert.ok(end > at, `the end of ${start} is found`);
  return src.slice(at, end + endMarker.length);
}
const helpers = [
  lift("function nowSecs()", "}"),
  lift("function toSecs(", "\n}\n"),
  lift("function inSeconds(", "\n}\n"),
].join("\n");
const method = lift("  async sessions_list() {", "\n  },\n")
  .replace("  async sessions_list() {", "async function sessions_list() {")
  .replace(/\n  },\n$/, "\n}\n");

function world(local, remote) {
  let saved = null;
  const run = new Function(
    "localSessions", "saveLocalSessions", "bridge", "hub",
    `${helpers}\n${method}\nreturn sessions_list();`,
  );
  return run(
    () => local,
    (list) => { saved = list; },
    { token: "krs_x" },
    async () => ({ sessions: remote }),
  ).then((list) => ({ list, saved }));
}

const t = 1_790_000_000; // seconds, as Studio writes them
// The stale web copy (milliseconds, older) against Studio's rename (seconds, newer).
{
  const { list } = await world(
    [{ id: "a", title: "renamed", updated: t + 60 }],
    [{ id: "a", title: "old name", updated: (t + 5) * 1000 }],
  );
  assert.equal(list[0].title, "renamed", "the newer save wins, whatever unit the older one was in");
  assert.equal(list[0].updated, t + 60);
}
// A millisecond session does not sit above newer ones forever.
{
  const { list } = await world(
    [{ id: "old", title: "old", updated: (t - 3600) * 1000 }, { id: "new", title: "new", updated: t }],
    [],
  );
  assert.deepEqual(list.map((s) => s.id), ["new", "old"], "sorted by real time");
  assert.equal(list[1].updated, t - 3600, "read back in seconds");
}
// The bridge itself writes seconds now.
{
  const run = new Function(`${helpers}; return nowSecs();`);
  const n = run();
  assert.ok(n < 1e11 && Math.abs(n - Date.now() / 1000) < 5, "nowSecs is seconds");
  assert.doesNotMatch(src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, ""),
    /(updated|created):\s*Date\.now\(\)/, "no session time is written in milliseconds");
}
console.log("ok  session times are seconds everywhere, so the newest save always wins");
