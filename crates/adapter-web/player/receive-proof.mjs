// The receive loop in a real browser (F-320, K-1132): the hub Worker itself
// runs here in Node over the worker tests' stand-ins, the Player is the one
// in cloud/worker/player-dist (scripts/build-web-player.sh), and Chromium
// opens a shared link the way a person does -- a desktop and a phone.
//
//   node --experimental-wasm-modules receive-proof.mjs [shot-dir]
//
// It checks, by what the page and the hub actually did:
//   - the open is counted once, after the app is on the screen, as kind
//     "open" from client "tab", with the post's ?via= tag;
//   - "Made with Krate / Make your own" is under the app, linking to Studio
//     with the app's hash, and pressing it is counted;
//   - Report sends with no account and lands in the hub's record;
//   - a phone is never told "your computer".
// PLAYWRIGHT_MODULE and CHROMIUM may point at a playwright-core and a
// browser already on the machine.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const shots = process.argv[2] || null;
const port = 8932;
const base = `http://127.0.0.1:${port}`;

const points = [];
const env = orgEnv({
  PUBLIC_BASE: base,
  VISITOR_SALT: 'proof',
  ASSETS: {
    async fetch(request) {
      const name = normalize(new URL(request.url).pathname).replace(/^\/+/, '');
      if (name.includes('..')) return new Response('no', { status: 404 });
      try { return new Response(await readFile(join(dist, name))); } catch { return new Response('not found', { status: 404 }); }
    },
  },
});
env.USAGE = { writeDataPoint: (p) => points.push(p) };

// An app that asks for its own storage, so the permission card shows.
const bytes = new Uint8Array(readFileSync(join(repo, 'evidence', 'ported', 'savings.krate')));
const up = await call(env, 'alice', 'POST', '/publish', { bytes });
if (up.status !== 200) throw new Error(`publish: ${up.status} ${JSON.stringify(up.body)}`);
const hash = up.body.id;

const server = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(', ') : v);
  const request = new Request(base + req.url, { method: req.method, headers, body: req.method === 'GET' || req.method === 'HEAD' ? undefined : Buffer.concat(chunks) });
  const answer = await worker.fetch(request, env, { waitUntil() {} }).catch((e) => new Response(String(e && e.stack), { status: 500 }));
  const out = {};
  answer.headers.forEach((v, k) => { out[k] = v; });
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
}).listen(port, '127.0.0.1');

const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) });
let failed = 0;
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';

for (const [label, viewport, ua] of [['desktop', { width: 1280, height: 800 }, undefined], ['phone', { width: 375, height: 812 }, IPHONE]]) {
  points.length = 0;
  const context = await browser.newContext({ viewport, userAgent: ua, isMobile: Boolean(ua), hasTouch: Boolean(ua) });
  // krate.tech is not this machine: "Make your own" opens a blank answer.
  await context.route('https://krate.tech/**', (route) => route.fulfill({ status: 204, body: '' }));
  const page = await context.newPage();
  await page.goto(`${base}/run/${hash}?via=proof`);
  await page.waitForFunction(() => window.__krate && window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
  const consent = await page.evaluate(() => document.body.innerText);
  check(!points.some((p) => p.blobs[0] === 'open'), `[${label}] nothing is counted before the app is allowed`, points.map((p) => p.blobs[0]));
  await page.click('#consentAllow');
  await page.waitForFunction(() => window.__krate.frames > 0 && window.__krate.counted, null, { timeout: 30000 }).catch(() => {});
  await page.waitForTimeout(300);
  const opens = points.filter((p) => p.blobs[0] === 'open');
  check(opens.length === 1 && opens[0].blobs[2] === 'tab' && opens[0].blobs[7] === hash && opens[0].blobs[10] === 'proof',
    `[${label}] one open, from the tab, for this app, with the post's tag`, opens.map((p) => p.blobs));
  const made = await page.evaluate(() => ({ shown: !document.getElementById('made').hidden, href: document.getElementById('makeOwn').href, text: document.getElementById('made').innerText }));
  check(made.shown && made.href === `https://krate.tech/app/?via=player&from=${hash}` && /Made with Krate/.test(made.text) && /Make your own/.test(made.text),
    `[${label}] Made with Krate, Make your own, linking to Studio with the app`, made);
  const frame = await page.evaluate(() => { const f = document.getElementById('frame').getBoundingClientRect(); const m = document.getElementById('made').getBoundingClientRect(); return { frameBottom: f.bottom, madeTop: m.top, scroll: document.documentElement.scrollWidth, width: innerWidth }; });
  check(frame.frameBottom <= frame.madeTop + 0.5 && frame.scroll <= frame.width, `[${label}] the line never covers the app, and nothing scrolls sideways`, frame);
  const [popup] = await Promise.all([context.waitForEvent('page', { timeout: 5000 }).catch(() => null), page.click('#makeOwn')]);
  await page.waitForTimeout(300);
  check(points.some((p) => p.blobs[0] === 'badge' && p.blobs[7] === hash), `[${label}] pressing Make your own is counted`, points.map((p) => p.blobs[0]));
  if (popup) await popup.close();
  await page.click('#reportOpen');
  await page.click('input[value="scam"]');
  await page.fill('#reportNote', 'proof');
  await page.click('#reportSend');
  await page.waitForFunction(() => window.__krate.reported, null, { timeout: 5000 }).catch(() => {});
  const record = JSON.parse(env._kv.get(`app-report:${hash}`) || 'null');
  check(record && record.reports.some((r) => r.why === 'scam' && r.note === 'proof'), `[${label}] Report sends with no account and reaches the hub's record`, record);
  if (shots) await page.screenshot({ path: join(shots, `receive-proof-${label}.png`) });
  if (label === 'phone') {
    const all = consent + (await page.evaluate(() => document.body.innerText));
    check(!/your computer/i.test(all), '[phone] nothing on the page says "your computer"', all.match(/.{0,40}your computer.{0,20}/i));
    check(/It runs in this tab and stops when you close it\./.test(consent), '[phone] the card says it runs in this tab and stops when it is closed', consent.slice(0, 300));
  }
  env._kv.delete(`app-report:${hash}`);
  await context.close();
}
await browser.close();
server.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
