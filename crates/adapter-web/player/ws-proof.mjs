// Live connections in the tab (IC-974): krate:net/ws, opened in Chromium the
// way a person opens a shared link -- a desktop and a phone -- against the
// hub Worker itself, running here in Node over the worker tests' stand-ins,
// the Player in cloud/worker/player-dist (scripts/build-web-player.sh), and
// a WebSocket echo server on this machine.
//
//   node --experimental-wasm-modules ws-proof.mjs [shot-dir]
//
// The app is Live Check (fixtures/ws-check, evidence/web-player/ws-check.krate),
// granted net.connect:127.0.0.1:8935. Remade with:
//   (cd fixtures/ws-check && cargo component build --release)
//   node pack.mjs fixtures/ws-check/manifest.toml \
//     fixtures/ws-check/target/wasm32-wasip1/release/krate_ws_check.wasm \
//     ../../../evidence/web-player/ws-check.krate
//
// It checks, by what the app printed and what the server saw:
//   (a) a host the app was never granted is refused at open with
//       permission-denied, and nothing reaches it;
//   (b) the granted echo server: text and binary sent before the handshake
//       finished arrive in order, come back in order, byte for byte, and a
//       polite close reports closed;
//   (c) the granted port, a path the server hangs up on: a handle, then
//       failed -- never pending forever;
//   (d) no notice says live connections need a computer, no console errors,
//       and the app keeps drawing.
// PLAYWRIGHT_MODULE and CHROMIUM may point at a playwright and a browser
// already on the machine.
import http from 'node:http';
import { createRequire } from 'node:module';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const { WebSocketServer } = createRequire(join(repo, 'cloud', 'worker', 'package.json'))('ws');
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

const bytes = new Uint8Array(readFileSync(join(repo, 'evidence', 'web-player', 'ws-check.krate')));
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

// The echo server: /echo sends every message back as it came; anything
// else is hung up on before the handshake.
const seen = [];
const echoHttp = http.createServer((_, res) => { res.writeHead(404); res.end(); }).listen(8935, '127.0.0.1');
const wss = new WebSocketServer({ noServer: true });
echoHttp.on('upgrade', (req, socket, head) => {
  if (req.url !== '/echo') { seen.push({ refused: req.url }); socket.destroy(); return; }
  wss.handleUpgrade(req, socket, head, (ws) => {
    seen.push({ opened: req.url, origin: req.headers.origin });
    ws.on('message', (data, isBinary) => {
      seen.push(isBinary ? { binary: [...data] } : { text: data.toString() });
      ws.send(data, { binary: isBinary });
    });
    ws.on('close', (code) => seen.push({ closed: code }));
  });
});

const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) });
let failed = 0;
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const EXPECTED = [
  'elsewhere: refused permission-denied',
  'echo: opened',
  'echo: text hello from a tab',
  'echo: binary 1 2 3 250 0',
  'echo: closed',
];

for (const [label, viewport, ua] of [['desktop', { width: 1280, height: 800 }, undefined], ['phone', { width: 375, height: 812 }, IPHONE]]) {
  const tag = `[${label}]`;
  seen.length = 0;
  const context = await browser.newContext({ viewport, userAgent: ua, isMobile: Boolean(ua), hasTouch: Boolean(ua) });
  // Anything the app reaches besides this machine is recorded: (a) says none.
  const outside = [];
  await context.route(/^(?!http:\/\/127\.0\.0\.1).*/, (route) => { outside.push(route.request().url()); route.abort(); });
  const page = await context.newPage();
  const errors = [];
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', (e) => errors.push(String(e)));
  const sockets = [];
  page.on('websocket', (w) => sockets.push(w.url()));
  await page.goto(`${base}/run/${hash}?probe=1`);
  await page.waitForFunction(() => window.__krate && window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
  const card = await page.evaluate(() => document.body.innerText);
  check(!/computer/i.test(card), `${tag} the card does not send the live connection to a computer`, card.slice(0, 400));
  await page.click('#consentAllow');
  await page.waitForFunction(() => /echo: (closed|failed)/.test(window.__krate.out) && /dead: /.test(window.__krate.out), null, { timeout: 20000 }).catch(() => {});
  await page.waitForTimeout(400);
  const r = await page.evaluate(() => ({
    out: window.__krate.out,
    frames: window.__krate.frames,
    trap: window.__krate.trap,
    state: window.__krate.state,
    notices: window.__krate.notices,
    notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText,
  }));
  const lines = r.out.split('\n').filter(Boolean);
  // (a)
  check(lines[0] === EXPECTED[0], `${tag} a host never granted is refused at open with permission-denied`, lines);
  check(!sockets.some((u) => /example\.com/.test(u)) && !outside.some((u) => /example\.com/.test(u)), `${tag} nothing reaches the host it was not granted`, { sockets, outside });
  // (b)
  const echo = lines.filter((l) => l.startsWith('echo: '));
  check(JSON.stringify(echo) === JSON.stringify(EXPECTED.slice(1)), `${tag} text and binary sent before the handshake come back in order, byte for byte, then closed`, echo);
  const got = seen.filter((s) => s.text || s.binary);
  check(got.length === 2 && got[0].text === 'hello from a tab' && JSON.stringify(got[1].binary) === '[1,2,3,250,0]', `${tag} the server got both messages, in order, as sent`, seen);
  check(seen.some((s) => s.closed === 1000), `${tag} the close is a polite one (1000)`, seen);
  check(seen.some((s) => s.opened && s.origin === base), `${tag} the connection comes from the Player's own origin`, seen);
  // (c)
  check(lines.includes('dead: failed') && seen.some((s) => s.refused === '/refuse'), `${tag} a connection the server hangs up on reports failed, not pending forever`, { lines, seen });
  // (d)
  await page.waitForTimeout(500);
  const later = await page.evaluate(() => window.__krate.frames);
  check(r.state === 'app' && !r.trap && later > r.frames, `${tag} the app keeps drawing`, { state: r.state, trap: r.trap, frames: r.frames, later });
  check(!r.notices.some((n) => /net/.test(n.iface)) && !/computer/i.test(r.notice), `${tag} no notice says live connections need a computer`, { notices: r.notices, notice: r.notice });
  // The browser logs the connection (c) fails on purpose; nothing else.
  const others = errors.filter((e) => !/ws:\/\/127\.0\.0\.1:8935\/refuse/.test(e));
  check(others.length === 0, `${tag} no console errors besides the browser's line for (c)`, others);
  if (shots) await page.screenshot({ path: join(shots, `ws-proof-${label}.png`) });
  await context.close();
}

await browser.close();
server.close();
wss.close();
echoHttp.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
