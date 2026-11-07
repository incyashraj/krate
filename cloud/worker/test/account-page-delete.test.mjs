/*
 * The account page's "Your data" card (docs/landing/account/index.html,
 * K-1245) speaks the hub's routes as src/account-delete.js answers them:
 * the export, the confirm step, the deletion with the phrase and the
 * choice about published apps, and the key that carries a long one on.
 * The page itself is proved in a browser against the local stand-in hub
 * (the scratchpad harness); this keeps the two from drifting apart.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/account-page-delete.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { CONFIRM_PHRASE } from "../src/account-delete.js";

const here = dirname(fileURLToPath(import.meta.url));
const page = readFileSync(join(here, "..", "..", "..", "docs", "landing", "account", "index.html"), "utf8");

test("the account page offers the export and deletion, through the hub's routes", () => {
  assert.match(page, /fetch\(HUB \+ "\/me\/export", \{ headers: \{ authorization: "Bearer " \+ tok \} \}\)/, "the export is fetched signed in");
  assert.match(page, /api\("\/me\/delete"\)/, "the confirm step is the hub's own preview");
  assert.match(page, /fetch\(HUB \+ "\/me\/delete", \{\s*method: "POST"/, "the deletion is a POST");
  assert.match(page, /send\(\{ confirm: phrase, apps: pick \? pick\.value : "remove" \}\)/, "it sends the phrase and the apps choice the hub requires");
  assert.match(page, /send\(\{ resume: r\.body\.resume \}\)/, "a running deletion is carried on with its key");
  assert.match(page, /typed\.value\.trim\(\)\.toLowerCase\(\) !== v\.confirm_phrase/, "the button waits for the hub's phrase");
  assert.ok(!page.includes(`"${CONFIRM_PHRASE.toUpperCase()}"`));
  for (const id of ["expBtn", "delOpen", "delBox", "gone"]) assert.ok(page.includes(`id="${id}"`), id);
  assert.match(page, /localStorage\.removeItem\("krate_tok"\)/, "a deleted account is signed out on this browser");
});
