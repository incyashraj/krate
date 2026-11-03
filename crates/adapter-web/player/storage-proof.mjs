// An app published by someone else, declaring the same manifest app id, must
// not open another publisher's storage in the tab (security review, 2026-11-03).
// Before the fix it read store.secret and store.shared: the Player keyed
// storage by the app id alone, which the author picks, on one shared origin.
//
// alice publishes Share Check (dev.krate.share-check). mallory publishes the
// same component under a different name ("Free Puzzle Game") with the same
// app id. One browser: the person opens alice's app (it keeps a sign-in token
// in store.secret and makes a shared list), then opens mallory's link.
//
//   node --experimental-wasm-modules storage-proof.mjs
//
// Exit 1 when mallory's app reads alice's token or share code, or alice's own
// next open loses them. PLAYWRIGHT_MODULE may point at a playwright.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { join, normalize } from 'node:path';

const repo = join(new URL('.', import.meta.url).pathname, '..', '..', '..');
const { orgEnv, call, worker } = await import(join(repo, 'cloud/worker/test/org-tools.mjs'));
const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const dist = process.env.DIST || join(repo, 'cloud', 'worker', 'player-dist');
const port = 8951;
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

const publish = async (who, file) => {
  const up = await call(env, who, 'POST', '/publish', { bytes: new Uint8Array(readFileSync(file)) });
  if (up.status !== 200) throw new Error(`publish ${file}: ${up.status} ${JSON.stringify(up.body)}`);
  return up.body.id;
};
const victim = await publish('alice', join(repo, 'evidence/web-player/share-check.krate'));
// Share Check's own component, packed under another name with the same app id.
const impostor = await publish('mallory', join(repo, 'evidence/web-player/share-check-impostor.krate'));

const server = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(', ') : v);
  const request = new Request(base + req.url, { method: req.method, headers, body: req.method === 'GET' || req.method === 'HEAD' ? undefined : Buffer.concat(chunks) });
  const answer = await worker.fetch(request, env, { waitUntil() {} }).catch((e) => new Response(String(e && e.stack), { status: 500 }));
  const out = {};
  answer.headers.forEach((v, k) => { out[k] = v; });
  out['cache-control'] = 'no-store';
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
}).listen(port, '127.0.0.1');

async function launch(context, hash, arg) {
  const page = await context.newPage();
  await page.goto(`${base}/run/${hash}?probe=1&arg=${encodeURIComponent(arg)}`);
  await page.waitForFunction(() => window.__krate && (window.__krate.state === 'consent' || window.__krate.state === 'app'), null, { timeout: 30000 }).catch(() => {});
  if (await page.evaluate(() => window.__krate.state === 'consent')) await page.click('#consentAllow');
  await page.waitForFunction(() => /(^|\n)ready\n/.test(window.__krate.out) || window.__krate.trap, null, { timeout: 30000 }).catch(() => {});
  const lines = await page.evaluate(() => window.__krate.out.split('\n').filter(Boolean));
  const title = await page.title();
  await page.close();
  return { lines, title };
}

let leaked = false;
const engine = process.env.ENGINE || 'chromium';
const browser = await engines[engine].launch({ headless: true });
const ctx = await browser.newContext();
const a = await launch(ctx, victim, 'create');
const code = (a.lines.find((l) => l.startsWith('created: ')) || '').slice('created: '.length);
console.log(`alice's Share Check (${victim.slice(0, 12)}): token set: ${a.lines.includes('token set: ok')}, share code ${code}`);
const m = await launch(ctx, impostor, 'reopen');
console.log(`mallory's "${m.title}" (${impostor.slice(0, 12)}) printed:`, m.lines.filter((l) => /code at start|token|names/.test(l)));
if (m.lines.includes('token: kept')) { leaked = true; console.log('HOLE: mallory\'s app opened alice\'s sealed secret'); }
if (code && m.lines.includes(`code at start: ${code}`)) { leaked = true; console.log('HOLE: mallory\'s app holds alice\'s share code (and so her shared list)'); }
if (!leaked) console.log('ok: mallory\'s app sees none of alice\'s storage');
const back = await launch(ctx, victim, 'reopen');
const keeps = back.lines.includes('token: kept') && back.lines.includes(`code at start: ${code}`);
console.log(`alice's own next open: ${keeps ? 'keeps her token and share code' : 'LOST HER DATA'}`, back.lines.filter((l) => /code at start|token:/.test(l)));
if (!keeps) leaked = true;
await browser.close();
server.close();
process.exit(leaked ? 1 : 0);
