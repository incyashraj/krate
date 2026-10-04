#!/usr/bin/env node
// Every page on the 2026-10 design kit, with JavaScript switched off: the
// words a person came to read must be on screen. A reveal-on-scroll or a
// rise that waits for a script would leave them invisible to a reader with
// scripts off, and nothing else would notice.
//
// Only fades count (opacity, visibility): an element switched off with
// display: none is a state the page chose, such as a loader or a button its
// script shows later. Text inside an illustration (an element marked
// data-illustration, or aria-hidden) is exempt: those are pictures drawn in HTML, animated by the
// page's script, and the words they show are repeated in the page's text.
//
// Run with Playwright installed, or set KRATE_PLAYWRIGHT_MODULE to a module
// exporting `chromium` (scripts/site-gate.sh does when one is available).
// Serves docs/landing locally; no live service is called.
import { createServer } from 'node:http';
import { readFile, stat, readdir } from 'node:fs/promises';
import { dirname, extname, resolve, sep, relative } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../docs/landing');
const mod = process.env.KRATE_PLAYWRIGHT_MODULE;
const { chromium } = await import(mod ? pathToFileURL(resolve(mod)).href : 'playwright');
const types = { '.html': 'text/html', '.png': 'image/png', '.webp': 'image/webp', '.woff2': 'font/woff2', '.css': 'text/css', '.js': 'text/javascript' };

const server = createServer(async (req, res) => {
  try {
    let file = resolve(root, '.' + decodeURIComponent(new URL(req.url, 'http://local').pathname));
    if (file !== root && !file.startsWith(root + sep)) throw new Error('outside root');
    if ((await stat(file)).isDirectory()) file = resolve(file, 'index.html');
    res.writeHead(200, { 'Content-Type': types[extname(file)] || 'application/octet-stream' });
    res.end(await readFile(file));
  } catch { res.writeHead(404); res.end(); }
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}`;

async function kitPages(dir) {
  const out = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = resolve(dir, e.name);
    if (e.isDirectory()) out.push(...await kitPages(p));
    else if (e.name.endsWith('.html')) {
      const t = await readFile(p, 'utf8');
      if (/<link[^>]+href="[^"]*\/kit\/site\.css"/.test(t)) out.push('/' + relative(root, p).split(sep).join('/').replace(/index\.html$/, ''));
    }
  }
  return out;
}
const pages = await kitPages(root);
if (pages.length < 2) { console.error('found fewer than two kit pages; the finder is broken'); process.exit(1); }

const browser = await chromium.launch();
let bad = 0;
for (const theme of ['light', 'dark']) {
  const ctx = await browser.newContext({ javaScriptEnabled: false, viewport: { width: 1280, height: 900 }, colorScheme: theme });
  await ctx.route(/^https?:\/\/(?!127\.0\.0\.1)/, r => r.abort());
  for (const path of pages) {
    const page = await ctx.newPage();
    await page.goto(base + path, { waitUntil: 'load' });
    await page.waitForTimeout(1300); // CSS-only first-paint animations finish without a script
    const hidden = await page.evaluate(() => {
      const out = [];
      const main = document.querySelector('main') || document.body;
      for (const el of main.querySelectorAll('h1,h2,h3,h4,p,li,dt,dd,td,th,summary,a,button,label')) {
        const text = el.textContent.replace(/\s+/g, ' ').trim();
        if (!text || el.closest('[data-illustration],[aria-hidden="true"],template,noscript,[hidden],details:not([open]) > :not(summary)')) continue;
        let n = el, why = '';
        for (; n && n !== document.documentElement; n = n.parentElement) {
          const cs = getComputedStyle(n);
          // display: none is a state (a loader, a button a script reveals
          // later), not a reveal waiting for a script; only fades are.
          if (cs.display === 'none') { why = ''; n = null; break; }
          if (cs.visibility === 'hidden') { why = 'visibility: hidden'; break; }
          if (parseFloat(cs.opacity) < 0.05) { why = 'opacity ' + cs.opacity; break; }
        }
        if (why && !(n && n.closest && n.closest('.mnav,.drawer'))) out.push(`${el.tagName.toLowerCase()} "${text.slice(0, 50)}" hidden by ${n.tagName.toLowerCase()}${n.className ? '.' + String(n.className).split(' ').join('.') : ''} (${why})`);
      }
      return out;
    });
    if (hidden.length) { bad += hidden.length; console.log(`FAIL ${path} (${theme}, scripts off)`); hidden.slice(0, 12).forEach(h => console.log('   ', h)); }
    await page.close();
  }
  await ctx.close();
}
await browser.close();
server.close();
console.log(bad ? `${bad} piece(s) of text invisible without JavaScript` : `ok  ${pages.length} kit pages read without JavaScript, light and dark`);
process.exit(bad ? 1 : 0);
