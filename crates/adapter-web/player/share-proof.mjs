// A shared store and secrets in the tab (IC-974): krate:store/shared and
// krate:store/secret, opened the way a person opens a shared link --
// Chromium on a computer and as a phone, WebKit as an iPhone -- against the
// hub Worker itself, running here in Node over the worker tests' stand-ins
// (so /share/new, GET and PUT /share/<code> are the hub's own), and the
// Player in cloud/worker/player-dist (scripts/build-web-player.sh).
//
//   node --experimental-wasm-modules share-proof.mjs [shot-dir]
//
// The app is Share Check (fixtures/share-check), packed twice:
//   (cd fixtures/share-check && cargo component build --release)
//   W=fixtures/share-check/target/wasm32-wasip1/release/krate_share_check.wasm
//   node pack.mjs fixtures/share-check/manifest.toml $W ../../../evidence/web-player/share-check.krate
//   node pack.mjs fixtures/share-check/manifest-ungranted.toml $W ../../../evidence/web-player/share-check-ungranted.krate
//
// Two browser contexts are two people's browsers. It checks, by what the
// app printed, what the hub holds and what the browser keeps:
//   (a) the consent card does not send either store to a computer;
//   (b) A makes a share; a value over 64 KiB is too-large, an empty or
//       129-byte key invalid-name, sync before a share not-joined;
//   (c) A's writes reach the hub by A's own syncs;
//   (d) B joins by the code (a made-up one is no-such-share) and reads A's
//       writes; B's delete and new key reach A by sync;
//   (e) B offline: its write is kept, sync is not an error, nothing reaches
//       the hub; back online, the write reaches A;
//   (f) secrets: over 64 KiB is too-large, a name with / invalid-name,
//       deleting an absent one succeeds; the token set in A's first open is
//       there in its next, and not in B's browser;
//   (g) nothing in A's IndexedDB holds the token, its base64 or the
//       secret's name in plain text, and the key there is a CryptoKey the
//       page cannot export;
//   (h) the next open of A keeps the shared copy and the code; leaving
//       keeps the copy and forgets the code;
//   (i) the same app without either grant hears denied, as on desktop;
//   (j) no console errors and no notice that anything needs a computer.
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
const port = 8937;
const base = `http://127.0.0.1:${port}`;
const TOKEN = 'tok-share-check-PLAINTEXT-7f3a9c';

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

const publish = async (file) => {
  const bytes = new Uint8Array(readFileSync(join(repo, 'evidence', 'web-player', file)));
  const up = await call(env, 'alice', 'POST', '/publish', { bytes });
  if (up.status !== 200) throw new Error(`publish ${file}: ${up.status} ${JSON.stringify(up.body)}`);
  return up.body.id;
};
const granted = await publish('share-check.krate');
const ungranted = await publish('share-check-ungranted.krate');

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
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};
const until = async (fn, ms) => {
  const end = Date.now() + ms;
  for (;;) {
    const v = await fn();
    if (v || Date.now() > end) return v;
    await new Promise((r) => setTimeout(r, 200));
  }
};
/// The share as the hub holds it.
const hubShare = async (code) => {
  const raw = await env.APPS.get(`share:${code}`);
  return raw ? JSON.parse(raw).kv : null;
};
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
const RUNS = [
  ['chromium', 'desktop', { width: 1280, height: 800 }, undefined],
  ['chromium', 'phone', { width: 375, height: 812 }, IPHONE],
  ['webkit', 'iphone', { width: 390, height: 844 }, IPHONE],
];
const launched = {};

/// Open the app in a context with one argument; resolves once it printed
/// `ready` (or stopped).
async function launch(context, hash, arg) {
  const page = await context.newPage();
  const errors = [];
  page.on('console', (m) => { if (consoleError(m)) errors.push(`${m.text()} @${JSON.stringify(m.location())}`); });
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/run/${hash}?probe=1&arg=${encodeURIComponent(arg)}`);
  // An app asking for nothing past the defaults opens with no card.
  await page.waitForFunction(() => window.__krate && (window.__krate.state === 'consent' || window.__krate.state === 'app'), null, { timeout: 30000 }).catch(() => {});
  const card = await page.evaluate(() => document.body.innerText);
  if (await page.evaluate(() => window.__krate.state === 'consent')) await page.click('#consentAllow');
  await page.waitForFunction(() => /(^|\n)ready\n/.test(window.__krate.out) || window.__krate.trap, null, { timeout: 30000 }).catch(() => {});
  const lines = () => page.evaluate(() => window.__krate.out.split('\n').filter(Boolean));
  const state = () => page.evaluate(() => ({
    trap: window.__krate.trap, notices: window.__krate.notices,
    notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText,
  }));
  const waitLine = async (re, ms) => until(async () => (await lines()).find((l) => re.test(l)), ms);
  return { page, card, errors, lines, state, waitLine };
}

/// Everything this origin's IndexedDB holds for the Player, as text a
/// person could read, and what the key records are.
const dumpStorage = () => new Promise((resolve, reject) => {
  const open = indexedDB.open('krate-player');
  open.onerror = () => reject(open.error);
  open.onsuccess = () => {
    const t = open.result.transaction('kv', 'readonly');
    const s = t.objectStore('kv');
    const keys = s.getAllKeys();
    const values = s.getAll();
    t.oncomplete = () => {
      const keyRecords = [];
      const text = (v) => {
        if (v === null || v === undefined) return '';
        if (typeof v === 'string') return v;
        if (typeof v === 'number' || typeof v === 'boolean') return String(v);
        if (typeof CryptoKey !== 'undefined' && v instanceof CryptoKey) {
          keyRecords.push({ type: v.type, extractable: v.extractable, algorithm: v.algorithm.name, usages: v.usages });
          return '[CryptoKey]';
        }
        if (v instanceof ArrayBuffer || ArrayBuffer.isView(v)) {
          const b = v instanceof ArrayBuffer ? new Uint8Array(v) : new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
          return Array.from(b, (c) => String.fromCharCode(c)).join('');
        }
        if (typeof v === 'object') return Object.entries(v).map(([k, x]) => `${k}:${text(x)}`).join('|');
        return String(v);
      };
      resolve({
        keys: keys.result.map((k) => JSON.stringify(k)),
        records: values.result.map((v, i) => ({ key: JSON.stringify(keys.result[i]), text: text(v) })),
        keyRecords,
      });
    };
  };
});

for (const [engine, label, viewport, ua] of RUNS) {
  const tag = `[${engine} ${label}]`;
  const browser = (launched[engine] ||= await engines[engine].launch({ headless: true }));
  const options = { viewport, userAgent: ua, isMobile: Boolean(ua), hasTouch: Boolean(ua) };
  const ctxA = await browser.newContext(options);
  const ctxB = await browser.newContext(options);
  const pages = [];

  // (a) (b) A makes a share and fills it.
  const a = await launch(ctxA, granted, 'create');
  pages.push(a);
  const al = await a.lines();
  const code = (al.find((l) => l.startsWith('created: ')) || '').slice('created: '.length);
  check(!/computer/i.test(a.card), `${tag} the consent card does not send the shared store or secrets to a computer`, a.card.slice(0, 400));
  check(/^[a-z2-9]{10}$/.test(code), `${tag} A makes a share and hears its ten-character code`, al);
  const want = (lines, expected, what) => check(expected.every((l) => lines.includes(l)), `${tag} ${what}`, { want: expected.filter((l) => !lines.includes(l)), lines });
  want(al, ['code at start: none', 'sync before a share: not-joined', 'milk: ok', 'eggs: ok', 'reads back: eggs=12 milk=2'],
    'A: sync before a share is not-joined; its writes read back at once');
  want(al, ['too big: too-large', 'empty key: invalid-name', 'long key: invalid-name'],
    'A: a value over 64 KiB is too-large, an empty or 129-byte key invalid-name');
  want(al, ['token: none', 'names: ', 'token set: ok', 'secret too big: too-large', 'secret at the limit: ok', 'secret bad name: invalid-name', 'secret delete absent: ok', 'secret delete limit: ok', 'names now: token'],
    'A: secrets keep, list by name, and refuse over 64 KiB and a name with /');

  // (c) A's own syncs push its writes.
  const pushed = await until(async () => {
    const kv = await hubShare(code);
    return kv && kv.milk && kv.eggs ? kv : null;
  }, 15000);
  check(pushed && atob(pushed.milk.v) === '2' && atob(pushed.eggs.v) === '12' && !pushed.big, `${tag} A's writes reach the hub by its own sync (and the refused value does not)`, pushed);

  // (d) B joins and edits; A hears it.
  const b = await launch(ctxB, granted, `join:${code}`);
  pages.push(b);
  const bl = await b.lines();
  want(bl, ['made-up code: no-such-share', 'short code: no-such-share', 'joined', 'after join: eggs=12 milk=2'],
    'B: a made-up code is no-such-share; joining reads what A wrote');
  want(bl, ['token: none', 'delete eggs: ok', 'bread: ok', 'edited: bread=1 milk=2'],
    'B: its own browser has no token of A\'s; its delete and new key read back');
  const heard = await a.waitLine(/^changed: bread=1 milk=2$/, 20000);
  check(Boolean(heard), `${tag} B's new key and its delete reach A by sync`, (await a.lines()).slice(-6));
  const tomb = await hubShare(code);
  check(tomb && tomb.eggs && tomb.eggs.v === null && atob(tomb.bread.v) === '1', `${tag} the hub holds the delete as a tombstone`, tomb);

  // (e) B offline: the write waits, sync is not an error.
  await b.page.route('**/share/**', (route) => route.abort('internetdisconnected'));
  await b.page.locator('#screen').focus();
  await b.page.keyboard.press('j');
  const wrote = await b.waitLine(/^sync after jam: /, 10000);
  await new Promise((r) => setTimeout(r, 5000));
  const offline = await b.lines();
  const atHub = (await hubShare(code)) || {};
  check(offline.includes('wrote jam: ok') && /^sync after jam: ok (true|false)$/.test(wrote || '') && !offline.some((l) => l.startsWith('sync error')) && !atHub.jam,
    `${tag} offline, B's write is kept, five seconds of syncs are not an error, nothing reached the hub`, { wrote, tail: offline.slice(-5), jam: atHub.jam });
  await b.page.unroute('**/share/**');
  const back = await a.waitLine(/^changed: bread=1 jam=1 milk=2$/, 20000);
  check(Boolean(back), `${tag} back online, B's write reaches A`, (await a.lines()).slice(-6));

  // (g) What the browser keeps for A. First, that the reading below would
  // see the token if it were kept as bytes: a control record, then gone.
  const control = (put) => a.page.evaluate(({ put: p, token }) => new Promise((resolve, reject) => {
    const open = indexedDB.open('krate-player');
    open.onsuccess = () => {
      const t = open.result.transaction('kv', 'readwrite');
      const s = t.objectStore('kv');
      if (p) s.put(new TextEncoder().encode(token).buffer, ['proof-control']); else s.delete(['proof-control']);
      t.oncomplete = () => resolve();
      t.onerror = () => reject(t.error);
    };
  }), { put, token: TOKEN });
  await control(true);
  const seen = await a.page.evaluate(dumpStorage);
  await control(false);
  const tokenB64 = Buffer.from(TOKEN).toString('base64');
  const kept = await a.page.evaluate(dumpStorage);
  const all = kept.records.map((r) => r.text).join('\n');
  const sealed = kept.records.filter((r) => /,2\]$/.test(r.key));
  check(seen.records.some((r) => r.text.includes(TOKEN)) && sealed.length === 1
    && !all.includes(TOKEN) && !all.includes(tokenB64) && !all.includes(tokenB64.slice(0, 16)),
    `${tag} the token is kept, sealed, and nowhere in A's IndexedDB in plain text or base64 (a plain control record is seen)`, kept.keys);
  check(sealed.length === 1 && !/token/.test(sealed[0].text), `${tag} the sealed secrets do not even show the secret's name`, sealed.map((r) => r.key));
  check(kept.keyRecords.length === 1 && kept.keyRecords[0].extractable === false && kept.keyRecords[0].algorithm === 'AES-GCM',
    `${tag} the key kept there is a non-extractable AES-GCM CryptoKey`, kept.keyRecords);
  const exported = await a.page.evaluate(() => new Promise((resolve) => {
    const open = indexedDB.open('krate-player');
    open.onsuccess = () => {
      const s = open.result.transaction('kv', 'readonly').objectStore('kv');
      const r = s.openCursor();
      r.onsuccess = async () => {
        const c = r.result;
        if (!c) return resolve('no key found');
        if (typeof CryptoKey !== 'undefined' && c.value instanceof CryptoKey) {
          try { await crypto.subtle.exportKey('raw', c.value); resolve('exported'); } catch (e) { resolve(`refused: ${e.name}`); }
          return;
        }
        c.continue();
      };
    };
  }));
  check(/^refused/.test(exported), `${tag} page script cannot export the key`, exported);

  // Read before the pictures, which inject a style WebKit's CSP logs.
  const errorsAB = [...a.errors, ...b.errors];
  const statesAB = [await a.state(), await b.state()];
  if (shots) {
    await a.page.screenshot({ path: join(shots, `share-proof-${engine}-${label}-a.png`) });
    await b.page.screenshot({ path: join(shots, `share-proof-${engine}-${label}-b.png`) });
  }
  await a.page.close();
  await b.page.close();

  // (f) (h) A's next open.
  const a2 = await launch(ctxA, granted, 'reopen');
  pages.push(a2);
  const a2l = await a2.lines();
  want(a2l, [`code at start: ${code}`, 'token: kept', 'names: token'], 'the next open in A\'s browser keeps the code and the token');
  want(a2l, ['kept here: bread=1 jam=1 milk=2', 'leave: ok', 'code after leave: none', 'sync after leave: not-joined', 'after leave: bread=1 jam=1 milk=2'],
    'the shared copy outlived the tab; leaving keeps the copy and forgets the code');
  const errorsA2 = [...a2.errors];
  const stateA2 = await a2.state();
  await a2.page.close();

  // (i) Without the grants: denied, as on desktop.
  const u = await launch(ctxB, ungranted, 'create');
  pages.push(u);
  const ul = await u.lines();
  want(ul, ['code at start: denied', 'token: denied', 'names: denied'], 'an app without store.shared and store.secret hears denied');
  const errorsU = [...u.errors];
  const states = [...statesAB, stateA2, await u.state()];
  await u.page.close();

  // (j)
  const quiet = states.every((s) => !s.notices.length && !/computer/i.test(s.notice) && !s.trap);
  check(quiet, `${tag} nothing says the stores need a computer`, states);
  // The browser logs every failed request itself: the made-up code's 404
  // and the requests this proof cut off are what it asked for. Anything
  // else is an error.
  const asked = (e) => /^Failed to load resource/.test(e)
    && (/\/share\/nothere234"/.test(e) || (/ERR_INTERNET_DISCONNECTED/.test(e) && /\/share\/[a-z2-9]{10}"/.test(e)));
  const errors = [...errorsAB, ...errorsA2, ...errorsU].filter((e) => !asked(e));
  check(errors.length === 0, `${tag} no console errors (past the failed requests this proof caused)`, errors);
  await ctxA.close();
  await ctxB.close();
}

for (const x of Object.values(launched)) await x.close();
server.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
