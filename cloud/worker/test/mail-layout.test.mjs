/* The one layout every system mail uses (cloud/worker/src/mail-layout.js):
 * what it promises a mail client, and that what people type stays text.
 *
 *   node --test cloud/worker/test/mail-layout.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { emailHtml, LOGO_URL } from "../src/mail-layout.js";
import { publishedMail, madeMail, firstOpenMail } from "../src/mail.js";
import { inviteMailHtml } from "../src/org-mail.js";
import { assertLayout } from "./mail-layout-tools.mjs";

const hostile = '<img src=x onerror="alert(1)">';

test("the layout: the site's mark, one button, the link written out, dark mode, Outlook's column", () => {
  const html = emailHtml({
    heading: "Sign in to Krate",
    blocks: ["Press the button."],
    button: { label: "Sign in to Krate", url: "https://hub.example/login/email/verify?t=abc&x=1" },
    note: ["If you did not ask, ignore this email."],
  });
  assertLayout(html, "https://hub.example/login/email/verify?t=abc&x=1", "Sign in to Krate");
  assert.equal(LOGO_URL, "https://krate.tech/krate-mark-3d-96.png", "the mark the site's header shows");
  assert.ok(html.includes('<meta name="supported-color-schemes" content="light dark">'));
  assert.ok(html.includes("@media (prefers-color-scheme:dark)"), "the site's dark tokens");
  assert.ok(html.includes('bgcolor="#0d0d0d"'), "the button is a painted cell, which Outlook keeps");
  assert.ok(html.includes('<!--[if mso]><table role="presentation" width="560"'), "a fixed column for Outlook");
  assert.ok(html.includes("font-family:Arial"), "Outlook gets a font it has");
  assert.ok(!/<link\b|@import|@font-face|<script/i.test(html), "nothing to load but the logo");
  assert.ok(!html.includes("\u2014"), "no em dash");
  // Every style the words need is on the element: the <style> block adds
  // dark mode and phone padding only.
  const body = html.slice(html.indexOf("<body")).replace(/<!--\[if mso\]>[\s\S]*?<!\[endif\]-->/g, "");
  for (const tag of body.match(/<(p|h1|td|a)\b[^>]*>/g)) assert.ok(/style="/.test(tag), `inline style on ${tag}`);
});

test("what people type is text, never markup", () => {
  const html = emailHtml({
    heading: `${hostile} is live`,
    blocks: [`Your app ${hostile}`, { quote: hostile }, { rows: [["Name", hostile]] }],
    button: { label: `Open ${hostile}`, url: 'https://hub.example/a/x"onmouseover="alert(1)' },
    note: [hostile],
    footer: [hostile],
  });
  assert.ok(!html.includes("<img src=x"), "no injected tag");
  assert.ok(!html.includes('"onmouseover="'), "no attribute broken out of");
  assert.ok(html.includes("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;"));
});

test("the app mails and the invitation carry the layout, the unsubscribe only where it belongs", () => {
  const url = "https://hub.example/a/abc";
  for (const [mail, link, label] of [
    [publishedMail({ name: "Bounce", url, listed: true }), url, "Open Bounce"],
    [madeMail({ request: "a habit tracker" }), "https://krate.tech/app/", "Open Krate Studio"],
    [firstOpenMail({ name: "Bounce", url }), url, "Open Bounce"],
  ]) {
    const html = emailHtml({ ...mail.layout, title: mail.subject, unsubscribe: "https://hub.example/mail/unsubscribe?u=a&t=b" });
    assertLayout(html, link, label);
    assert.ok(html.includes('href="https://hub.example/mail/unsubscribe?u=a&amp;t=b"'), "a mail nobody asked for can be stopped");
    // The designed copy says what the text says.
    const words = html.replace(/<[^>]+>/g, "");
    for (const line of mail.text.split("\n").filter((l) => l.length > 40)) {
      assert.ok(words.includes(line.trim().replace(/[:.]$/, "").replace(/'/g, "&#39;").replace(/"/g, "&quot;")), line);
    }
  }
  const invite = inviteMailHtml({ inviter: "alice", orgName: "Acme", slug: "acme", role: "developer", joinUrl: "https://krate.tech/join/?c=x", expiresAt: 1800000000, email: "erin@corp.example" });
  assertLayout(invite, "https://krate.tech/join/?c=x", "Join Acme");
  assert.ok(!invite.includes("Unsubscribe"), "an invitation an admin sent is essential: no unsubscribe");
});
