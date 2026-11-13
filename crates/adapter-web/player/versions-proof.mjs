// One app, many versions, from a link (IC-976): a link sent at v1 opens
// v2 after an update, and the app's own data is still there. Records Check
// (fixtures/sql-check) packed as 0.1.0 and 0.2.0 -- one app id, two builds
// (cloud/worker/test/fixtures/notes-*.krate) -- published to the hub Worker
// running here in Node, and opened in WebKit as an iPhone the way a person
// opens the link: the phone landing, then "Open it".
//
//   node --experimental-wasm-modules versions-proof.mjs [shot-dir]
//
// It checks, by what the app printed and what the pages said:
//   (a) the app's link opens v1, which counts its first open;
//   (b) after v2 is published, the SAME link opens v2, and the count goes on
//       to 2: the data moved with the app, not with the build;
//   (c) v1's own link still opens v1, and its landing says a newer one exists.
// PLAYWRIGHT_MODULE may point at a playwright already on the machine.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';

const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const shots = process.argv[2] || null;
const build = (v) => new Uint8Array(readFileSync(join(repo, 'cloud', 'worker', 'test', 'fixtures', `notes-${v}.krate`)));
const port = 8934;
const base = `http://127.0.0.1:${port}`;

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

let failed = 0;
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const publish = async (v) => {
  const up = await call(env, 'alice', 'POST', '/publish', { bytes: build(v), headers: { 'x-krate-name': 'Records Check' } });
  if (up.status !== 200) throw new Error(`publish ${v}: ${up.status} ${JSON.stringify(up.body)}`);
  return up.body;
};
const local = (u) => u.replace(/^https?:\/\/[^/]+/, base);

const browser = await engines.webkit.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 390, height: 844 }, userAgent: IPHONE, isMobile: true, hasTouch: true });
// Open a link as a phone does: the landing page, then its "Open it".
const open = async (link, n) => {
  const page = await context.newPage();
  await page.goto(local(link));
  const landing = await page.evaluate(() => document.body.innerText);
  await page.click('a.go');
  await page.waitForFunction(() => window.__krate && window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
  await page.click('#consentAllow');
  await page.waitForFunction(() => /done/.test(window.__krate.out) || window.__krate.trap, null, { timeout: 30000 }).catch(() => {});
  const r = await page.evaluate(() => ({ out: window.__krate.out, trap: window.__krate.trap, url: location.pathname }));
  if (shots) await page.screenshot({ path: join(shots, `versions-proof-${n}.png`) });
  await page.close();
  return { ...r, landing, lines: r.out.split('\n').filter(Boolean) };
};

const v1 = await publish('0.1.0');
const first = await open(v1.url, 'v1');
check(first.lines[0] === 'opens: 1' && first.url.endsWith(v1.id), "(a) the app's link opens v1, which counts its first open", { lines: first.lines, url: first.url, trap: first.trap });

const v2 = await publish('0.2.0');
check(v2.url === v1.url && v2.version === 2, 'v2 is published behind the same link', v2);
const second = await open(v1.url, 'v2-through-the-v1-link');
check(second.url.endsWith(v2.id), '(b) the same link now opens v2', { url: second.url, v2: v2.id });
check(second.lines[0] === 'opens: 2', "(b) and the app's data is still there: the count goes on to 2", second.lines);
check(!/newer version/i.test(second.landing), '(b) the current version says nothing about a newer one', second.landing.slice(0, 200));

const pinned = await open(v1.version_url, 'v1-pinned');
check(pinned.url.endsWith(v1.id), "(c) v1's own link still opens v1", { url: pinned.url });
check(/This is v1\. There is a newer version, v2\./.test(pinned.landing), '(c) and its landing says a newer version exists', pinned.landing.slice(0, 300));
check(pinned.lines[0] === 'opens: 3', '(c) the same app, so the same data', pinned.lines);

await browser.close();
server.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
