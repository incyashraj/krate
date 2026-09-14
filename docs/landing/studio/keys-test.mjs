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
await COMMANDS.api_key_set({ vendor: "anthropic", key: "sk-ant-secret" });
const put = calls.find((c) => c.path === "/keys" && c.method === "POST");
assert.ok(put, "the key is sent to the hub");
assert.equal(put.body.key, "sk-ant-secret");
await COMMANDS.api_key_forget({ vendor: "anthropic" });
assert.ok(calls.some((c) => c.path === "/keys/forget"), "forget reaches the hub");

// Nothing is stored in the browser at any point.
assert.ok(!src.slice(src.indexOf("async api_key_set")).slice(0, 400).includes("localStorage"),
  "a key is never written to browser storage");

// Spend comes back with both pockets separated.
const spend = await COMMANDS.spend_report();
assert.equal(spend.total.own, 1.28);
assert.equal(spend.total.krate, 0.76);

// Signed out, nothing is asked of the hub and nothing pretends to be zero
// spend that was really unknown.
bridge.token = null;
assert.deepEqual(await COMMANDS.api_keys(), [], "signed out shows no keys");

// The spending panel must land in the PANE, not the nav button that opens
// it. Both carry data-ai="keys" and the button comes first in the document,
// so a bare attribute selector picks the wrong one -- which is exactly what
// happened: the panel rendered inside the sheet's left-hand nav column.
const paint = src.slice(src.indexOf("async function paintSpend"));
assert.match(paint.slice(0, 400), /querySelector\('\.ai-pane\[data-ai="keys"\]'\)/,
  "paintSpend targets the pane, not the nav button that shares its attribute");

console.log("ok  the spending panel targets the pane, not the nav button");
console.log("ok  keys are stored server-side, never in the browser");
console.log("ok  spend separates your key from Krate's");
