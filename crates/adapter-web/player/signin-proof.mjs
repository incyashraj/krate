// An organisation's app opened from its own link (/run/org/<slug>/<app>)
// by someone signed out must offer the ways that team really signs in
// (K-1165): the company's own sign-in, an emailed link, then GitHub and
// Google. Before, the tab offered only GitHub and Google, so a member
// invited by work email or signing in through Okta, Entra or SAML could
// not open the app in a tab at all -- phones included.
//
// The emailed link opens in a new tab, so the proof of "this browser asked"
// (K-909) must reach that tab; a finished sign-in this browser did not ask
// for must still be refused.
//
//   node --experimental-wasm-modules signin-proof.mjs
//
// The real hub Worker runs in Node over the worker tests' stand-ins; the
// mail service is answered here and the link it was handed is opened.
// Exit 1 on any FAIL. PLAYWRIGHT_MODULE may point at a playwright.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { join, normalize } from 'node:path';
import { consoleError } from './engine-abilities.mjs';

const repo = join(new URL('.', import.meta.url).pathname, '..', '..', '..');
const { orgEnv, worker } = await import(join(repo, 'cloud/worker/test/org-tools.mjs'));
const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const dist = process.env.DIST || join(repo, 'cloud', 'worker', 'player-dist');
const port = 8952;
const base = `http://127.0.0.1:${port}`;
const SLUG = 'acme';
const APP = 'dev.krate.timesheet';
const ADDRESS = `/run/org/${SLUG}/${APP}`;

const env = orgEnv({
  PUBLIC_BASE: base,
  VISITOR_SALT: 'proof',
  RESEND_API_KEY: 're_proof',
  ASSETS: {
    async fetch(request) {
      const name = normalize(new URL(request.url).pathname).replace(/^\/+/, '');
      if (name.includes('..')) return new Response('no', { status: 404 });
      try { return new Response(await readFile(join(dist, name))); } catch { return new Response('not found', { status: 404 }); }
    },
  },
});
env.USAGE = { writeDataPoint() {} };

// The mail service: every mail the hub sends is kept, never sent.
const mails = [];
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init) => {
  const url = typeof input === 'string' ? input : input.url;
  if (url.startsWith('https://api.resend.com/')) {
    mails.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ id: `mail-${mails.length}` }), { status: 200, headers: { 'content-type': 'application/json' } });
  }
  return realFetch(input, init);
};

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
  out['cache-control'] = 'no-store';
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
}).listen(port, '127.0.0.1');

let failed = 0;
function check(tag, ok, name, detail) {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${tag} ${name}${ok || detail === undefined ? '' : ` ${JSON.stringify(detail)}`}`);
}
const state = (page) => page.evaluate(() => window.__krate && window.__krate.state);
const until = (page, fn) => page.waitForFunction(fn, null, { timeout: 20000 }).catch(() => {});

const profiles = [
  ['chromium', 'desktop', {}],
  ['chromium', 'phone', { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true }],
  ['webkit', 'iphone', { viewport: { width: 390, height: 844 }, hasTouch: true, userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1' }],
];
const launched = {};
for (const [engine, label, opts] of profiles) {
  const tag = `[${engine} ${label}]`;
  const browser = (launched[engine] ||= await engines[engine].launch({ headless: true }));
  const context = await browser.newContext(opts);
  const errors = [];
  context.on('weberror', (e) => errors.push(String(e.error())));
  // Failed loads by address: the browser's console line names none. The
  // one expected is the team's app list after signing in -- this proof
  // makes no organisation, so the hub answers that it has no such app.
  const expected404 = (url) => new URL(url).pathname.startsWith(`/orgs/${SLUG}/apps/`);
  context.on('response', (r) => { if (r.status() >= 400 && !expected404(r.url())) errors.push(`${r.status()} ${r.url()}`); });
  const fromPage = (m) => consoleError(m) && !/^Failed to load resource/.test(m.text());
  const page = await context.newPage();
  page.on('console', (m) => { if (fromPage(m)) errors.push(m.text()); });

  // 1. Signed out, the org app's own link asks how this team signs in.
  await page.goto(base + ADDRESS);
  await until(page, () => window.__krate && window.__krate.state !== 'loading');
  check(tag, (await state(page)) === 'signin', 'signed out, the org app asks for sign-in', await state(page));
  const card = await page.evaluate(() => ({
    company: !document.getElementById('signinCompany').hidden && document.getElementById('signinCompany').getAttribute('href'),
    first: document.querySelector('#signin .stack a:not([hidden])').id,
    github: document.getElementById('signinGithub').className,
    mail: !document.getElementById('signinMail').hidden,
  }));
  const company = card.company ? new URL(card.company, base) : null;
  check(tag, company && company.pathname === '/login/oidc/start' && company.searchParams.get('org') === SLUG
    && company.searchParams.get('from') === `player:org/${SLUG}/${APP}` && /^[0-9a-f]{36}$/.test(company.searchParams.get('nonce') || ''),
  'the company\'s own sign-in is offered, for this team, coming back to this app', card);
  check(tag, card.first === 'signinCompany' && card.github === 'btn ghost' && card.mail,
    'it comes first; GitHub steps back beside Google; an emailed link is offered too', card);
  // The card fits a phone: nothing wider than the screen.
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  check(tag, wide <= 0, 'the sign-in card fits the screen', { overflow: wide });

  // 2. A mistyped address is said, not sent.
  const none = mails.length;
  await page.fill('#signinEmail', 'dan@acme');
  await page.click('#signinSend');
  const typo = await page.textContent('#signinSaid');
  check(tag, /Type the email address/.test(typo) && mails.length === none, 'a mistyped address is said in words and nothing is sent', typo);

  // 3. The emailed link: asked for here, opened in a new tab.
  const before = mails.length;
  await page.fill('#signinEmail', `dan+${engine}${label}@acme.example`);
  await page.click('#signinSend');
  await until(page, () => /We sent a link|did not send|could not be reached/.test(document.getElementById('signinSaid').textContent));
  const sent = await page.textContent('#signinSaid');
  const mail = mails[before];
  check(tag, /^We sent a link to dan\+.*@acme\.example\. Open it on this device/.test(sent) && mail, 'the link is mailed and the page says where it went', { sent, mails: mails.length - before });
  const link = mail && /(http:\/\/127\.0\.0\.1:\d+\/login\/email\/verify\?t=[0-9a-f]+)/.exec(mail.text);
  const tab = await context.newPage();
  tab.on('console', (m) => { if (fromPage(m)) errors.push(m.text()); });
  if (link) await tab.goto(link[1]);
  await until(tab, () => window.__krate && window.__krate.state !== 'loading' && window.__krate.state !== 'signin');
  const landed = new URL(tab.url());
  const session = await tab.evaluate(() => { try { return JSON.parse(localStorage.getItem('krate-player-session') || 'null'); } catch { return null; } });
  check(tag, landed.pathname === ADDRESS && !landed.hash, 'the link comes back to this app, the token out of the address bar', tab.url());
  check(tag, session && /^[A-Za-z0-9_-]{16,}/.test(session.token || '') && (await state(tab)) !== 'signin',
    'opened in a new tab, it is signed in: the proof this browser asked reached that tab', { session: Boolean(session), state: await state(tab) });
  // The same link, used again, is no sign-in at all.
  const again = await context.newPage();
  if (link) await again.goto(link[1]);
  const twice = await again.evaluate(() => document.body.innerText.slice(0, 200)).catch(() => '');
  check(tag, !/\/run\/org\//.test(again.url()) || !again.url().includes('#token='), 'the emailed link works once', { url: again.url(), twice });
  await again.close();
  await tab.close();

  // 4. A finished sign-in this browser never asked for is refused (K-909):
  //    a stranger's own session, handed over in a link.
  const fresh = await browser.newContext(opts);
  const lure = await fresh.newPage();
  await lure.goto(`${base}${ADDRESS}#token=gho_stranger_session_0000&login=mallory&nonce=${'ab'.repeat(18)}`);
  await until(lure, () => window.__krate && window.__krate.state !== 'loading');
  const taken = await lure.evaluate(() => localStorage.getItem('krate-player-session'));
  check(tag, taken === null && (await state(lure)) === 'signin', 'a sign-in this browser did not ask for is not taken', { taken, state: await state(lure) });
  await fresh.close();

  check(tag, errors.length === 0, 'no page errors', errors);
  await context.close();
}
for (const b of Object.values(launched)) await b.close();
server.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
