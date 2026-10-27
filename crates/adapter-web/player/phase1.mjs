// IC-956 Phase 1 proof: real Krate apps opened in a tab from the real hub
// Worker (hub-local.mjs), driven headless the way a person uses them --
// every control clicked, typed into, scrolled past the bottom, reloaded --
// in Chromium and WebKit, with cold start and input-to-frame measured.
//
//   node prepare.mjs                    # the bundles (after build.sh)
//   node phase1.mjs <out.json> [runs]   # writes the evidence, exits 1 on a failed check
//
// Ports: the hub on 8911, the echo server the CLI app fetches from on 8913.
import http from 'node:http';
import { spawn } from 'node:child_process';
import { writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { loadavg } from 'node:os';
import { chromium, webkit, firefox } from 'playwright';

const here = fileURLToPath(new URL('.', import.meta.url));
const out = process.argv[2] || join(here, 'build', 'phase1.json');
const RUNS = Number(process.argv[3] || 10);
const HUB = 'http://127.0.0.1:8911';
const ECHO = 'http://127.0.0.1:8913';
const shots = join(here, 'build', 'shots');
mkdirSync(shots, { recursive: true });

// ---- the server the CLI app reaches: CORS on, one redirect, a log of what
// it was asked, so a redirect that was followed would show here.
const asked = [];
const echo = http.createServer((req, res) => {
  asked.push(req.url);
  const cors = { 'access-control-allow-origin': '*', 'cache-control': 'no-store' };
  if (req.url === '/hello') { res.writeHead(200, { ...cors, 'content-type': 'text/plain' }); res.end('hello from the test server\n'); return; }
  if (req.url === '/redirect') { res.writeHead(302, { ...cors, location: `${ECHO}/elsewhere` }); res.end(); return; }
  if (req.url === '/elsewhere') { res.writeHead(200, { ...cors, 'content-type': 'text/plain' }); res.end('YOU-WERE-REDIRECTED\n'); return; }
  res.writeHead(404, cors); res.end();
});
await new Promise((r) => echo.listen(8913, '127.0.0.1', r));

// ---- the hub
const hub = spawn(process.execPath, ['--experimental-wasm-modules', join(here, 'hub-local.mjs'), '8911', join(here, 'seed.json')], {
  cwd: here, env: { ...process.env, NO_CACHE: '1' }, stdio: ['ignore', 'pipe', 'pipe'],
});
const seeded = await new Promise((resolve, reject) => {
  let buf = '';
  hub.stdout.on('data', (d) => { buf += d; const line = buf.split('\n').find((l) => l.startsWith('{')); if (line) resolve(JSON.parse(line)); });
  hub.on('exit', (code) => reject(new Error(`hub exited ${code}`)));
});
const id = (label) => seeded.answers[label].id;

const results = { checks: [], measures: {}, environment: {}, load: { start: loadavg() } };
let failed = 0;
function check(engine, name, ok, detail) {
  results.checks.push({ engine, name, ok: Boolean(ok), detail });
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} [${engine}] ${name}${detail ? ` -- ${typeof detail === 'string' ? detail : JSON.stringify(detail)}` : ''}`);
}
const median = (xs) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.floor(s.length / 2)] : null; };
const pct = (xs, q) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(s.length * q))] : null; };
const round = (x) => (x === null ? null : Math.round(x * 100) / 100);

// ---- helpers over one page
async function open(browser, path, { allow = true, viewport = { width: 1000, height: 760 }, colorScheme = 'light', storage } = {}) {
  const context = await browser.newContext({ viewport, colorScheme, storageState: storage });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  const sep = path.includes('?') ? '&' : '?';
  await page.goto(`${HUB}${path}${sep}probe=1`);
  await page.waitForFunction(() => window.__krate && window.__krate.state !== 'loading', null, { timeout: 15000 });
  if (allow && (await page.evaluate(() => window.__krate.state)) === 'consent') {
    await page.click('#consentAllow');
    await page.waitForFunction(() => window.__krate.state !== 'loading' && window.__krate.state !== 'consent', null, { timeout: 15000 });
  }
  return { context, page, errors };
}
const k = (page) => page.evaluate(() => window.__krate);
async function waitFrame(page, after) {
  await page.waitForFunction((n) => window.__krate.frames > n, after, { timeout: 10000 });
}
async function widget(page, pred) {
  const screen = (await k(page)).screen || [];
  return screen.find(pred);
}
async function canvasBox(page) { return page.locator('#screen').boundingBox(); }
async function clickAt(page, x, y) {
  const box = await canvasBox(page);
  const frames = (await k(page)).frames;
  await page.mouse.click(box.x + x, box.y + y);
  await waitFrame(page, frames).catch(() => {});
}
async function clickWidget(page, pred) {
  const w = await widget(page, pred);
  if (!w) return false;
  await clickAt(page, w.x + w.w / 2, w.y + w.h / 2);
  return true;
}
const settle = (page, ms = 150) => page.waitForTimeout(ms);

async function runEngine(name, type) {
  let browser;
  try {
    browser = await type.launch({ headless: true });
  } catch (e) {
    check(name, 'the browser starts', false, String(e.message).split('\n')[0]);
    return;
  }
  results.environment[name] = browser.version();
  const m = (results.measures[name] = {});

  // ---- 1. the CLI app: http through the bridge, the grant, no redirects
  {
    const { context, page } = await open(browser, `/run/${id('curl')}?arg=${encodeURIComponent(`${ECHO}/hello`)}`, { allow: false });
    check(name, 'curl: the consent screen asks in the desktop words', (await page.textContent('#consentRows')).includes('connect to 127.0.0.1:* over the network'));
    await page.screenshot({ path: join(shots, `${name}-curl-consent.png`) });
    await page.click('#consentAllow');
    await page.waitForFunction(() => window.__krate.exit !== undefined || window.__krate.trap, null, { timeout: 10000 });
    const r = await k(page);
    check(name, 'curl: fetches through the bridge and prints the body', r.out.includes('hello from the test server') && r.exit === 0, { out: r.out, exit: r.exit });
    await page.screenshot({ path: join(shots, `${name}-curl-ran.png`) });
    await context.close();
  }
  {
    const before = asked.length;
    const { context, page } = await open(browser, `/run/${id('curl')}?arg=${encodeURIComponent(`${ECHO}/redirect`)}`);
    await page.waitForFunction(() => window.__krate.exit !== undefined || window.__krate.trap, null, { timeout: 10000 });
    const r = await k(page);
    const since = asked.slice(before);
    check(name, 'curl: a redirect is not followed', since.includes('/redirect') && !since.includes('/elsewhere') && !r.out.includes('YOU-WERE-REDIRECTED'), { asked: since, out: r.out.trim() });
    await context.close();
  }
  {
    const before = asked.length;
    const { context, page } = await open(browser, `/run/${id('curl')}?arg=${encodeURIComponent('http://localhost:8913/hello')}`);
    await page.waitForFunction(() => window.__krate.exit !== undefined || window.__krate.trap, null, { timeout: 10000 });
    const r = await k(page);
    check(name, 'curl: a host the person did not grant is refused before anything is sent', asked.length === before && !r.out.includes('hello from'), { out: r.out.trim(), exit: r.exit });
    await context.close();
  }

  // ---- 2. hello-gui: every control
  {
    const { context, page, errors } = await open(browser, `/run/${id('hello')}`);
    await waitFrame(page, 0);
    await page.screenshot({ path: join(shots, `${name}-hello-first.png`) });
    let r = await k(page);
    check(name, 'hello-gui: opens with no consent (it asks for nothing beyond a window)', r.marks.consent === undefined && r.frames > 0);
    await clickWidget(page, (w) => w.label === 'Click me');
    await settle(page);
    check(name, 'hello-gui: the button answers', !!(await widget(page, (w) => w.label === 'clicked!')));
    await clickWidget(page, (w) => w.label === 'robot was here');
    await settle(page);
    check(name, 'hello-gui: the checkbox ticks', (await widget(page, (w) => w.label === 'robot was here'))?.checked === true);
    await clickWidget(page, (w) => w.kind === 'TextField');
    await page.keyboard.type('hi krate');
    await settle(page, 300);
    check(name, 'hello-gui: typing reaches the focused field', !!(await widget(page, (w) => w.kind === 'TextField' && w.label === 'clicked!hi krate')) || !!(await widget(page, (w) => w.kind === 'TextField' && (w.label || '').endsWith('hi krate'))));
    await page.keyboard.press('Backspace');
    await settle(page);
    check(name, 'hello-gui: Backspace deletes', !!(await widget(page, (w) => w.kind === 'TextField' && (w.label || '').endsWith('hi krat'))));
    await clickWidget(page, (w) => w.label === 'pick beta');
    await settle(page);
    check(name, 'hello-gui: a list row is chosen', (await widget(page, (w) => w.kind === 'ListView'))?.selected === true);
    const line = await widget(page, (w) => w.label === 'scroll line one');
    const box = await canvasBox(page);
    await page.mouse.move(box.x + line.x + 5, box.y + line.y + 5);
    await page.mouse.wheel(0, 40);
    await settle(page);
    const moved = await widget(page, (w) => w.label === 'scroll line one');
    check(name, 'hello-gui: the scroll area scrolls under the wheel', moved && moved.y < line.y, { before: line.y, after: moved && moved.y });
    // Enter on the focused button presses it (keyboard only).
    // Latency: the checkbox, pressed RUNS*2 times.
    const lat0 = (await k(page)).latencies.length;
    for (let i = 0; i < 20; i++) await clickWidget(page, (w) => w.label === 'robot was here');
    r = await k(page);
    m.helloClickToFrame = r.latencies.slice(lat0);
    m.helloClickToPainted = r.paintLatencies.slice(lat0);
    await page.emulateMedia({ colorScheme: 'dark' });
    const f = r.frames;
    await waitFrame(page, f).catch(() => {});
    await page.screenshot({ path: join(shots, `${name}-hello-dark.png`) });
    const darkPixel = await page.evaluate(() => { const c = document.getElementById('screen'); const d = c.getContext('2d').getImageData(c.width - 4, c.height - 4, 1, 1).data; return d[0] + d[1] + d[2]; });
    check(name, 'hello-gui: follows the browser into dark', darkPixel < 200, { corner: darkPixel });
    await page.click('#closeApp');
    await page.waitForFunction(() => window.__krate.exit !== undefined, null, { timeout: 5000 }).catch(() => {});
    r = await k(page);
    check(name, 'hello-gui: Close asks the app, and it reports what was typed', r.exit === 0 && r.out.includes('typed:hi krat'), { out: r.out, exit: r.exit });
    check(name, 'hello-gui: no page errors', errors.length === 0, errors);
    await context.close();
  }

  // ---- 3. bigscroll: five hundred rows, scrolled past the bottom
  {
    const { context, page } = await open(browser, `/run/${id('bigscroll')}`);
    await waitFrame(page, 0);
    const box = await canvasBox(page);
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    const first = await widget(page, (w) => w.kind === 'Text' && w.clip);
    const firstId = first && first.id;
    for (let i = 0; i < 80; i++) await page.mouse.wheel(0, 2000);
    await settle(page, 400);
    const screen = (await k(page)).screen;
    const rows = screen.filter((w) => w.kind === 'Text' && w.clip);
    const last = rows.reduce((a, b) => (b.y > a.y ? b : a));
    const clip = last.clip;
    const lastVisible = last.y + last.h <= clip[1] + clip[3] + 0.5 && last.y + last.h > clip[1];
    const firstNow = rows.find((w) => w.id === firstId);
    // One more turn at the end moves nothing: the offset is clamped.
    await page.mouse.wheel(0, 2000);
    await settle(page, 300);
    const still = ((await k(page)).screen || []).find((w) => w.id === last.id);
    check(name, 'bigscroll: the wheel reaches the last row and stops there', lastVisible && firstNow.y < first.y && still.y === last.y,
      { firstRowBefore: first.y, firstRowAfter: firstNow.y, lastRow: last.label, bottom: last.y + last.h, clipBottom: clip[1] + clip[3], afterOneMore: still.y });
    await page.screenshot({ path: join(shots, `${name}-bigscroll-bottom.png`) });
    await context.close();
  }

  // ---- 4. convert: buttons on a grid
  {
    const { context, page } = await open(browser, `/run/${id('convert')}`);
    await waitFrame(page, 0);
    const buttons = ((await k(page)).screen || []).filter((w) => w.kind === 'Button');
    let all = true;
    for (const b of buttons) { if (!(await clickWidget(page, (w) => w.id === b.id))) all = false; }
    await page.click('#closeApp');
    await page.waitForFunction(() => window.__krate.exit !== undefined, null, { timeout: 5000 }).catch(() => {});
    const r = await k(page);
    check(name, `convert: every preset button (${buttons.length}) answers`, all && buttons.length > 0 && /celsius:-?\d+/.test(r.out), { out: r.out.trim() });
    await context.close();
  }

  // ---- 5. checklist: canvas, typing, the store, a reload
  {
    const { context, page, errors } = await open(browser, `/run/${id('checklist')}`);
    await waitFrame(page, 0);
    await settle(page, 200);
    const box = await canvasBox(page);
    // The field sits at the bottom left of the app's own drawing.
    await page.mouse.click(box.x + box.width * 0.35, box.y + box.height - 40);
    await page.keyboard.type('Buy bread');
    await page.keyboard.press('Enter');
    await settle(page, 400);
    // A row toggles where the app drew the first one.
    await page.mouse.click(box.x + 60, box.y + 170 * (box.height / 612));
    await settle(page, 300);
    await page.screenshot({ path: join(shots, `${name}-checklist.png`) });
    const stored = await page.evaluate(async () => {
      const db = await new Promise((r, j) => { const o = indexedDB.open('krate-player', 1); o.onsuccess = () => r(o.result); o.onerror = () => j(o.error); });
      const all = await new Promise((r) => { const t = db.transaction('kv').objectStore('kv').getAll(); t.onsuccess = () => r(t.result); });
      return all.map((v) => new TextDecoder().decode(v)).join('|');
    });
    check(name, 'checklist: typed item saved through the store to the browser (IndexedDB)', stored.includes('Buy bread'), stored.slice(0, 200));
    const storage = await context.storageState({ indexedDB: true });
    check(name, 'checklist: no page errors', errors.length === 0, errors);
    await context.close();
    // A new tab, same browser storage: the list is still there.
    const again = await open(browser, `/run/${id('checklist')}`, { storage });
    await waitFrame(again.page, 0);
    await again.page.click('#closeApp');
    await again.page.waitForFunction(() => window.__krate.exit !== undefined, null, { timeout: 5000 }).catch(() => {});
    const r2 = await k(again.page);
    check(name, 'checklist: reopened, it has the saved items', /items:4/.test(r2.out), { out: r2.out.trim() });
    await again.context.close();
  }

  // ---- 6. refusals and guards
  {
    const { context, page } = await open(browser, `/run/${id('camera')}`, { allow: false });
    const t = await page.textContent('#consent');
    check(name, 'camera: a capability with no browser equivalent is refused in words before running',
      t.includes('use your camera') && t.includes('works in Krate on your computer') && (await page.textContent('#consentAllow')) === 'Open in Krate', t.replace(/\s+/g, ' ').slice(0, 220));
    await page.screenshot({ path: join(shots, `${name}-camera-refused.png`) });
    await context.close();
  }
  {
    const { context, page } = await open(browser, `/run/${id('orgprobe')}?arg=put&arg=notes&arg=n1&arg=text%3Dhello`);
    await page.waitForFunction(() => window.__krate.exit !== undefined || window.__krate.trap, null, { timeout: 10000 });
    const r = await k(page);
    // Team data works in a tab (G1, team-data-proof.mjs); an app that is
    // not an organisation's release has none there, as on a computer.
    check(name, 'store.org in a tab, for an app that is not an organisation release: not-an-org-app, as on a computer',
      /error=not-an-org-app/.test(r.out) && !r.notices.some((n) => n.iface.startsWith('krate:store/org')), { out: r.out.trim(), notices: r.notices });
    await page.screenshot({ path: join(shots, `${name}-orgstore-not-org.png`) });
    await context.close();
  }
  {
    const { context, page } = await open(browser, `/run/${id('tampered')}`);
    const r = await k(page);
    check(name, 'a bundle changed after signing is refused in the tab', r.state === 'refused' && /changed after it was signed/.test(r.refused.title), r.refused);
    await page.screenshot({ path: join(shots, `${name}-tampered.png`) });
    await context.close();
  }
  {
    const { context, page } = await open(browser, `/run/${id('swapped')}`);
    const r = await k(page);
    check(name, 'bytes that are not the ones the link names are refused', r.state === 'refused' && /changed on the way/.test(r.refused.title), r.refused);
    await context.close();
  }
  {
    const { context, page } = await open(browser, `/run/${id('signed')}`);
    await page.waitForFunction(() => window.__krate.frames > 0 || window.__krate.trap || window.__krate.exit !== undefined, null, { timeout: 10000 }).catch(() => {});
    const r = await k(page);
    check(name, 'a signed app opens and says who signed it', r.app && r.app.signature === 'valid' && (await page.textContent('#whoSig')).startsWith('Signed by'), { sig: r.app && r.app.signature, frames: r.frames, trap: r.trap });
    await page.screenshot({ path: join(shots, `${name}-signed.png`) });
    await context.close();
  }

  // ---- 7. the private organisation app
  {
    const { context, page } = await open(browser, `/run/${id('org')}`);
    const r = await k(page);
    check(name, 'org app: signed out, the tab asks for sign-in', r.state === 'signin', r.state);
    const hrefs = await page.evaluate(() => [document.getElementById('signinGithub').href, sessionStorage.getItem('krate-player-nonce')]);
    check(name, 'org app: sign-in starts on the hub and comes back to this app', hrefs[0].includes('/login/start?from=player%3A' + id('org')) && !!hrefs[1]);
    await context.close();
  }
  for (const [who, expect] of [['dan', 'runs'], ['stranger', 'refused']]) {
    const context = await browser.newContext();
    await context.addInitScript((token) => localStorage.setItem('krate-player-session', JSON.stringify({ token, login: '' })), `gho_${who}`);
    const page = await context.newPage();
    await page.goto(`${HUB}/run/${id('org')}`);
    await page.waitForFunction(() => window.__krate && window.__krate.state !== 'loading', null, { timeout: 15000 });
    if ((await k(page)).state === 'consent') await page.click('#consentAllow');
    await page.waitForTimeout(800);
    const r = await k(page);
    if (expect === 'runs') check(name, 'org app: a member signed in opens it in the tab (the private-link checks held)', r.state === 'app' && r.app && r.app.signature === 'valid', { state: r.state, refused: r.refused });
    else check(name, 'org app: someone it was not shared with is refused in words', r.state === 'refused' && /not shared with you/.test(r.refused.title), r.refused);
    await context.close();
  }

  // ---- 8. phone width
  {
    const { context, page } = await open(browser, `/run/${id('hello')}`, { viewport: { width: 375, height: 812 } });
    await waitFrame(page, 0);
    const size = await page.evaluate(() => { const c = document.getElementById('screen'); return [c.getBoundingClientRect().width, document.documentElement.scrollWidth]; });
    check(name, 'hello-gui at 375 wide: the window fits the phone, no sideways scroll', size[0] <= 375 && size[1] <= 375, size);
    await page.screenshot({ path: join(shots, `${name}-hello-375.png`) });
    await context.close();
  }

  // ---- measured: cold starts, each a fresh context (empty cache)
  const cold = { cliConsentShown: [], cliAllowToOutput: [], guiFirstFrame: [], canvasAllowToFirstFrame: [] };
  for (let i = 0; i < RUNS; i++) {
    {
      const { context, page } = await open(browser, `/run/${id('hello')}`);
      await waitFrame(page, 0);
      const r = await k(page);
      cold.guiFirstFrame.push(r.marks.firstFrame);
      await context.close();
    }
    {
      const { context, page } = await open(browser, `/run/${id('curl')}?arg=${encodeURIComponent(`${ECHO}/hello`)}`, { allow: false });
      const shown = (await k(page)).marks.consent;
      await page.click('#consentAllow');
      await page.waitForFunction(() => window.__krate.marks.firstOutput !== undefined, null, { timeout: 10000 });
      const r = await k(page);
      cold.cliConsentShown.push(shown);
      cold.cliAllowToOutput.push(r.marks.firstOutput - r.marks.allowed);
      await context.close();
    }
    {
      const { context, page } = await open(browser, `/run/${id('checklist')}`);
      await waitFrame(page, 0);
      const r = await k(page);
      cold.canvasAllowToFirstFrame.push(r.marks.firstFrame - r.marks.allowed);
      await context.close();
    }
  }
  for (const [key, xs] of Object.entries(cold)) m[key] = { median: round(median(xs)), min: round(Math.min(...xs)), max: round(Math.max(...xs)), n: xs.length };
  const lat = m.helloClickToFrame;
  m.helloClickToFrame = { median: round(median(lat)), p95: round(pct(lat, 0.95)), n: lat.length };
  const painted = m.helloClickToPainted;
  m.helloClickToPainted = { median: round(median(painted)), p95: round(pct(painted, 0.95)), n: painted.length };
  await browser.close();
}

try {
  const only = (process.env.ENGINES || 'chromium,webkit,firefox').split(',');
  for (const [name, type] of [['chromium', chromium], ['webkit', webkit], ['firefox', firefox]].filter(([n]) => only.includes(n))) {
    await runEngine(name, type).catch((e) => check(name, 'the run completes', false, String(e && e.stack || e).split('\n').slice(0, 3).join(' | ')));
  }
} finally {
  hub.kill();
  echo.close();
}
results.load.end = loadavg();
results.when = new Date().toISOString();
results.method = 'Headless Playwright 1.63.0 browsers against the real hub Worker in Node (hub-local.mjs) on 127.0.0.1, no HTTP cache (NO_CACHE). Times are ms from navigation start (performance.now) unless named as a difference; click-to-frame is pointerdown timeStamp to the putImageData that showed the change.';
writeFileSync(out, JSON.stringify(results, null, 2) + '\n');
console.log(`${results.checks.length - failed}/${results.checks.length} checks passed; wrote ${out}`);
process.exit(failed ? 1 : 0);
