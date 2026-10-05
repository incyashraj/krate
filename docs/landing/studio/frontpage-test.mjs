// The homepage's prompt bar starts a build in Studio -- and only it can.
//
// krate.tech/#start writes the typed sentence to the site's own storage and
// opens /app; Studio starts from it. The rules that keep that safe (K-885:
// a page load must never start a build at somebody's expense):
//   - only a handoff in this site's storage starts anything, never the URL
//   - it is used once
//   - a stale one (over thirty minutes; an email sign-in link alone lasts fifteen) is dropped, not started
//   - signed out, it waits for sign-in instead of being used up
// The function is lifted out of the shipped bridge and run, not re-typed.
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
const code = [
  lift("const START_KEY", ";"),
  lift("function startFromFrontPage(", "\n}\n"),
].join("\n");

function world({ token = "krs_x", handoff = undefined, search = "" } = {}) {
  const store = new Map();
  if (handoff !== undefined) store.set("krate_start_request", JSON.stringify(handoff));
  const sent = [];
  const box = { value: "", dispatchEvent() {} };
  const els = {
    homePrompt: box,
    homeSend: { click: () => sent.push(box.value) },
    viewHome: { classList: { contains: () => false } },
    welcomeSheet: { classList: { add() {} } },
  };
  const env = {
    localStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
      removeItem: (k) => store.delete(k),
    },
    document: { getElementById: (id) => els[id] || null },
    bridge: { token },
    location: { search },
    Event: class { constructor(t) { this.type = t; } },
    setTimeout: (fn) => fn(),
  };
  const run = new Function(
    "localStorage", "document", "bridge", "location", "Event", "setTimeout",
    `${code}; startFromFrontPage();`,
  );
  return { store, sent, go: () => run(env.localStorage, env.document, env.bridge, env.location, env.Event, env.setTimeout) };
}

// A fresh handoff from the front page starts, with the person's words, once.
{
  const w = world({ handoff: { text: "  a recipe box  ", at: Date.now() } });
  w.go();
  assert.deepEqual(w.sent, ["a recipe box"], "the sentence is sent as typed");
  assert.equal(w.store.has("krate_start_request"), false, "and used up");
  w.go();
  assert.deepEqual(w.sent, ["a recipe box"], "a second load starts nothing");
}
// A stale one is dropped, not started.
{
  const w = world({ handoff: { text: "old", at: Date.now() - 31 * 60 * 1000 } });
  w.go();
  assert.deepEqual(w.sent, [], "a request from over thirty minutes ago never fires");
  assert.equal(w.store.has("krate_start_request"), false, "and is cleared");
}
// Signed out: it waits for sign-in instead of being spent.
{
  const w = world({ token: null, handoff: { text: "a timer", at: Date.now() } });
  w.go();
  assert.deepEqual(w.sent, [], "nothing starts without an account");
  assert.equal(w.store.has("krate_start_request"), true, "and the request is kept for after sign-in");
}
// A URL cannot start a build: nothing in the address is read.
{
  const w = world({ search: "?start=a%20crypto%20miner" });
  w.go();
  assert.deepEqual(w.sent, [], "a crafted link starts nothing");
  assert.doesNotMatch(code, /location\.|URLSearchParams/, "the start path never reads the address");
}
console.log("ok  the front page starts a build once, fresh, signed in, and never from a link");
