// G1 proof: an organisation's app with team data (store.org) in a real
// browser tab, with nothing installed, against the real hub Worker on this
// machine -- and a phone.
//
// For each engine (Chromium, WebKit), a fresh organisation:
//   - alice, an owner, sets it up from a browser (kit/org-device.js, in
//     Node here): the organisation key and its signing key;
//   - apps/krate-orgstore-probe, signed and released as the organisation;
//   - carol, a developer, opens it in a tab: the tab registers as one of
//     the organisation's browser devices and waits for the key, keeping her
//     record; alice's device passes the key on in the background; the
//     record goes up sealed, a computer reads it, the computer's change
//     comes back to the tab;
//   - dan, outside the app's audience, is refused in words;
//   - phone (Chromium, 390x844, touch): the page fits, a tap presses, a
//     drag scrolls, a tap on a text field raises the keyboard and typing
//     reaches it, the page can be added to a home screen as the app.
// It also measures the validator module (which now carries the team data
// code) loading cold in the tab.
//
//   sh scripts/build-hub-validator.sh && sh scripts/build-web-player.sh
//   (cd apps/krate-hello-gui && cargo component build --release)
//   node --experimental-wasm-modules team-data-proof.mjs [out.json] [chromium,webkit]
//
// PLAYWRIGHT=<path to playwright or playwright-core's index.mjs> picks the
// library when it is not installed here (npm ci in this folder installs it).
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';
import '../../../docs/landing/kit/org-device.js';

const { chromium, webkit } = await import(process.env.PLAYWRIGHT || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const outFile = process.argv[2] || join(here, 'build', 'team-data.json');
const ENGINES = (process.argv[3] || 'chromium,webkit').split(',');
const shots = join(here, 'build', 'shots');
mkdirSync(shots, { recursive: true });
const S = globalThis.KrateSigning;
const PORT = 8921;
const HUB = `http://127.0.0.1:${PORT}`;
const enc = encodeURIComponent;

const results = { checks: [], measures: {}, environment: {} };
let failed = 0;
function check(engine, name, ok, detail) {
  results.checks.push({ engine, name, ok: Boolean(ok), detail });
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} [${engine}] ${name}${detail !== undefined ? ` -- ${typeof detail === 'string' ? detail : JSON.stringify(detail)}` : ''}`);
}

// ---- the hub, in this process, over HTTP for the browsers
let env = null;
const assets = {
  async fetch(request) {
    const name = normalize(new URL(request.url).pathname).replace(/^\/+/, '');
    if (name.includes('..')) return new Response('no', { status: 404 });
    try { return new Response(await readFile(join(dist, name))); } catch { return new Response('not found', { status: 404 }); }
  },
};
const fresh = () => {
  env = orgEnv({ PUBLIC_BASE: HUB, SITE_BASE: 'https://krate.tech', ASSETS: assets, ORG_LEASE_KEY: '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60' });
  return env;
};
// Node's own calls (alice's browser, the computer) reach the same Worker.
const nodeFetch = globalThis.fetch;
globalThis.fetch = (url, init) => (String(url).startsWith(`${HUB}/`) ? worker.fetch(new Request(url, init), env, { waitUntil() {} }) : nodeFetch(url, init));
const server = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(', ') : v);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  let answer;
  try {
    answer = await worker.fetch(new Request(HUB + req.url, { method: req.method, headers, body: req.method === 'GET' || req.method === 'HEAD' ? undefined : body }), env, { waitUntil() {} });
  } catch (e) {
    answer = new Response(String(e && e.stack), { status: 500 });
  }
  const out = {};
  answer.headers.forEach((v, k) => { out[k] = v; });
  out['cache-control'] = 'no-store';
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
});
await new Promise((r) => server.listen(PORT, '127.0.0.1', r));

// ---- the app: the orgstore probe, as a .krate
const probe = join(here, 'build', 'apps', 'orgstore-probe.krate');
mkdirSync(join(here, 'build', 'apps'), { recursive: true });
execFileSync(process.execPath, [join(here, 'pack.mjs'), join(repo, 'apps', 'krate-orgstore-probe', 'manifest.toml'), join(repo, 'crates', 'cli', 'tests', 'fixtures', 'orgstore-probe.wasm'), probe], { stdio: 'ignore' });
const hello = join(here, 'build', 'apps', 'hello-gui.krate');
execFileSync(process.execPath, [join(here, 'pack.mjs'), join(repo, 'apps', 'krate-hello-gui', 'manifest.toml'), join(repo, 'apps', 'krate-hello-gui', 'target', 'wasm32-wasip1', 'release', 'krate_hello_gui.wasm'), hello], { stdio: 'ignore' });
const mod = await S.loadModule(readFileSync(join(dist, 'validator.wasm')));

/// A fresh organisation with the probe released and alice's browser holding
/// its keys. Answers what the rest needs.
async function world() {
  fresh();
  const ok = (r, what) => { if (r.status >= 300) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body)}`); return r; };
  ok(await call(env, 'alice', 'POST', '/orgs', { json: { slug: 'acme', name: 'Acme' } }), 'create');
  for (const [login, role] of [['carol', 'developer'], ['dan', 'recipient']]) {
    const inv = ok(await call(env, 'alice', 'POST', '/orgs/acme/invites', { json: { github: login, role } }), 'invite');
    ok(await call(env, login, 'POST', `/orgs/acme/invites/${inv.body.code}/accept`), 'accept');
  }
  const alice = await S.openDevice({ store: S.memoryStore(), name: "Alice's browser" });
  const as = { hub: HUB, token: 'gho_alice', slug: 'acme', device: alice, mod };
  await S.setUp(as);
  const out = await S.release({ ...as, bytes: new Uint8Array(readFileSync(probe)), name: 'Site inspections' });
  if (out.state !== 'released') throw new Error(`not released: ${JSON.stringify(out)}`);
  // The publish door made the tab's pack for it; a public app beside it.
  ok(await call(env, 'alice', 'POST', '/publish', { bytes: new Uint8Array(readFileSync(hello)), headers: { 'x-krate-name': 'Hello GUI' } }), 'publish hello');
  const appId = mod.judge(new Uint8Array(readFileSync(probe))).judgement.manifest.app.id;
  const id = mod.keys({ op: 'device_id', public_key: alice.public_key }).device_id;
  return { as, alice, aliceId: id, digest: out.digest, appId };
}

/// Alice's computer: the same crate the runtime syncs with, through the
/// module, holding the key from alice's device's envelope.
async function computerOf(w) {
  const view = (await call(env, 'alice', 'GET', `/orgs/acme/keys?device=${w.aliceId}`)).body;
  const store = `computer-${Math.random()}`;
  mod.data({ op: 'open', store, device: 'aaaaaaaaaaaaaaaa' });
  const keys = { store, org: 'acme', app_id: w.appId, envelope: view.envelope.blob, device_secret: await w.alice.secret() };
  return {
    put: (id, fields) => mod.data({ op: 'put', store, collection: 'inspections', id, now: Date.now(), me: 'alice', fields }),
    get: (id) => mod.data({ op: 'get', store, collection: 'inspections', id }).entry,
    async sync() {
      const begin = mod.data({ ...keys, op: 'sync_begin', now: Date.now() }, 480);
      const r = await call(env, 'alice', 'POST', `/orgs/acme/data/${w.digest}/sync`, { json: begin.body });
      if (r.status !== 200) throw new Error(JSON.stringify(r.body));
      return mod.data({ ...keys, op: 'sync_end', answer: r.body });
    },
  };
}

/// One run of the probe in a tab, as `who`: its steps, joined by `then`.
async function runProbe(browser, w, who, steps, { context } = {}) {
  const ctx = context || (await browser.newContext());
  if (!context) await ctx.addInitScript((t) => localStorage.setItem('krate-player-session', JSON.stringify({ token: t.token, login: t.login })), { token: `gho_${who}`, login: who });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  const q = steps.split(' ').map((a) => `arg=${enc(a)}`).join('&');
  await page.goto(`${HUB}/run/${w.digest}?${q}`);
  await page.waitForFunction(() => window.__krate && window.__krate.state !== 'loading', null, { timeout: 20000 });
  if ((await page.evaluate(() => window.__krate.state)) === 'consent') await page.click('#consentAllow');
  await page.waitForFunction(() => window.__krate.exit !== undefined || window.__krate.trap || window.__krate.state === 'refused', null, { timeout: 20000 }).catch(() => {});
  const r = await page.evaluate(() => ({ ...window.__krate, team: undefined }));
  return { ctx, page, r, errors };
}

async function engine(name, type) {
  const browser = await type.launch({ headless: true, ...(name === 'chromium' && process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) });
  results.environment[name] = browser.version();
  const m = (results.measures[name] = {});
  const w = await world();

  // ---- a member's first open: new device, waiting for the key
  const first = await runProbe(browser, w, 'carol', 'put inspections i1 site=North-depot passed?=false then sync then get inspections i1');
  const lines = first.r.out.trim().split('\n');
  check(name, 'the app opens with team data granted (consent lists it as working here)', first.r.state === 'app' && first.r.teamMode === 'on', { state: first.r.state, teamMode: first.r.teamMode, teamError: first.r.teamError, errors: first.errors, refused: first.r.refused });
  check(name, 'a write is kept before the tab holds the key', lines[0] === 'put i1 ok', lines);
  check(name, 'its sync reaches the organisation and waits for the key: one change pending', /^sync online=true changed=false pending=2$/.test(lines[1] || ''), lines);
  check(name, 'the record reads back from this browser', /^get i1 passed=false;site=North-depot by=carol$/.test(lines[2] || ''), lines);
  check(name, 'the page says it is waiting for the team key', first.r.sync && first.r.sync.link === 'waiting-for-key', first.r.sync);
  await first.page.click('#sync');
  check(name, 'tapping the state says what happens next, in words', /gives it the team key by itself/.test(await first.page.textContent('#syncSay')));
  await first.page.screenshot({ path: join(shots, `${name}-team-waiting.png`) });
  check(name, "the state is said with the organisation's name", /new to Acme\./.test(await first.page.textContent('#syncSay')), await first.page.textContent('#syncSay'));
  // The same, on a phone's width, and in dark.
  await first.page.setViewportSize({ width: 360, height: 760 });
  await first.page.waitForTimeout(300);
  const narrow = await first.page.evaluate(() => [document.documentElement.scrollWidth, innerWidth, document.getElementById('sync').getBoundingClientRect().right]);
  check(name, 'at 360 wide the bar and its state fit: no sideways scroll', narrow[0] <= narrow[1] && narrow[2] <= narrow[1], narrow);
  await first.page.screenshot({ path: join(shots, `${name}-team-waiting-360.png`) });
  await first.page.emulateMedia({ colorScheme: 'dark' });
  await first.page.waitForTimeout(200);
  await first.page.screenshot({ path: join(shots, `${name}-team-waiting-360-dark.png`) });
  const devices = (await call(env, 'alice', 'GET', `/orgs/acme/keys?device=${w.aliceId}`)).body;
  const tab = devices.devices.find((d) => d.principal === 'github:carol');
  check(name, "the tab is one of the organisation's devices, a browser, waiting", tab && tab.kind === 'browser' && tab.holds === null && devices.requests.some((r) => r.device_id === tab.device_id), tab);
  await first.page.close();

  // ---- a holder passes the key on in the background (alice's device)
  const duty = await S.refresh({ ...w.as, held: devices.envelope.blob });
  check(name, "alice's device passes the key to carol's tab", duty.status === 'holds');

  // ---- next open: the record goes up sealed
  const second = await runProbe(browser, w, 'carol', 'sync then get inspections i1 then who', { context: first.ctx });
  const l2 = second.r.out.trim().split('\n');
  check(name, 'next open: the tab holds the key and its record went up (nothing pending)', /^sync online=true changed=(true|false) pending=0$/.test(l2[0] || ''), l2);
  check(name, 'who: the organisation names the person', l2[2] === 'who carol developer Acme', l2);
  check(name, 'the chip says synced', second.r.sync && second.r.sync.link === 'synced', second.r.sync);
  const held = JSON.stringify(env.ORGS.object('acme').sql.exec('SELECT * FROM org_data').toArray());
  check(name, 'the hub holds ciphertext only', held.length > 50 && !/North-depot|inspections|"i1"|carol@/.test(held));

  // ---- a computer reads it, and its change comes back
  const desk = await computerOf(w);
  await desk.sync();
  const seen = desk.get('i1');
  check(name, "a computer reads the tab's record, by carol", seen && seen.changed_by === 'carol' && JSON.stringify(seen.fields) === JSON.stringify([['passed', { b: false }], ['site', { t: 'North-depot' }]]), seen);
  desk.put('i1', [['passed', { b: true }]]);
  await desk.sync();
  await second.page.close();
  const third = await runProbe(browser, w, 'carol', 'sync then get inspections i1', { context: first.ctx });
  const l3 = third.r.out.trim().split('\n');
  check(name, "the tab reads the computer's change, by alice", /^get i1 passed=true;site=North-depot by=alice$/.test(l3[1] || ''), l3);
  await third.page.close();

  // ---- outside the app's audience
  await call(env, 'alice', 'POST', '/orgs/acme/groups', { json: { name: 'finance' } });
  await call(env, 'alice', 'PUT', `/orgs/acme/releases/${w.digest}/audience`, { json: { groups: ['finance'] } });
  const outsider = await runProbe(browser, w, 'dan', 'get inspections i1');
  check(name, 'someone outside the audience is refused in words', outsider.r.state === 'refused' && /not shared with you/.test(outsider.r.refused.title), outsider.r.refused);
  await outsider.ctx.close();
  await first.ctx.close();

  // ---- the validator module, cold, in this tab
  {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    await page.goto(`${HUB}/run/${'0'.repeat(64)}`);
    const t = await page.evaluate(async () => {
      const out = [];
      for (let i = 0; i < 5; i++) {
        const t0 = performance.now();
        const res = await fetch(`/player/validator.wasm?cold=${Math.random()}`, { cache: 'no-store' });
        const bytes = await res.arrayBuffer();
        const t1 = performance.now();
        const mod = await WebAssembly.compile(bytes);
        await WebAssembly.instantiate(mod, {});
        out.push({ fetch: t1 - t0, compileAndInstantiate: performance.now() - t1, bytes: bytes.byteLength });
      }
      return out;
    });
    const med = (k) => t.map((x) => x[k]).sort((a, b) => a - b)[2];
    m.validatorBytes = t[0].bytes;
    m.validatorFetchMs = Math.round(med('fetch') * 10) / 10;
    m.validatorCompileMs = Math.round(med('compileAndInstantiate') * 10) / 10;
    console.log(`     [${name}] validator.wasm ${m.validatorBytes} bytes: fetch ${m.validatorFetchMs} ms, compile and instantiate ${m.validatorCompileMs} ms (median of 5, cold, local)`);
    await ctx.close();
  }
  await browser.close();
}

// ---- a phone (Chromium's emulation: touch, a coarse pointer, 390x844)
async function phone() {
  const name = 'phone';
  const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) });
  fresh();
  const pub = await call(env, 'alice', 'POST', '/publish', { bytes: new Uint8Array(readFileSync(hello)), headers: { 'x-krate-name': 'Hello GUI' } });
  const hash = pub.body.id;
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(`${HUB}/run/${hash}?probe=1`);
  await page.waitForFunction(() => window.__krate && window.__krate.frames > 0, null, { timeout: 20000 });
  const k = () => page.evaluate(() => ({ ...window.__krate }));
  const widget = async (pred) => ((await k()).screen || []).find(pred);
  const box = await page.locator('#screen').boundingBox();
  const fit = await page.evaluate(() => [document.documentElement.scrollWidth, innerWidth, document.getElementById('screen').getBoundingClientRect().width]);
  check(name, 'the app fits the phone: no sideways scroll', fit[0] <= fit[1] && fit[2] <= fit[1], fit);
  const settle = (ms = 250) => page.waitForTimeout(ms);
  const tapOn = async (pred) => {
    const w = await widget(pred);
    if (!w) return false;
    await page.touchscreen.tap(box.x + w.x + w.w / 2, box.y + w.y + w.h / 2);
    await settle();
    return true;
  };
  await tapOn((w) => w.label === 'Click me');
  check(name, 'a tap presses the button', !!(await widget((w) => w.label === 'clicked!')));
  // A browser raises the keyboard only for a focus made inside the tap
  // itself: what has focus is read in the same pointerup, after the Player's
  // own handler has run.
  await page.evaluate(() => {
    window.__inTap = [];
    document.getElementById('screen').addEventListener('pointerup', () => window.__inTap.push(document.activeElement && document.activeElement.id));
  });
  await tapOn((w) => w.kind === 'TextField');
  const focused = await page.evaluate(() => window.__inTap);
  check(name, 'a tap on the text field raises the keyboard: the hidden field has focus inside the tap', focused.at(-1) === 'ime', focused);
  await page.keyboard.insertText('hi');
  await settle();
  check(name, "the phone keyboard's typing reaches the field", !!(await widget((w) => w.kind === 'TextField' && (w.label || '').endsWith('hi'))), await widget((w) => w.kind === 'TextField'));
  // Backspace on the phone keyboard: an input event deleting backwards.
  await page.keyboard.press('Backspace');
  await settle();
  check(name, "the phone keyboard's delete reaches the field", !!(await widget((w) => w.kind === 'TextField' && (w.label || '').endsWith('h') && !(w.label || '').endsWith('hi'))), await widget((w) => w.kind === 'TextField'));
  // A finger dragged up over the scroll area scrolls it and presses nothing.
  const line = await widget((w) => w.label === 'scroll line one');
  if (line) {
    const cdp = await ctx.newCDPSession(page);
    const x = box.x + line.x + 10;
    const y = box.y + line.y + 10;
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x, y }] });
    for (let i = 1; i <= 6; i++) await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x, y: y - i * 8 }] });
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await settle(400);
    const moved = await widget((w) => w.label === 'scroll line one');
    check(name, 'a finger dragged up scrolls the list', moved && moved.y < line.y, { before: line.y, after: moved && moved.y });
  } else {
    check(name, 'a finger dragged up scrolls the list', false, 'no scroll line on screen');
  }
  // Home screen: the manifest the page links, as Chromium reads it.
  const cdp = await ctx.newCDPSession(page);
  const man = await cdp.send('Page.getAppManifest');
  const parsed = man.data ? JSON.parse(man.data) : null;
  check(name, 'the page links a web app manifest with the app name and its own start address', parsed && parsed.name === 'krate-hello-gui' && parsed.start_url === `/run/${hash}` && parsed.display === 'standalone' && (man.errors || []).length === 0, { url: man.url, errors: man.errors, name: parsed && parsed.name });
  const inst = await cdp.send('Page.getInstallabilityErrors').catch((e) => ({ error: e.message }));
  results.measures.phone = { installability: inst };
  check(name, 'Chromium finds nothing that stops it being installed', inst.installabilityErrors && inst.installabilityErrors.length === 0, inst);
  const touchIcon = await page.evaluate(() => (document.querySelector('link[rel="apple-touch-icon"]') || {}).href || '');
  check(name, 'an iPhone gets its home screen picture (a drawn PNG of the initial)', touchIcon.startsWith('data:image/png;base64,'), touchIcon.slice(0, 40));
  const icon = await page.evaluate(async (u) => { const r = await fetch(u); return [r.status, r.headers.get('content-type')]; }, parsed.icons[0].src);
  check(name, "the manifest's icon is served", icon[0] === 200 && /svg/.test(icon[1]), icon);
  check(name, 'no page errors', errors.length === 0, errors);
  await page.screenshot({ path: join(shots, 'phone-hello-390.png') });
  await browser.close();
}

try {
  for (const e of ENGINES) {
    if (e === 'phone') continue;
    try {
      await engine(e, e === 'webkit' ? webkit : chromium);
    } catch (err) {
      check(e, 'the run finished', false, String(err && err.stack).split('\n').slice(0, 4).join(' | '));
    }
  }
  if (!process.argv[3] || ENGINES.includes('phone')) {
    try { await phone(); } catch (err) { check('phone', 'the run finished', false, String(err && err.stack).split('\n').slice(0, 4).join(' | ')); }
  }
} finally {
  server.close();
  writeFileSync(outFile, JSON.stringify(results, null, 2));
  console.log(`${results.checks.length - failed}/${results.checks.length} checks passed; wrote ${outFile}`);
  process.exit(failed ? 1 : 0);
}
