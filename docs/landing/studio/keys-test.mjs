// The bridge's key and spend commands, run against a stub hub, using the
// shipped source rather than a copy of it.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");

// Pull the three key commands and the spend report out of COMMANDS and run
// them against a recorded fetch. Everything they touch is `hub()` and
// `bridge.token`, so those are all the scaffolding needed.
const calls = [];
const answers = {
  "/keys": { keys: [
    { vendor: "anthropic", label: "Anthropic (Claude)", set: true, tail: "f4a2" },
    { vendor: "openai", label: "OpenAI", set: false, tail: "" },
  ]},
  "/spend": { builds: 2, total: { own: 1.28, krate: 0.76 }, recent: [] },
};
const bridge = { token: "krs_abc" };
async function hub(path, opts = {}) {
  calls.push({ path, method: opts.method || "GET", body: opts.body ? JSON.parse(opts.body) : null });
  if (answers[path]) return answers[path];
  return { ok: true };
}
const refuse = (m) => Promise.reject(new Error(m));

// The signed-out half. Signed in, a key must never touch browser storage,
// so these stubs THROW when there is a token; signed out, a key waits here
// until the sign-in every build needs anyway, and account_status moves it
// to the hub (K-781).
let stash = null;
const stashedKey = () => stash;
const stashKey = (vendor, key) => {
  if (bridge.token) throw new Error("a key was written to browser storage while signed in");
  stash = { vendor, key };
};
const clearStash = () => { stash = null; };
const API_VENDOR_LABEL = { anthropic: "Anthropic (Claude)", openai: "OpenAI" };
const builderHealth = async () => ({ ok: true, authoring: "off", agent: "anthropic" });

// The command bodies, lifted verbatim from the file.
function grab(name) {
  const at = src.indexOf(`  async ${name}(`);
  assert.ok(at > 0, `${name} is defined`);
  // to the line that closes the method at two-space indent
  const end = src.indexOf("\n  },\n", at);
  return src.slice(at, end) + "\n  }";
}
const body = ["api_keys", "api_key_set", "api_key_forget", "spend_report"].map(grab).join(",\n");
const COMMANDS = eval(`({\n${body}\n})`);

// A key that is set reads back as set, with something to recognise it by
// and NEVER the key itself.
const keys = await COMMANDS.api_keys();
assert.equal(keys.length, 2);
assert.equal(keys[0].set, true);
assert.match(keys[0].where_kept, /ends f4a2/, "recognisable without being the key");
assert.equal(keys[1].set, false);
assert.ok(!JSON.stringify(keys).includes("sk-"), "no key material reaches the page");

// Saving and forgetting go to the hub, not to the browser.
await COMMANDS.api_key_set({ vendor: "anthropic", key: "sk-ant-secret-key-material-0001" });
const put = calls.find((c) => c.path === "/keys" && c.method === "POST");
assert.ok(put, "the key is sent to the hub");
assert.equal(put.body.key, "sk-ant-secret-key-material-0001");

// A key must LOOK like one before it is kept. Any string at all was
// accepted -- "not-a-real-key-123" was stored and reported as saved -- and
// the person found out when a build failed on the paid path minutes later
// (K-812). Shape only: the vendor is the judge of whether a key is real.
for (const [bad, why] of [
  ["", "empty"],
  ["not-a-real-key-123", "wrong prefix"],
  ["sk-ant-x", "too short"],
]) {
  await assert.rejects(
    () => COMMANDS.api_key_set({ vendor: "anthropic", key: bad }),
    "a " + why + " key is refused before it is kept anywhere",
  );
}
assert.ok(
  !calls.some((c) => c.path === "/keys" && c.method === "POST"
    && String((c.body || {}).key || "").startsWith("not-a-real")),
  "a refused key never reaches the hub",
);

await COMMANDS.api_key_forget({ vendor: "anthropic" });
assert.ok(calls.some((c) => c.path === "/keys/forget"), "forget reaches the hub");

// Nothing was stored in the browser at any point while signed in (the
// stub above throws if it is), and the key itself never went anywhere but
// the hub.
assert.equal(stash, null, "signed in, a key is never written to browser storage");

// Spend comes back with both pockets separated.
const spend = await COMMANDS.spend_report();
assert.equal(spend.total.own, 1.28);
assert.equal(spend.total.krate, 0.76);

// Signed out, nothing is asked of the hub and nothing pretends to be zero
// spend that was really unknown. The pane still gets its ONE row -- the
// vendor the build service runs on -- so there is a field to paste into.
bridge.token = null;
const before = calls.length;
const out = await COMMANDS.api_keys();
assert.equal(calls.length, before, "signed out asks the hub for nothing");
assert.equal(out.length, 1, "signed out still shows the build service's vendor");
assert.equal(out[0].vendor, "anthropic");
assert.equal(out[0].set, false, "no key yet");
// A key pasted signed out waits in this browser, is recognisable by its
// tail and never shown, and can be removed again -- all without the hub.
await COMMANDS.api_key_set({ vendor: "anthropic", key: "sk-ant-later-9z8y-material-0002" });
assert.deepEqual(stash, { vendor: "anthropic", key: "sk-ant-later-9z8y-material-0002" }, "held until sign-in");
const held = await COMMANDS.api_keys();
assert.equal(held[0].set, true);
assert.match(held[0].where_kept, /until you sign in, ends 0002$/);
assert.ok(!JSON.stringify(held).includes("sk-"), "no key material reaches the page");
await COMMANDS.api_key_forget({ vendor: "anthropic" });
assert.equal(stash, null, "forgotten from the browser");
assert.equal(calls.length, before, "none of that touched the hub");
const signedOut = await COMMANDS.spend_report();
assert.equal(signedOut.builds, 0, "signed out, spend is not asked of the hub");

// The spending panel must land in the PANE, not the nav button that opens
// it. Both carry data-ai="keys" and the button comes first in the document,
// so a bare attribute selector picks the wrong one -- which is exactly what
// happened: the panel rendered inside the sheet's left-hand nav column.
const paint = src.slice(src.indexOf("async function paintSpend"));
assert.match(paint.slice(0, 400), /querySelector\('\.ai-pane\[data-ai="keys"\]'\)/,
  "paintSpend targets the pane, not the nav button that shares its attribute");

console.log("ok  the spending panel targets the pane, not the nav button");
console.log("ok  keys go to the hub when signed in, and wait in the browser only until then");
console.log("ok  spend separates your key from Krate's");
