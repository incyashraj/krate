// Shoots og-v3.png -- the link preview card -- from the real landing page.
//
// The preview image used to be drawn separately, which meant it drifted from
// the site every time the site changed. This screenshots index.html instead,
// so the card is the page by construction and cannot be out of date.
//
//   krate/docs/landing $ python3 -m http.server 8765 &
//   krate/docs/landing $ node _og-shoot.mjs
//
// Playwright is not a repo dependency -- this runs by hand when the hero
// changes, not in CI -- so install it wherever you run this from:
//   npm i playwright && npx playwright install chromium
// OG_URL and OG_OUT override the source page and the output path.

import { chromium } from "playwright";

const URL = process.env.OG_URL || "http://localhost:8765/index.html";
const OUT = process.env.OG_OUT || new URL("./og-v3.png", import.meta.url).pathname;
const W = 1200, H = 630;   // the size every platform wants

// The terminal prompt, the last entry in the page's own GLYPHS array. The
// headline cycles through the AI marks; a still card should not advertise
// one vendor, so we pin the agent-neutral one.
const TERMINAL_GLYPH =
  '<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" ' +
  'stroke-linecap="round" stroke-linejoin="round">' +
  '<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>';

const browser = await chromium.launch();
const page = await browser.newPage({
  viewport: { width: W, height: H },
  // 1, not 2: the clip below is in CSS pixels, so a scale factor of 2 would
  // write a 2400x1260 file. The card wants to be exactly 1200x630.
  deviceScaleFactor: 1,
  // The hero fades itself in on scroll. Reduced motion is the page's own
  // switch for "show the finished state", so we ask for it rather than
  // reaching in and overriding the animation by hand.
  reducedMotion: "reduce",
  colorScheme: "dark",
});

await page.goto(URL, { waitUntil: "networkidle" });
await page.evaluate(() => document.fonts.ready);

await page.evaluate((glyph) => {
  // Stop the headline carousel so the shot is deterministic, then pin it.
  for (let i = 1; i < 9999; i++) clearInterval(i);
  const g = document.getElementById("heroGlyph");
  if (g) {
    g.innerHTML = glyph;
    g.classList.remove("out", "pre");
    g.style.cssText = "opacity:1;transform:none;filter:none";
  }

  // Everything below the hero, and the blur bar over it, is not part of the
  // card.
  const hide = ".marqWrap, .marq, .marqLabel, .topblur";
  document.querySelectorAll(hide).forEach((e) =>
    e.style.setProperty("display", "none", "important"));
}, TERMINAL_GLYPH);

// The star pill fetches from GitHub; give it a beat, then settle.
await page.waitForTimeout(1200);

await page.screenshot({ path: OUT, clip: { x: 0, y: 0, width: W, height: H } });
await browser.close();

console.log(`wrote ${OUT} (${W}x${H})`);
