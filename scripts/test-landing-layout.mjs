#!/usr/bin/env node
// Run with Playwright installed, or set KRATE_PLAYWRIGHT_MODULE to its index.mjs.
// Serves only local static assets; CSP prevents production API/analytics calls.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { dirname, extname, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../docs/landing');
const playwright = process.env.KRATE_PLAYWRIGHT_MODULE;
const { chromium } = await import(playwright ? pathToFileURL(resolve(playwright)).href : 'playwright');
const types = {'.html':'text/html', '.png':'image/png', '.webp':'image/webp', '.woff2':'font/woff2', '.css':'text/css', '.js':'text/javascript'};
let fontRequests = 0;
const server = createServer(async (req, res) => {
  try {
    let file = resolve(root, '.' + decodeURIComponent(new URL(req.url, 'http://local').pathname));
    if (file !== root && !file.startsWith(root + sep)) throw new Error('outside root');
    if ((await stat(file)).isDirectory()) file = resolve(file, 'index.html');
    let content = await readFile(file);
    // Same document with the previous font-dependent measure, for comparing
    // fully loaded geometry. No source file is changed by the test.
    if (file === resolve(root,'index.html') && new URL(req.url,'http://local').searchParams.has('baseline'))
      content = Buffer.from(content.toString().replace('max-width: 22.848em;', 'max-width: 34ch;'));
    if (extname(file) === '.woff2') {
      fontRequests++;
      await new Promise(resolve => setTimeout(resolve, 1200));
    }
    res.writeHead(200, {
      'Content-Type': types[extname(file)] || 'application/octet-stream',
      'Cache-Control': extname(file) === '.html' ? 'no-store' : 'public, max-age=3600',
      'Content-Security-Policy': "connect-src 'none'; frame-src 'none'; object-src 'none'",
    });
    res.end(content);
  } catch {
    res.writeHead(404); res.end('Not found');
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({headless:true});
const results = [];
try {
  for (const [width,height] of [[320,740],[360,800],[375,812],[390,1000],[430,932],[760,1000],[768,1000],[1440,1000]]) {
    const context = await browser.newContext({viewport:{width,height}, reducedMotion:'reduce'});
    await context.addInitScript(() => {
      window.layoutShifts = [];
      new PerformanceObserver(list => window.layoutShifts.push(...list.getEntries()
        .filter(entry=>!entry.hadRecentInput).map(entry=>entry.value)))
        .observe({type:'layout-shift', buffered:true});
    });
    const page = await context.newPage();
    for (const cache of ['cold','warm']) {
      const before = fontRequests;
      await page.goto(base, {waitUntil:'load'});
      await page.evaluate(()=>document.fonts.ready);
      await page.waitForTimeout(150);
      const result = await page.evaluate(() => {
        const subtitle = document.querySelector('.heroSubPhone');
        const heading = document.querySelector('h1');
        return {
          shift:window.layoutShifts.reduce((total,value)=>total+value,0),
          subtitleHeight:subtitle.getBoundingClientRect().height,
          scrollWidth:document.documentElement.scrollWidth,
          tagsFit:[...document.querySelectorAll('.tagRow .tag')].every(el=>{
            const box=el.getBoundingClientRect();
            return box.left>=0 && box.right<=innerWidth+1;
          }),
          readable:getComputedStyle(heading).opacity!=='0' && heading.getBoundingClientRect().height>0,
        };
      });
      const entry = {width,height,cache,fontRequests:fontRequests-before,...result};
      results.push(entry);
      assert.ok(result.scrollWidth<=width+1,JSON.stringify(entry));
      assert.equal(result.tagsFit,true,JSON.stringify(entry));
      assert.equal(result.readable,true,JSON.stringify(entry));
      assert.ok(result.shift<0.05,JSON.stringify(entry));
      if(cache==='warm') assert.equal(entry.fontRequests,0,'Warm font responses should come from browser cache');
    }
    const fixedGeometry = await page.evaluate(()=>[...document.querySelectorAll('section,h1,.heroSubPhone,.heroCtas,.shotBox')]
      .map(el=>JSON.stringify(el.getBoundingClientRect())));
    const fixedWidth = await page.evaluate(()=>document.documentElement.scrollWidth);
    await page.waitForTimeout(200);
    const fixedPixels = await page.screenshot({fullPage:true});
    await page.goto(base+'/?baseline=1',{waitUntil:'load'});
    await page.evaluate(()=>document.fonts.ready);
    await page.waitForTimeout(200);
    const originalGeometry = await page.evaluate(()=>[...document.querySelectorAll('section,h1,.heroSubPhone,.heroCtas,.shotBox')]
      .map(el=>JSON.stringify(el.getBoundingClientRect())));
    assert.deepEqual(fixedGeometry,originalGeometry,'Loaded layout must remain unchanged');
    assert.equal(fixedWidth,await page.evaluate(()=>document.documentElement.scrollWidth),'Must not add horizontal overflow');
    assert.ok(fixedPixels.equals(await page.screenshot({fullPage:true})), 'Loaded screenshot must remain pixel-identical');
    await context.close();
  }
  console.log(JSON.stringify({checks:results.length,results},null,2));
} finally {
  await browser.close();
  await new Promise(resolve=>server.close(resolve));
}
