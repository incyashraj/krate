/* Which keys the browser keeps when a person types into an app in a tab
 * (K-1196): Cmd/Ctrl+S saves in the app instead of opening the browser's
 * "Save page" over it, and copy, paste and closing the tab stay the
 * browser's.
 *
 *   node --test cloud/worker/test/player-keys.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { browserKeepsKey } from "../../../crates/adapter-web/player/web/ring.js";

const press = (key, mods = {}) => ({ key, ctrlKey: false, metaKey: false, altKey: false, ...mods });

test("plain keys and the app's shortcuts are the app's", () => {
  for (const k of ["a", "Enter", "ArrowLeft", " "]) assert.equal(browserKeepsKey(press(k)), false, k);
  for (const k of ["s", "S", "o", "p", "f", "g", "d", "b", "j", "e", "k", "a", "z", "y"]) {
    assert.equal(browserKeepsKey(press(k, { metaKey: true })), false, `Cmd+${k}`);
    assert.equal(browserKeepsKey(press(k, { ctrlKey: true })), false, `Ctrl+${k}`);
  }
});

test("copy, paste and the tab's own keys stay the browser's", () => {
  for (const k of ["c", "v", "x", "w", "t", "r", "l", "q", "=", "-", "0", "Tab"]) {
    assert.equal(browserKeepsKey(press(k, { metaKey: true })), true, `Cmd+${k}`);
  }
  assert.equal(browserKeepsKey(press("s", { metaKey: true, altKey: true })), true);
});
