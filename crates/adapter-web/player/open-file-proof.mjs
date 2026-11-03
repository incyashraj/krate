// A .krate FILE opened in a tab (/run/file), with nothing installed and
// nothing uploaded: the way a person opens one they got in a chat, by
// AirDrop, in an email or as a download. Run against the hub Worker itself,
// here in Node over the worker tests' stand-ins, and the Player in
// cloud/worker/player-dist (scripts/build-web-player.sh), in Chromium on a
// computer and as an Android phone, and WebKit as an iPhone.
//
//   node --experimental-wasm-modules open-file-proof.mjs [shot-dir]
//
// The app is Share Check (evidence/web-player/share-check.krate): it keeps a
// token in store.secret and a list in store.shared, which syncs through the
// hub. The same bytes are also published, as a link whose release has no
// owner on record -- the case a file's storage could most easily collide
// with, since such a link keys its storage by the build alone.
//
// It checks, by what the app printed, what the page showed, and every
// request the hub (and the page) saw:
//   (a) the card shows; a file chosen with the picker is judged, made ready
//       and shows the same consent card a link shows, saying it came from a
//       file; allowed, it runs and its store.shared syncs through the hub;
//   (b) NO request carries the file's bytes, and none names its digest:
//       the hub never hears what was opened;
//   (c) a tampered file is refused in the validator's words -- the words the
//       hub's publish door refuses the same bytes with;
//   (d) the same file opened again keeps its data; it never sees the data
//       of the published link with the same bytes, nor the link the file's,
//       nor another file with the same app id the first file's;
//   (e) a file dropped on the card opens;
//   (f) Chromium: the manifest parses with its file handler and share
//       target; the page's launchQueue consumer opens a launched file; a
//       file shared to /run/file (the share target's POST) is taken by the
//       page's service worker and never reaches the hub; a share that does
//       reach the hub is answered without being read.
// What headless cannot prove: the operating system handing a double-clicked
// .krate to an installed Player, and Android's share sheet listing it. The
// browser side of both (the manifest Chromium reads, the launch consumer,
// the share POST) is proved here.
//
// PLAYWRIGHT_MODULE and CHROMIUM may point at a playwright and a browser
// already on the machine; ENGINES limits the engines (chromium,webkit).
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';
import { consoleError } from './engine-abilities.mjs';

const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = process.env.DIST || join(repo, 'cloud', 'worker', 'player-dist');
const shots = process.argv[2] || null;
const port = 8957;
const base = `http://127.0.0.1:${port}`;
const FIXTURE = join(repo, 'evidence', 'web-player', 'share-check.krate');
const IMPOSTOR = join(repo, 'evidence', 'web-player', 'share-check-impostor.krate');
const fileBytes = new Uint8Array(readFileSync(FIXTURE));
const digest = createHash('sha256').update(fileBytes).digest('hex');

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

let failed = 0;
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};

// The same bytes as a published link, its owner record gone: a release
// from before the hub kept one, whose tab storage is keyed by the build.
const up = await call(env, 'alice', 'POST', '/publish', { bytes: fileBytes });
if (up.status !== 200) throw new Error(`publish: ${up.status} ${JSON.stringify(up.body)}`);
const link = up.body.id;
if (link !== digest) throw new Error('the published id is not the file digest');
env._kv.delete(`app:${link}`);
env._kv.delete(`owner:${link}`);

// The tampered file, and the hub's words for the same bytes at its door.
const tampered = fileBytes.slice();
tampered[9000] ^= 0xff;
const door = await call(env, 'alice', 'POST', '/publish', { bytes: tampered });

// Every request the hub gets, with its body and which phase sent it.
const seen = [];
let phase = 'setup';
const server = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const body = Buffer.concat(chunks);
  seen.push({ phase, method: req.method, url: req.url, body });
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(', ') : v);
  const request = new Request(base + req.url, { method: req.method, headers, body: req.method === 'GET' || req.method === 'HEAD' ? undefined : body });
  const answer = await worker.fetch(request, env, { waitUntil() {} }).catch((e) => new Response(String(e && e.stack), { status: 500 }));
  const out = {};
  answer.headers.forEach((v, k) => { out[k] = v; });
  out['cache-control'] = 'no-store';
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
}).listen(port, '127.0.0.1');

// Windows of the file a request carrying it would hold, compressed or not:
// 48 bytes at every 256th offset.
const windows = [];
for (let at = 0; at + 48 <= fileBytes.length; at += 256) windows.push(Buffer.from(fileBytes.subarray(at, at + 48)));
const carriesFile = (buf) => buf && buf.length >= 48 && windows.some((w) => buf.includes(w));
const namesDigest = (text) => text.includes(digest) || text.includes(digest.slice(0, 16));

const IPHONE = engines.devices['iPhone 13'];
const ANDROID = engines.devices['Pixel 7'];
const plan = [
  ['chromium', 'desktop', { viewport: { width: 1280, height: 800 } }],
  ['chromium', 'phone', { ...ANDROID }],
  ['webkit', 'iphone', { ...IPHONE }],
].filter(([e]) => !process.env.ENGINES || process.env.ENGINES.split(',').includes(e));

const ready = (page) => page.waitForFunction(() => window.__krate && /(^|\n)ready\n/.test(window.__krate.out) || (window.__krate && (window.__krate.trap || window.__krate.state === 'refused')), null, { timeout: 45000 }).catch(() => {});
const linesOf = (page) => page.evaluate(() => window.__krate.out.split('\n').filter(Boolean));
const codeIn = (lines, prefix) => (lines.find((l) => l.startsWith(prefix)) || '').slice(prefix.length);

async function openLink(context, arg) {
  phase = 'link';
  const page = await context.newPage();
  await page.goto(`${base}/run/${link}?probe=1&arg=${arg}`);
  await page.waitForFunction(() => window.__krate && (window.__krate.state === 'consent' || window.__krate.state === 'refused'), null, { timeout: 45000 }).catch(() => {});
  if (await page.evaluate(() => window.__krate.state === 'consent')) await page.click('#consentAllow');
  await ready(page);
  // The link's own count of the open names the same digest: it lands
  // before the file's pages start, so it is never taken for theirs.
  await page.waitForFunction(() => window.__krate.counted, null, { timeout: 10000 }).catch(() => {});
  const out = { lines: await linesOf(page), storeKey: await page.evaluate(() => window.__krate.storeKey) };
  await page.close();
  return out;
}

/// /run/file in a new page, with every request it makes watched; `pick`
/// hands it the file.
async function filePage(context, arg, tag, extra = '') {
  phase = `file:${tag}`;
  const page = await context.newPage();
  const requests = [];
  page.on('request', (r) => requests.push({ url: r.url(), body: r.postDataBuffer() }));
  const errors = [];
  // A screenshot injects a style, which WebKit's content policy logs; with
  // no shot folder, nothing is injected and every error counts.
  page.on('console', (m) => { if (consoleError(m) && !(shots && /Refused to apply a stylesheet/.test(m.text()))) errors.push(m.text()); });
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/run/file?probe=1&arg=${arg}${extra}`);
  await page.waitForFunction(() => window.__krate && (window.__krate.state === 'file' || window.__krate.state === 'refused'), null, { timeout: 30000 }).catch(() => {});
  return { page, requests, errors };
}
async function allowAndRun(page) {
  await page.waitForFunction(() => window.__krate.state === 'consent' || window.__krate.state === 'refused', null, { timeout: 45000 }).catch(() => {});
  const consent = await page.evaluate(() => ({ state: window.__krate.state, text: document.body.innerText, fine: document.getElementById('consentFine').textContent, who: document.getElementById('whoSig').textContent, refused: window.__krate.refused || null }));
  if (consent.state === 'consent') await page.click('#consentAllow');
  await ready(page);
  return { consent, lines: await linesOf(page), storeKey: await page.evaluate(() => window.__krate.storeKey) };
}
function leaks(tag, requests) {
  const hub = seen.filter((s) => s.phase === phase);
  const carrying = hub.filter((s) => carriesFile(s.body)).map((s) => `${s.method} ${s.url}`)
    .concat(requests.filter((r) => carriesFile(r.body)).map((r) => `page ${r.url}`));
  const naming = hub.filter((s) => namesDigest(s.url) || namesDigest(s.body.toString('latin1'))).map((s) => `${s.method} ${s.url}`);
  check(carrying.length === 0, `${tag} no request carries the file's bytes (${hub.length} requests to the hub, ${hub.reduce((n, s) => n + s.body.length, 0)} body bytes in all)`, carrying);
  check(naming.length === 0, `${tag} no request names the file's digest`, naming);
}

for (const [engineName, label, device] of plan) {
  const tag = `[${engineName} ${label}]`;
  const browser = await engines[engineName].launch({ headless: true, ...(engineName === 'chromium' && process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) });
  const context = await browser.newContext({ ...device });
  await context.route('https://krate.tech/**', (route) => route.fulfill({ status: 204, body: '' }));

  // The published link first: it keeps its own token and share code.
  const l1 = await openLink(context, 'create');
  const linkCode = codeIn(l1.lines, 'created: ');
  check(Boolean(linkCode) && l1.lines.includes('token set: ok'), `${tag} the published link keeps a token and makes a share`, l1.lines.slice(0, 6));

  // (a) the card, then the file through the picker.
  const f1 = await filePage(context, 'create', 'first');
  const card = await f1.page.evaluate(() => ({
    state: window.__krate.state, title: document.getElementById('fileTitle').textContent,
    words: document.getElementById('fileWords').textContent, tip: document.getElementById('fileFine').hidden ? '' : document.getElementById('fileTip').textContent,
    drop: !document.getElementById('fileDropSay').hidden, accept: document.getElementById('fileInput').accept,
    scroll: document.documentElement.scrollWidth, width: innerWidth,
    manifest: (document.querySelector('link[rel="manifest"]') || {}).href || '',
  }));
  check(card.state === 'file' && card.title === 'Open a .krate file' && /not uploaded/.test(card.words), `${tag} /run/file shows "Open a .krate file", saying the file is not uploaded`, card);
  check(card.scroll <= card.width, `${tag} nothing on the card scrolls sideways`, card);
  check(label === 'desktop' ? card.drop && card.accept.includes('.krate') : !card.drop && card.accept === '', `${tag} ${label === 'desktop' ? 'a computer is offered the drop and the picker takes .krate' : 'a phone has the picker only, open to every file (a phone greys out a type it does not know)'}`, card);
  check(card.manifest.endsWith('/run/file/app.webmanifest'), `${tag} the page links the Player's file manifest`, card.manifest);
  console.log(`     ${tag} the card's tip: ${card.tip ? `"${card.tip}"` : '(none)'}`);
  // What the transpiler costs the page: fetched and compiled while the
  // card waits for a file (this server sends it uncompressed).
  await f1.page.waitForFunction(() => window.__krate.marks.transpilerCompiled !== undefined, null, { timeout: 30000 }).catch(() => {});
  const cost = await f1.page.evaluate(() => {
    const e = performance.getEntriesByType('resource').find((r) => r.name.endsWith('/player/transpiler.wasm'));
    const m = window.__krate.marks;
    return { bytes: e ? e.decodedBodySize || e.encodedBodySize : null, ready: Math.round(m.transpilerCompiled - m.start), card: Math.round(m.fileCard - m.start) };
  });
  check(cost.bytes > 0 && cost.ready > 0, `${tag} the transpiler loads with the card: ${cost.bytes} bytes, compiled ${cost.ready} ms after the page started (the card showed at ${cost.card} ms)`, cost);
  if (shots) await f1.page.screenshot({ path: join(shots, `file-card-${engineName}-${label}.png`) });
  await f1.page.setInputFiles('#fileInput', FIXTURE);
  const r1 = await allowAndRun(f1.page);
  check(r1.consent.state === 'consent' && /“Share Check” wants to:/.test(r1.consent.text), `${tag} the file shows the consent card a link shows`, r1.consent);
  check(/Opened from the file “share-check\.krate” on this device\. Nothing was uploaded\./.test(r1.consent.fine) && /from a file/.test(r1.consent.who), `${tag} the card and the bar say it was opened from a file`, { fine: r1.consent.fine, who: r1.consent.who });
  const fileCode = codeIn(r1.lines, 'created: ');
  check(r1.lines.includes('code at start: none') && r1.lines.includes('token: none'), `${tag} the file sees none of the link's data (same bytes, same app id)`, r1.lines.slice(0, 3));
  check(Boolean(fileCode) && fileCode !== linkCode && r1.lines.includes('token set: ok') && r1.lines.includes('milk: ok'), `${tag} allowed, it runs: its own share through the hub, its own token`, r1.lines.slice(0, 8));
  check(/~f[0-9a-f]{16}$/.test(r1.storeKey || '') && r1.storeKey !== l1.storeKey, `${tag} its storage is keyed by its own bytes, apart from the link's`, { file: r1.storeKey, link: l1.storeKey });
  const made = await f1.page.evaluate(() => ({ transpile: Math.round(window.__krate.transpileMs), chosenToConsent: Math.round(window.__krate.marks.consent - window.__krate.marks.fileChosen) }));
  console.log(`     ${tag} made ready in the page: transpiled in ${made.transpile} ms; file chosen to consent card ${made.chosenToConsent} ms`);
  if (shots) await f1.page.screenshot({ path: join(shots, `file-app-${engineName}-${label}.png`) });
  check(f1.errors.length === 0, `${tag} no console errors`, f1.errors);
  await f1.page.close();
  leaks(tag, f1.requests);

  // (d) another file with the same app id sees nothing of the first.
  const f2 = await filePage(context, 'reopen', 'impostor');
  await f2.page.setInputFiles('#fileInput', IMPOSTOR);
  const r2 = await allowAndRun(f2.page);
  check(r2.lines.includes('code at start: none') && r2.lines.includes('token: none'), `${tag} another file with the same app id sees none of the first file's data`, r2.lines.slice(0, 3));
  await f2.page.close();

  // (d) the same file again keeps its data.
  const f3 = await filePage(context, 'reopen', 'again');
  await f3.page.setInputFiles('#fileInput', FIXTURE);
  const r3 = await allowAndRun(f3.page);
  check(r3.lines.includes(`code at start: ${fileCode}`) && r3.lines.includes('token: kept'), `${tag} the same file opened again keeps its token and share code`, r3.lines.slice(0, 3));
  await f3.page.close();
  leaks(tag, f3.requests);

  // ... and the link still has its own, untouched by the file.
  const l2 = await openLink(context, 'reopen');
  check(l2.lines.includes(`code at start: ${linkCode}`) && l2.lines.includes('token: kept'), `${tag} the link keeps its own data, none of the file's`, l2.lines.slice(0, 3));

  // (c) a tampered file: refused in the validator's words.
  const t = await filePage(context, 'create', 'tampered');
  await t.page.setInputFiles('#fileInput', { name: 'share-check.krate', mimeType: 'application/octet-stream', buffer: Buffer.from(tampered) });
  await t.page.waitForFunction(() => window.__krate.state === 'refused' || window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
  const refused = await t.page.evaluate(() => ({ state: window.__krate.state, refused: window.__krate.refused, act: document.getElementById('refusedAct').textContent, href: document.getElementById('refusedAct').getAttribute('href') }));
  const doorWords = String((door.body && (door.body.message || door.body.error)) || door.body);
  check(refused.state === 'refused' && refused.refused.title === 'This app did not pass Krate’s checks' && refused.refused.words === 'It was not opened.'
    && refused.refused.detail && doorWords.endsWith(refused.refused.detail) && refused.act === 'Choose another file' && refused.href === '/run/file',
    `${tag} a tampered file is refused in the validator's words, the hub's own: "${refused.refused && refused.refused.detail}"`, { refused, door: door.body });
  if (shots && label !== 'phone') await t.page.screenshot({ path: join(shots, `file-refused-${engineName}-${label}.png`) });
  await t.page.close();
  leaks(tag, t.requests);

  // (e) a file dropped on the card.
  const d = await filePage(context, 'other', 'dropped');
  await d.page.evaluate(async (b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], 'dropped.krate', { type: '' }));
    const zone = document.getElementById('fileDrop');
    for (const type of ['dragenter', 'dragover', 'drop']) zone.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt }));
  }, Buffer.from(fileBytes).toString('base64'));
  const rd = await allowAndRun(d.page);
  check(rd.consent.state === 'consent' && /dropped\.krate/.test(rd.consent.fine) && rd.lines.includes('ready') && rd.storeKey === r1.storeKey,
    `${tag} a file dropped on the card opens, with the same file's storage`, { consent: rd.consent.state, fine: rd.consent.fine, lines: rd.lines.slice(0, 4), key: rd.storeKey });
  await d.page.close();

  if (engineName === 'chromium' && label === 'desktop') {
    // (f) the manifest, as Chromium reads it.
    const m = await filePage(context, 'other', 'manifest');
    const cdp = await context.newCDPSession(m.page);
    const got = await cdp.send('Page.getAppManifest').catch((e) => ({ errors: [{ message: String(e) }] }));
    const parsed = got.manifest || {};
    const raw = JSON.parse(got.data || '{}');
    check((got.errors || []).length === 0 && raw.file_handlers && raw.file_handlers[0].accept['application/x-krate'].includes('.krate')
      && raw.share_target && raw.share_target.method === 'POST' && raw.share_target.params.files[0].name === 'file',
      `${tag} Chromium reads the manifest with no errors: a file handler for .krate and a share target`, { errors: got.errors, fileHandlers: parsed.fileHandlers, raw: raw.file_handlers });
    const installable = await cdp.send('Page.getInstallabilityErrors').catch((e) => ({ installabilityErrors: [{ errorId: String(e) }] }));
    console.log(`     ${tag} Chromium's installability errors: ${JSON.stringify(installable.installabilityErrors)}`);
    await m.page.close();

    // (f) a launched file reaches the page's launchQueue consumer.
    await context.addInitScript(() => {
      if (!window.LaunchQueue) return;
      const set = LaunchQueue.prototype.setConsumer;
      LaunchQueue.prototype.setConsumer = function (fn) { window.__launch = fn; return set.call(this, fn); };
    });
    const lq = await filePage(context, 'other', 'launched');
    const hasQueue = await lq.page.evaluate(() => ({ queue: 'launchQueue' in window, consumer: typeof window.__launch }));
    check(hasQueue.queue && hasQueue.consumer === 'function', `${tag} the page sets a launchQueue consumer`, hasQueue);
    await lq.page.evaluate(async (b64) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      window.__launch({ files: [{ kind: 'file', name: 'launched.krate', getFile: async () => new File([bytes], 'launched.krate') }] });
    }, Buffer.from(fileBytes).toString('base64'));
    const rl = await allowAndRun(lq.page);
    check(rl.consent.state === 'consent' && /launched\.krate/.test(rl.consent.fine) && rl.lines.includes('ready'), `${tag} a launched file opens through the consumer`, { state: rl.consent.state, fine: rl.consent.fine });
    await lq.page.close();
    leaks(tag, lq.requests);

    // (f) a share: the share target's POST, taken by the service worker.
    const sh = await filePage(context, 'other', 'shared');
    const worker = await sh.page.evaluate(async () => {
      const reg = await navigator.serviceWorker.getRegistration('/run/file');
      if (!reg) return { scope: null };
      await navigator.serviceWorker.ready;
      return { scope: reg.scope, active: Boolean(reg.active) };
    });
    check(worker.active && worker.scope === `${base}/run/file`, `${tag} the file page's service worker is active over /run/file only`, worker);
    await sh.page.evaluate(async (b64) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const form = document.createElement('form');
      form.method = 'POST';
      form.enctype = 'multipart/form-data';
      form.action = '/run/file';
      const input = document.createElement('input');
      input.type = 'file';
      input.name = 'file';
      const dt = new DataTransfer();
      dt.items.add(new File([bytes], 'shared.krate', { type: 'application/octet-stream' }));
      input.files = dt.files;
      form.appendChild(input);
      document.body.appendChild(form);
      form.submit();
    }, Buffer.from(fileBytes).toString('base64'));
    await sh.page.waitForURL(/\/run\/file/, { timeout: 15000 }).catch(() => {});
    const rs = await allowAndRun(sh.page);
    const posts = seen.filter((s) => s.phase === phase && s.method === 'POST' && s.url.startsWith('/run/file'));
    check(rs.consent.state === 'consent' && /shared\.krate/.test(rs.consent.fine) && rs.lines.includes('ready') && posts.length === 0,
      `${tag} a file shared to /run/file opens, and the share never reaches the hub`, { state: rs.consent.state, fine: rs.consent.fine, posts: posts.map((p) => p.url), url: sh.page.url() });
    const left = await sh.page.evaluate(async () => (await (await caches.open('krate-file-share')).keys()).length);
    check(left === 0, `${tag} nothing of the shared file is left in the browser's cache`, left);
    await sh.page.close();
    leaks(tag, sh.requests);
  }

  if (shots) {
    // The look, light and dark: the card and an app opened from a file.
    for (const theme of ['light', 'dark']) {
      const p = await filePage(context, 'other', `shot-${theme}`, `&theme=${theme}`);
      await p.page.waitForTimeout(300);
      await p.page.screenshot({ path: join(shots, `file-card-${engineName}-${label}-${theme}.png`) });
      await p.page.setInputFiles('#fileInput', FIXTURE);
      await p.page.waitForFunction(() => window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
      await p.page.screenshot({ path: join(shots, `file-consent-${engineName}-${label}-${theme}.png`) });
      await p.page.click('#consentAllow');
      await ready(p.page);
      await p.page.waitForTimeout(500);
      await p.page.screenshot({ path: join(shots, `file-app-${engineName}-${label}-${theme}.png`) });
      await p.page.close();
    }
  }
  await browser.close();
}

// (f) a share that reaches the hub (no service worker) is not read.
// The body comes a chunk per read, 64 of them: read through, it would be
// pulled 64 times.
let pulled = 0;
const body = new ReadableStream({ pull(c) { pulled += 1; c.enqueue(fileBytes.slice()); if (pulled === 64) c.close(); } }, { highWaterMark: 0 });
const lost = await worker.fetch(new Request(`${base}/run/file`, { method: 'POST', body, duplex: 'half', headers: { 'content-type': 'multipart/form-data; boundary=x' } }), env, { waitUntil() {} });
check(lost.status === 303 && lost.headers.get('location') === '/run/file?share=lost' && pulled <= 1, '[hub] a share that reaches the hub is sent to the page, its body dropped unread', { status: lost.status, location: lost.headers.get('location'), pulled });

server.close();
console.log(failed ? `${failed} check(s) failed` : 'every check passed');
process.exit(failed ? 1 : 0);
