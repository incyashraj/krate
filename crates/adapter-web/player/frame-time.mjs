// How fast a real app runs in a tab: frames per second and what each frame
// costs the page to put on screen, per engine and screen.
//
//   node --experimental-wasm-modules frame-time.mjs <app.krate>... [--seconds 5]
//
// Publishes each app to the hub's own code in this process (as the proofs
// do), opens it in Chromium as a desktop at 2x and in WebKit as an iPhone
// at 3x, lets it run, and reports: frames shown per second, the page's blit
// (the copy out of shared memory plus putImageData) median and p95, and the
// frame's size. A measurement aid, not a proof: it prints, it never fails.
//
// KRATE_PLAYER_DIST points it at another built Player; one whose
// app-worker.js prints "#render <ms>" around ui.render (stderr, so it lands
// in __krate.out) adds the worker's own paint time to each line. How the
// 2026-11-04 numbers were taken: worker paint 11-19 ms -> 2-8 ms for
// Trace, Query and Diff, 3.5-12 -> 0.7-1.4 ms for Nova (IC-290).
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize, basename } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';

const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const dist = process.env.KRATE_PLAYER_DIST || join(here, '..', '..', '..', 'cloud', 'worker', 'player-dist');
const args = process.argv.slice(2);
const secIdx = args.indexOf('--seconds');
const seconds = secIdx >= 0 ? Number(args.splice(secIdx, 2)[1]) : 5;
const apps = args;
const port = 8941;
const base = `http://127.0.0.1:${port}`;

const env = orgEnv({
  PUBLIC_BASE: base,
  VISITOR_SALT: 'frame-time',
  ASSETS: {
    async fetch(request) {
      const name = normalize(new URL(request.url).pathname).replace(/^\/+/, '');
      if (name.includes('..')) return new Response('no', { status: 404 });
      try { return new Response(await readFile(join(dist, name))); } catch { return new Response('not found', { status: 404 }); }
    },
  },
});
env.USAGE = { writeDataPoint() {} };
const server = http.createServer(async (req, res) => {
  const chunks = [];
  // A request the browser abandons as the proof closes it ends "aborted";
  // uncaught, that ended the whole proof after every check had passed.
  try { for await (const c of req) chunks.push(c); } catch { return; }
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(', ') : v);
  const request = new Request(base + req.url, { method: req.method, headers, body: req.method === 'GET' || req.method === 'HEAD' ? undefined : Buffer.concat(chunks) });
  const answer = await worker.fetch(request, env, { waitUntil() {} }).catch((e) => new Response(String(e && e.stack), { status: 500 }));
  const out = {};
  answer.headers.forEach((v, k) => { out[k] = v; });
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
}).listen(port, '127.0.0.1');

const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const screens = [
  { label: 'chromium desktop 2x', engine: 'chromium', ctx: { viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 } },
  // Chromium as a phone: the finger drag below is measured here, because
  // WebKit gives a synthetic PointerEvent a wrong offsetX (203 for 52).
  { label: 'chromium phone 3x', engine: 'chromium', ctx: { viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, userAgent: IPHONE, isMobile: true, hasTouch: true } },
  { label: 'webkit iphone 3x', engine: 'webkit', ctx: { viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, userAgent: IPHONE, isMobile: true, hasTouch: true } },
];

// Times every putImageData and the copy just before it (the Player does
// scratch.set then putImageData inside one blit), from the page's side.
const INSTRUMENT = () => {
  const times = [];
  const real = CanvasRenderingContext2D.prototype.putImageData;
  CanvasRenderingContext2D.prototype.putImageData = function (img, ...rest) {
    const t = performance.now();
    real.call(this, img, ...rest);
    times.push({ put: performance.now() - t, px: img.width * img.height });
  };
  const realSet = Uint8ClampedArray.prototype.set;
  window.__copies = [];
  Uint8ClampedArray.prototype.set = function (src, off) {
    const t = performance.now();
    realSet.call(this, src, off);
    if (this.length > 100000) window.__copies.push(performance.now() - t);
  };
  window.__puts = times;
};
const pct = (xs, p) => { if (!xs.length) return NaN; const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(p * s.length))]; };

for (const path of apps) {
  const bytes = new Uint8Array(readFileSync(path));
  const up = await call(env, 'alice', 'POST', '/publish', { bytes, headers: { 'x-krate-name': basename(path, '.krate') } });
  if (up.status !== 200) { console.log(`${basename(path)} | publish refused ${up.status} ${JSON.stringify(up.body)}`); continue; }
  const link = `${base}/run/${up.body.id}?probe=1`;
  for (const s of screens) {
    const browser = await engines[s.engine].launch({ headless: true });
    const page = await (await browser.newContext(s.ctx)).newPage();
    await page.addInitScript(INSTRUMENT);
    await page.goto(link);
    for (let i = 0; i < 40 && (await page.evaluate(() => window.__krate.state)) !== 'consent'; i++) await page.waitForTimeout(100);
    if ((await page.evaluate(() => window.__krate.state)) === 'consent') await page.click('#consentAllow');
    await page.waitForTimeout(1500);
    const before = await page.evaluate(() => ({ frames: window.__krate.frames, puts: window.__puts.length, copies: window.__copies.length }));
    await page.mouse.move(200, 200);
    await page.waitForTimeout(seconds * 1000);
    // What a person feels: scroll and click, then the input-to-screen time
    // the Player itself records (worker painted, page shown).
    const take = async (act) => {
      const n = await page.evaluate(() => window.__krate.latencies.length);
      for (let i = 0; i < 10; i++) { await act(i); await page.waitForTimeout(150); }
      await page.waitForTimeout(400);
      return page.evaluate((n) => window.__krate.latencies.slice(n), n);
    };
    // A tap or click, then a scroll: kept apart, because a scroll can ease
    // over many frames and a tap is one answer.
    const feel = {
      tap: await take((i) => (s.ctx.isMobile ? page.touchscreen.tap(120 + (i % 3) * 40, 300) : page.mouse.click(300 + (i % 3) * 40, 300))),
      // Mobile WebKit has no wheel: a phone scrolls by touch (the drag below).
      scroll: s.engine === 'webkit' && s.ctx.isMobile ? [] : await take((i) => page.mouse.wheel(0, i % 2 ? -240 : 240)),
    };
    // A finger dragging up the screen and back, at 60 Hz, through the
    // browser's own touch input (Chromium's DevTools protocol), so the
    // Player sees a real finger: a synthetic PointerEvent has no live
    // pointer, setPointerCapture refuses it, and WebKit gives it a wrong
    // offsetX. Frames shown during the drag, and each move's time to the
    // screen.
    let drag = null;
    if (s.ctx.isMobile && s.engine === 'chromium') {
      const cdp = await page.context().newCDPSession(page);
      const box = await page.evaluate(() => { const r = document.getElementById('screen').getBoundingClientRect(); return { x: r.left + r.width / 2, top: r.top, h: r.height }; });
      const finger = (type, y) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x: box.x, y, id: 1 }] });
      const n0 = await page.evaluate(() => window.__krate.latencies.length);
      const f0 = await page.evaluate(() => window.__krate.frames);
      const t0 = Date.now();
      for (const [from, to] of [[box.top + box.h * 0.75, box.top + box.h * 0.25], [box.top + box.h * 0.25, box.top + box.h * 0.75]]) {
        await finger('touchStart', from);
        for (let i = 1; i <= 20; i++) { await page.waitForTimeout(16); await finger('touchMove', from + ((to - from) * i) / 20); }
        await finger('touchEnd');
        await page.waitForTimeout(16);
      }
      await page.waitForTimeout(300);
      drag = await page.evaluate(({ n0, f0 }) => ({ shown: window.__krate.latencies.slice(n0), frames: window.__krate.frames - f0 }), { n0, f0 });
      drag.ms = Date.now() - t0;
    }
    const k = await page.evaluate((b) => ({
      state: window.__krate.state, marks: window.__krate.marks, frames: window.__krate.frames - b.frames,
      puts: window.__puts.slice(b.puts), copies: window.__copies.slice(b.copies),
    }), before);
    if (process.env.KRATE_SHOW_MARKS) {
      const m = await page.evaluate(() => ({ marks: window.__krate.marks, worker: window.__krate.workerMarks, origin: performance.timeOrigin }));
      if (m.worker && m.marks.firstFrame !== undefined) {
        const w = m.worker, at = (t) => m.origin + t;
        console.log(`  ${basename(path)} ${s.label} after the worker starts: player ${Math.round(w.playerReady - w.workerStart)} ms, ` +
          `glue ${Math.round(w.glueLoaded - w.workerStart)}, instantiated ${Math.round(w.instantiated - w.workerStart)}, ` +
          `first frame on screen ${Math.round(at(m.marks.firstFrame) - w.workerStart)} (page asked for the worker ${Math.round(w.workerStart - at(m.marks.workerStarted))} ms before it started)`);
      }
      const line = (o) => Object.entries(o || {}).sort((a, b) => a[1] - b[1]).map(([n, t]) => `${n} ${Math.round(t)}`).join(', ');
      console.log(`  ${basename(path)} ${s.label} page: ${line(m.marks)}`);
      console.log(`  ${basename(path)} ${s.label} worker: ${line(m.worker)}`);
      const out = (await page.evaluate(() => window.__krate.out)) || '';
      console.log(`  ${basename(path)} ${s.label} first paints: ${[...out.matchAll(/#render ([0-9.]+)/g)].slice(0, 3).map((x) => x[1]).join(', ')} ms`);
      const cp = [...out.matchAll(/#cpresent ([0-9.]+)/g)].map((x) => Number(x[1]));
      if (cp.length) console.log(`  ${basename(path)} ${s.label} canvas presents: first ${cp.slice(0, 3).join(', ')} ms, ${cp.length} in all, p50 ${pct(cp, 0.5).toFixed(1)} ms`);
    }
    const put = k.puts.map((p) => p.put);
    // With KRATE_PLAYER_DIST pointing at a copy whose worker prints
    // "#render <ms>" per paint, the worker's own paint time.
    // Copying only changed rows must leave the canvas equal to the frame.
    await page.waitForTimeout(300);
    const stale = await page.evaluate(() => (window.__krate.canvasDiffersFromFrame ? window.__krate.canvasDiffersFromFrame() : null));
    const render = [...((await page.evaluate(() => window.__krate.out)) || '').matchAll(/#render ([0-9.]+)/g)].map((m) => Number(m[1]));
    const px = k.puts.length ? k.puts[k.puts.length - 1].px : 0;
    console.log(`${basename(path)} | ${s.label} | state ${k.state} | first frame ${Math.round((k.marks.firstFrame ?? NaN) - (k.marks.start ?? 0))} ms | ` +
      `${(k.frames / seconds).toFixed(1)} frames/s | frame ${(px * 4 / 1e6).toFixed(1)} MB | ` +
      `copy p50 ${pct(k.copies, 0.5).toFixed(2)} p95 ${pct(k.copies, 0.95).toFixed(2)} ms | putImageData p50 ${pct(put, 0.5).toFixed(2)} p95 ${pct(put, 0.95).toFixed(2)} ms | ` +
      (render.length ? `worker paint p50 ${pct(render, 0.5).toFixed(1)} p95 ${pct(render, 0.95).toFixed(1)} ms (${render.length}) | ` : '') +
      `canvas vs frame ${stale === null ? 'n/a' : `${stale} bytes differ`} | ` +
      `tap to screen p50 ${pct(feel.tap, 0.5).toFixed(1)} p95 ${pct(feel.tap, 0.95).toFixed(1)} ms (${feel.tap.length}) | ` +
      `scroll to screen p50 ${pct(feel.scroll, 0.5).toFixed(1)} p95 ${pct(feel.scroll, 0.95).toFixed(1)} ms (${feel.scroll.length})` +
      (drag ? ` | finger drag ${(drag.frames / (drag.ms / 1000)).toFixed(0)} frames/s, move to screen p50 ${pct(drag.shown, 0.5).toFixed(1)} p95 ${pct(drag.shown, 0.95).toFixed(1)} ms (${drag.shown.length})` : ''));
    await browser.close();
  }
}
server.close();
