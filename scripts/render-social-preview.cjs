#!/usr/bin/env node
// Rebuild with Playwright installed: node scripts/render-social-preview.cjs
// Local fonts and assets only. Keep the output versioned when its copy changes.
const { chromium } = require('playwright');
const { resolve } = require('node:path');
const { pathToFileURL } = require('node:url');
const root = resolve(__dirname, '..');
(async () => {
  const browser = await chromium.launch({headless: true});
  try {
    const page = await browser.newPage({viewport: {width: 1200, height: 630}, deviceScaleFactor: 1});
    const failures = [];
    page.on('pageerror', e => failures.push(e.message));
    page.on('requestfailed', r => failures.push(r.url()));
    await page.route('**/*', route => route.request().url().startsWith('file:')
      ? route.continue() : route.abort());
    await page.goto(pathToFileURL(resolve(root, 'docs/social/preview.html')).href);
    await page.evaluate(() => document.fonts.ready);
    const assetsReady = await page.evaluate(() => [...document.images].every(img => img.complete && img.naturalWidth > 0));
    if (!assetsReady || failures.length) throw new Error(JSON.stringify({assetsReady, failures}));
    const output = resolve(root, 'docs/landing/og-v4.png');
    await page.screenshot({path: output});
    console.log(`Rendered ${output} (1200 x 630)`);
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
