// An app's database in the tab (K-1149): krate:store/sql, opened the way a
// person opens a shared link -- Chromium on a computer and as a phone,
// WebKit as an iPhone -- against the hub Worker itself, running here in
// Node over the worker tests' stand-ins, and the Player in
// cloud/worker/player-dist (scripts/build-web-player.sh, which builds
// crates/sql-web into sql.wasm).
//
//   node --experimental-wasm-modules sql-proof.mjs [shot-dir]
//
// The app is Records Check (fixtures/sql-check, evidence/web-player/sql-check.krate).
// Remade with:
//   (cd fixtures/sql-check && cargo component build --release)
//   node pack.mjs fixtures/sql-check/manifest.toml \
//     fixtures/sql-check/target/wasm32-wasip1/release/krate_sql_check.wasm \
//     ../../../evidence/web-player/sql-check.krate
//
// It checks, by what the app printed, opening it twice in one browser:
//   (a) the first open makes its table and counts 1; the second counts 2,
//       so the database outlived the tab;
//   (b) an integer past 2^53 and a blob come back exactly;
//   (c) ATTACH is refused as forbidden, as on desktop;
//   (d) a batch whose last statement fails leaves nothing behind;
//   (e) clearing the app's key-value settings leaves its database;
//   (f) nothing says a database needs a computer, and no console errors.
// PLAYWRIGHT_MODULE and CHROMIUM may point at a playwright and a browser
// already on the machine.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';
import { consoleError } from './engine-abilities.mjs';

const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const shots = process.argv[2] || null;
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

const bytes = new Uint8Array(readFileSync(join(repo, 'evidence', 'web-player', 'sql-check.krate')));
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

let failed = 0;
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const RUNS = [
  ['chromium', 'desktop', { width: 1280, height: 800 }, undefined],
  ['chromium', 'phone', { width: 375, height: 812 }, IPHONE],
  ['webkit', 'iphone', { width: 390, height: 844 }, IPHONE],
];
const launched = {};

for (const [engine, label, viewport, ua] of RUNS) {
  const tag = `[${engine} ${label}]`;
  const browser = (launched[engine] ||= await engines[engine].launch({ headless: true }));
  const context = await browser.newContext({ viewport, userAgent: ua, isMobile: Boolean(ua), hasTouch: Boolean(ua) });
  const open = async (n) => {
    const page = await context.newPage();
    const errors = [];
    page.on('console', (m) => { if (consoleError(m)) errors.push(`${m.text()} @${JSON.stringify(m.location())}`); });
    page.on('pageerror', (e) => errors.push(String(e)));
    await page.goto(`${base}/run/${hash}?probe=1`);
    await page.waitForFunction(() => window.__krate && window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
    const card = await page.evaluate(() => document.body.innerText);
    await page.click('#consentAllow');
    await page.waitForFunction(() => /done/.test(window.__krate.out) || window.__krate.trap, null, { timeout: 30000 }).catch(() => {});
    const r = await page.evaluate(() => ({
      out: window.__krate.out, trap: window.__krate.trap, state: window.__krate.state, notices: window.__krate.notices,
      notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText,
    }));
    // Read before the picture: Playwright's screenshot injects a style that
    // the Player's CSP refuses, which WebKit logs as an error.
    const seen = [...errors];
    if (shots) await page.screenshot({ path: join(shots, `sql-proof-${engine}-${label}-${n}.png`) });
    await page.close();
    return { ...r, card, errors: seen, lines: r.out.split('\n').filter(Boolean) };
  };
  const first = await open(1);
  const second = await open(2);
  // (a)
  check(first.lines[0] === 'opens: 1' && second.lines[0] === 'opens: 2', `${tag} the database outlives the tab: 1 open, then 2`, { first: first.lines, second: second.lines, trap: first.trap || second.trap });
  // (b) - (e)
  for (const [i, run] of [first, second].entries()) {
    const n = i + 1;
    check(run.lines.includes('values: exact'), `${tag} open ${n}: an integer past 2^53 and a blob come back exactly`, run.lines);
    check(run.lines.includes('attach: forbidden'), `${tag} open ${n}: ATTACH is refused as forbidden`, run.lines);
    check(run.lines.includes('batch: invalid-statement, nothing left'), `${tag} open ${n}: a failed batch leaves nothing behind`, run.lines);
    check(run.lines.includes('after kv clear: database kept'), `${tag} open ${n}: clearing settings leaves the database`, run.lines);
    // (f)
    check(!/computer/i.test(run.card) && !run.notices.length && !/computer/i.test(run.notice), `${tag} open ${n}: nothing says a database needs a computer`, { notices: run.notices, notice: run.notice, card: run.card.slice(0, 300) });
    check(run.errors.length === 0 && !run.trap, `${tag} open ${n}: no console errors`, { errors: run.errors, trap: run.trap });
  }
  await context.close();
}

for (const b of Object.values(launched)) await b.close();
server.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
