/* What every designed mail must carry (cloud/worker/src/mail-layout.js):
 * the site's mark as a hosted image with its size and alt text, the one
 * button, and the same link written out under it for a client that strips
 * buttons. `url` is the link as the mail's text part has it. */
import assert from "node:assert/strict";

export function assertLayout(html, url, label) {
  assert.ok(html, "an HTML part");
  assert.ok(html.includes('src="https://krate.tech/krate-mark-3d-96.png" width="28" height="28" alt="Krate"'), "the logo");
  assert.ok(html.includes('<meta name="color-scheme" content="light dark">'), "dark mode declared");
  assert.ok(html.includes(`>${label}</a>`), `the button "${label}"`);
  const href = String(url).replace(/&/g, "&amp;");
  assert.equal(html.split(`href="${href}"`).length - 1, 2, "the link on the button and written out under it");
  assert.ok(html.includes(`>${href}</a>`), "the link as text");
}
