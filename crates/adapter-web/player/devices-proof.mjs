// Serial and USB devices in the tab (IC-992): krate:serial through the
// browser's Web Serial and krate:usb through WebUSB, opened the way a person
// opens a shared link, against the hub Worker itself, running here in Node
// over the worker tests' stand-ins, and the Player in
// cloud/worker/player-dist (scripts/build-web-player.sh).
//
//   node --experimental-wasm-modules devices-proof.mjs [shot-dir]
//
// The apps are the desktop's own probes, apps/krate-serial-probe and
// apps/krate-usb-probe (their built components are
// crates/cli/tests/fixtures/serial-probe.wasm and usb-probe.wasm), packed
// here with their manifests and a few variants: the device optional, the
// device not declared, a keyboard named beside the dongle. Each run is one
// of the probe's checks (`?arg=talk`), and what the app printed is what is
// checked, word for word against what the desktop prints
// (crates/cli/tests/cli.rs, serial and usb).
//
// Chromium has Web Serial and WebUSB; a headless one has no device and no
// list to show, so the page is given scripted ones before the Player loads
// (fake-devices.mjs: Chrome's navigator.serial and navigator.usb, a serial
// device that answers PING with PONG, and the desktop fake's USB devices).
// The list opens only inside the person's tap, as Chrome's does. It checks:
//   (a) serial: the pick, talking PING/PONG, a quiet read, a spent handle,
//       forged handles, a refused baud rate, the port closed when the run
//       ends, a cancel, and an app without device.serial refused with no
//       list shown;
//   (b) USB: the pick, bulk, interrupt and control transfers byte for byte,
//       forged handles, the HID interface beside the dongle's own refused,
//       nothing outside the claimed interface, a Crazyradio's set-up,
//       requests to the whole device, a write the device never takes timed
//       out, everything let go when the run ends;
//   (c) the browser's list is filtered to exactly the models the manifest
//       names: a dongle of another model cannot be chosen, a browser that
//       hands one back anyway is refused, and a keyboard is never the
//       app's even when named;
//   (d) each list opened inside the person's tap;
//   (e) WebKit and Firefox, which have neither API: an app that needs a
//       device is not run, and the card says to open it in Chrome or Edge,
//       or in Krate; one for which the device is optional runs and hears
//       `unavailable` in those words;
//   (f) printing what the window shows (krate:ui/print, apps/krate-print-probe)
//       goes to the browser's own print dialog, in every engine: one page
//       and two, each the window's picture as it was when added, the job
//       named after the app's title, and only those pages on the paper; a
//       window that is not the app's and an empty document refused in the
//       desktop's words. The dialog is the browser's: headless, window.print
//       is recorded instead of shown;
//   (g) no console errors, and nothing says a device needs a computer.
// PLAYWRIGHT_MODULE and CHROMIUM may point at a playwright and a browser
// already on the machine.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';
import { consoleError } from './engine-abilities.mjs';
import { packKrate } from './pack.mjs';
import { fakeDevices } from './fake-devices.mjs';

const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const shots = process.argv[2] || null;
const port = 8962;
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

const probe = (name) => ({
  manifest: readFileSync(join(repo, 'apps', `krate-${name}-probe`, 'manifest.toml'), 'utf8'),
  wasm: readFileSync(join(repo, 'crates', 'cli', 'tests', 'fixtures', `${name}-probe.wasm`)),
});
const publish = async (what, extra = []) => {
  const p = probe(what);
  const up = await call(env, 'alice', 'POST', '/publish', { bytes: new Uint8Array(packKrate(p.manifest, p.wasm, extra)) });
  if (up.status !== 200) throw new Error(`publish ${what} ${extra.join(' ')}: ${up.status} ${JSON.stringify(up.body)}`);
  return up.body.id;
};
const APPS = {
  serial: await publish('serial'),
  serialOptional: await publish('serial', ['optional:device.serial']),
  serialBare: await publish('serial', ['without:device.serial']),
  usb: await publish('usb'),
  usbOptional: await publish('usb', ['optional:device.usb:1915:7777']),
  usbBare: await publish('usb', ['without:device.usb:1915:7777']),
  usbKeys: await publish('usb', ['device.usb:1915:0101']),
  print: await publish('print'),
};

const server = http.createServer(async (req, res) => {
  const chunks = [];
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

const ELSEWHERE = 'works in Chrome or Edge, or in Krate on a computer';
const NO_SERIAL = 'This browser cannot reach a serial device. Open the app in Chrome or Edge, or in Krate on a computer.';
const NO_USB = 'This browser cannot reach a USB device. Open the app in Chrome or Edge, or in Krate on a computer.';

const launched = {};
const skipped = [];
// An engine that will not start on this machine is said plainly and is not
// counted as passed: the run ends "not run" rather than "all passed" (exit 2,
// as ws-proof does).
const browserFor = async (engine) => {
  if (!(engine in launched)) {
    launched[engine] = await engines[engine].launch({
      headless: true,
      ...(engine === 'chromium' && process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}),
    }).catch((e) => { console.log(`SKIP ${engine} did not start here: ${String(e.message).split('\n').find((l) => /err\]/.test(l)) || 'see playwright'}`); return null; });
  }
  return launched[engine];
};

/// Open `app` with `?arg=<check>`, allow it, answer the device list as
/// `fake` says, and wait for the app to end. What it printed, what the
/// devices saw, what the browser was asked.
let visitors = 0;
async function run(engine, app, check, fake = {}) {
  const tag = `[${engine} ${check}]`;
  const browser = await browserFor(engine);
  // Each run is its own visitor: the hub counts opens by address, and this
  // proof opens far more than one person would.
  visitors += 1;
  const context = await browser.newContext({ viewport: { width: 1280, height: 1000 }, extraHTTPHeaders: { 'cf-connecting-ip': `10.9.${visitors >> 8}.${visitors & 255}` } });
  // The browser's print dialog, recorded: what would be on the paper.
  await context.addInitScript(() => {
    window.__prints = [];
    window.print = () => {
      const pages = Array.from(document.querySelectorAll('#printPages img'), (img) => {
        const c = document.createElement('canvas');
        c.width = img.naturalWidth;
        c.height = img.naturalHeight;
        const g = c.getContext('2d');
        g.drawImage(img, 0, 0);
        return { width: img.naturalWidth, height: img.naturalHeight, top: Array.from(g.getImageData(10, 10, 1, 1).data.slice(0, 3)) };
      });
      window.__prints.push({ title: document.title, printing: document.body.classList.contains('printing'), pages });
      dispatchEvent(new Event('afterprint'));
    };
  });
  if (engine === 'chromium') {
    await context.addInitScript(fakeDevices);
    await context.addInitScript((f) => { Object.assign(window.__fake, f); }, fake);
  }
  const page = await context.newPage();
  const errors = [];
  page.on('console', (m) => { if (consoleError(m)) errors.push(`${m.text()} @${JSON.stringify(m.location())}`); });
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`${base}/run/${app}?probe=1&arg=${encodeURIComponent(check)}`);
  await page.waitForFunction(() => window.__krate && (window.__krate.state === 'consent' || window.__krate.state === 'app'), null, { timeout: 30000 }).catch(() => {});
  const card = await page.evaluate(() => ({
    state: window.__krate.state,
    rows: Array.from(document.querySelectorAll('#consentRows li')).map((li) => ({ away: li.className === 'away', text: li.innerText })),
    note: document.getElementById('consentNote').textContent,
    allow: document.getElementById('consentAllow').textContent,
  }));
  const result = { tag, page, context, errors, card, sheets: [] };
  // A card that does not let the app open is the answer.
  if (card.state === 'consent' && card.allow !== 'Allow and open') return result;
  if (card.state === 'consent') await page.click('#consentAllow');
  // Until the app ends: the Player's sheet answered as the person would.
  const deadline = Date.now() + 30000;
  for (;;) {
    const now = await page.evaluate(() => ({
      done: window.__krate.exit !== undefined || Boolean(window.__krate.trap),
      sheet: !document.getElementById('askSheet').hidden,
    }));
    if (now.done || Date.now() > deadline) break;
    if (now.sheet) {
      result.sheets.push(await page.evaluate(() => ({ title: document.getElementById('askTitle').textContent, body: document.getElementById('askBody').textContent, yes: document.getElementById('askYes').textContent })));
      if (shots) await page.screenshot({ path: join(shots, `devices-proof-${engine}-${check}-sheet.png`) });
      await page.click(fake.decline ? '#askNo' : '#askYes');
    }
    await page.waitForTimeout(50);
  }
  Object.assign(result, await page.evaluate(() => ({
    out: window.__krate.out.split('\n').filter(Boolean),
    exit: window.__krate.exit,
    trap: window.__krate.trap,
    notices: window.__krate.notices,
    notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText,
    log: window.__fake ? window.__fake.log : [],
    asked: window.__fake ? window.__fake.asked : [],
    listed: window.__fake ? window.__fake.listed : undefined,
    prints: window.__prints,
  })));
  result.line = result.out[0] || `(none) trap=${result.trap}`;
  return result;
}
const finish = async (r) => {
  check(!r.trap && !(r.notices || []).length && !/computer/i.test(r.notice || ''), `${r.tag} nothing says a device needs a computer`, { trap: r.trap, notices: r.notices, notice: r.notice });
  check(r.errors.length === 0, `${r.tag} no console errors`, r.errors);
  if (shots) await r.page.screenshot({ path: join(shots, `devices-proof-${r.tag.slice(1, -1).replace(/ /g, '-')}.png`) });
  await r.context.close();
};
const inTap = (r) => r.asked.length > 0 && r.asked.every((a) => a.active);

// ------------------------------------------------------- (a) serial
{
  let r = await run('chromium', APPS.serial, 'pick');
  check(r.line === 'pick: picked USB serial device (0403:6001)', `${r.tag} the pick is the device the person chose in the browser's list`, r.line);
  check(inTap(r) && r.asked.length === 1 && r.asked[0].what === 'serial', `${r.tag} the browser's list opened once, inside the person's tap`, { asked: r.asked, sheets: r.sheets });
  check(r.sheets.every((s) => s.title === 'Choose the test device' && s.yes === 'Choose a device'), `${r.tag} a sheet, when one was needed, carries the app's title`, r.sheets);
  await finish(r);

  r = await run('chromium', APPS.serial, 'talk');
  check(r.line === 'talk: heard PONG, then nothing for 100 ms, closed: unknown-handle', `${r.tag} PING out, PONG back, a quiet read ends empty, the closed handle is spent`, r.line);
  check(r.log.includes('serial open 115200 8N1') && r.log.includes('serial write "PING\\n"') && r.log[r.log.length - 1] === 'serial close', `${r.tag} the device was opened at 115200 8N1, heard PING, and was closed`, r.log);
  await finish(r);

  r = await run('chromium', APPS.serial, 'forge');
  check(r.line === 'forge: unopened=not-open next=unknown-handle zero=unknown-handle', `${r.tag} handles the person never gave reach nothing; the pick is not open yet`, r.line);
  await finish(r);

  r = await run('chromium', APPS.serial, 'bad');
  check(r.line === 'bad: bad-settings: a baud rate is from 50 to 4000000, not 0', `${r.tag} a baud rate of 0 is refused in the desktop's words`, r.line);
  check(!r.log.some((l) => l.startsWith('serial open')), `${r.tag} and the port was never opened`, r.log);
  await finish(r);

  r = await run('chromium', APPS.serial, 'hold');
  await r.page.waitForTimeout(300);
  const held = await r.page.evaluate(() => window.__fake.log);
  check(r.line === 'hold: left open' && held.includes('serial write "HELD\\n"') && held[held.length - 1] === 'serial close', `${r.tag} a port the app left open is closed when the run ends`, { line: r.line, log: held });
  await finish(r);

  r = await run('chromium', APPS.serial, 'pick', { serialPick: 'cancel' });
  check(r.line === 'pick: none', `${r.tag} cancelling the browser's list is no device`, r.line);
  await finish(r);

  r = await run('chromium', APPS.serialBare, 'pick');
  check(r.line === 'pick: not-allowed: this app did not ask for device.serial, so it cannot talk to a serial device', `${r.tag} without device.serial, refused in the desktop's words`, r.line);
  check(r.asked.length === 0 && r.sheets.length === 0, `${r.tag} and no list or sheet was shown`, { asked: r.asked, sheets: r.sheets });
  await finish(r);
}

// ---------------------------------------------------------- (b) USB
const DONGLE = [{ vendorId: 0x1915, productId: 0x7777 }];
const sameFilters = (got, want) => JSON.stringify(got) === JSON.stringify(want);
{
  let r = await run('chromium', APPS.usb, 'pick');
  check(r.line === 'pick: picked Radio dongle (1915:7777) 1915:7777', `${r.tag} the pick says which device it is`, r.line);
  check(inTap(r) && r.asked.length === 1 && sameFilters(r.asked[0].filters, DONGLE), `${r.tag} the browser's list was asked for exactly the model the manifest names, inside the tap`, r.asked);
  await finish(r);

  r = await run('chromium', APPS.usb, 'talk');
  check(r.line === 'talk: bulk echoed 256 bytes, interrupt echoed 3 bytes, control kept 3 bytes, then nothing for 100 ms, closed: unknown-handle',
    `${r.tag} bulk, interrupt and control, byte for byte; a quiet read ends empty; the closed handle is spent`, r.line);
  const every = Array.from({ length: 256 }, (_, b) => b.toString(16).padStart(2, '0')).join('');
  check(r.log.includes(`out 1915:7777 0x01 ${every}`) && r.log.includes('out 1915:7777 0x02 00ff7f') && r.log.includes('control-out 1915:7777 vendor interface 1 7 0 090807'),
    `${r.tag} every byte reached the device as sent`, r.log);
  check(r.log.includes('release 1915:7777 0') && r.log[r.log.length - 1] === 'close 1915:7777', `${r.tag} the interface was released and the device closed`, r.log);
  await finish(r);

  r = await run('chromium', APPS.usb, 'forge');
  check(r.line === 'forge: unclaimed=not-claimed next=unknown-handle zero=unknown-handle', `${r.tag} handles the person never gave reach nothing`, r.line);
  await finish(r);

  r = await run('chromium', APPS.usb, 'hid');
  check(r.line.startsWith('hid: not-for-apps: interface 1 of Radio dongle (1915:7777) is a keyboard, mouse or other input device'), `${r.tag} the HID interface beside the dongle's own is the computer's`, r.line);
  check(!r.log.some((l) => l.startsWith('claim')), `${r.tag} and no claim reached the device`, r.log);
  await finish(r);

  r = await run('chromium', APPS.usb, 'outside');
  check(r.line === 'outside: interface-1=bad-request set-configuration=bad-request endpoint-0x83=bad-request', `${r.tag} nothing outside the claimed interface`, r.line);
  check(!r.log.some((l) => l.startsWith('control') || l.startsWith('in ')), `${r.tag} nothing refused reached the device`, r.log);
  await finish(r);

  r = await run('chromium', APPS.usb, 'radio');
  check(r.line === 'radio: channel 80, data rate 2, null packet answered [ff]', `${r.tag} a Crazyradio's set-up, as sent, then the null packet and its answer`, r.line);
  check(r.log.includes('control-out 1915:7777 vendor device 1 80 0 ') && r.log.includes('control-out 1915:7777 vendor device 3 2 0 '), `${r.tag} the vendor requests went to the whole device`, r.log);
  await finish(r);

  r = await run('chromium', APPS.usb, 'device');
  check(r.line === 'device: class=sent set-configuration=bad-request get-descriptor=18 bytes', `${r.tag} a class request to the device is sent, SET_CONFIGURATION is not, GET_DESCRIPTOR reads`, r.line);
  await finish(r);

  r = await run('chromium', APPS.usb, 'slow');
  check(r.line === 'slow: timed-out', `${r.tag} a write the device never takes ends at its wait`, r.line);
  await finish(r);

  r = await run('chromium', APPS.usb, 'hold');
  await r.page.waitForTimeout(300);
  const held = await r.page.evaluate(() => window.__fake.log);
  check(r.line === 'hold: left open' && held.includes('release 1915:7777 0') && held[held.length - 1] === 'close 1915:7777', `${r.tag} a device the app left open is let go when the run ends`, { line: r.line, log: held });
  await finish(r);

  r = await run('chromium', APPS.usb, 'pick', { usbPick: 'cancel' });
  check(r.line === 'pick: none', `${r.tag} cancelling the browser's list is no device`, r.line);
  await finish(r);
}

// --------------------------------------------- (c) the model filter
{
  // The person reaches for a dongle of a model the manifest does not name:
  // the browser's list, filtered, does not have it.
  let r = await run('chromium', APPS.usb, 'pick', { usbPick: '1915:7778' });
  check(r.line === 'pick: none' && sameFilters(r.asked[0] && r.asked[0].filters, DONGLE) && JSON.stringify(r.listed) === '["1915:7777"]',
    `${r.tag} a model the manifest does not name is not in the browser's list, and cannot be chosen`, { line: r.line, asked: r.asked, listed: r.listed });
  await finish(r);

  // A browser that hands it back anyway: refused, and never opened.
  r = await run('chromium', APPS.usb, 'pick', { usbPick: '1915:7778', usbRogue: true });
  check(r.line === 'pick: not-allowed: this app did not ask for device.usb:1915:7778, so it cannot talk to that USB device', `${r.tag} a device of another model handed back anyway is refused in the desktop's words`, r.line);
  check(!r.log.some((l) => l.startsWith('open')), `${r.tag} and was never opened`, r.log);
  await finish(r);

  // A keyboard named and allowed is still the computer's.
  r = await run('chromium', APPS.usbKeys, 'pick', { usbPick: '1915:0101' });
  check(r.line === 'pick: not-for-apps: Keyboard (1915:0101) is a keyboard, mouse or other input device, which the computer\'s own drivers serve; Krate does not hand it to an app',
    `${r.tag} a keyboard, named and allowed, is never the app's`, r.line);
  check(!r.log.some((l) => l.startsWith('open')) && sameFilters(r.asked[0] && r.asked[0].filters, [...DONGLE, { vendorId: 0x1915, productId: 0x0101 }]), `${r.tag} it was never opened; the list held the two named models`, { log: r.log, asked: r.asked });
  await finish(r);

  r = await run('chromium', APPS.usbBare, 'pick');
  check(r.line === 'pick: not-allowed: this app did not ask for device.usb, so it cannot talk to a USB device', `${r.tag} without device.usb, refused in the desktop's words`, r.line);
  check(r.asked.length === 0 && r.sheets.length === 0, `${r.tag} and no list or sheet was shown`, { asked: r.asked, sheets: r.sheets });
  await finish(r);
}

// ----------------------------------------------------- (f) printing
const near = (got, want) => got && want.every((v, i) => Math.abs(got[i] - v) <= 3);
const BLUE = [33, 89, 204];
const ORANGE = [204, 77, 26];
for (const engine of ['chromium', 'webkit', 'firefox']) {
  if (!(await browserFor(engine))) { if (!skipped.includes(engine)) skipped.push(engine); continue; }
  let r = await run(engine, APPS.print, 'one');
  r.tag = `[${engine} print one]`;
  const one = (r.prints || [])[0];
  check(r.line === 'printed' && r.prints.length === 1 && one.title === 'Rate card' && one.printing && one.pages.length === 1 && one.pages[0].width >= 300 && near(one.pages[0].top, BLUE),
    `${r.tag} what the window shows goes to the browser's print dialog, as one page named after the app's title`, { line: r.line, prints: r.prints });
  // On paper: the pages, and nothing else of the Player.
  const paper = await r.page.evaluate(() => {
    document.body.classList.add('printing');
    document.getElementById('printPages').appendChild(document.createElement('img'));
    return null;
  }).then(() => r.page.emulateMedia({ media: 'print' })).then(() => r.page.evaluate(() => ({
    shown: Array.from(document.body.children).filter((e) => getComputedStyle(e).display !== 'none').map((e) => e.id || e.tagName),
  })));
  check(JSON.stringify(paper.shown) === '["printPages"]', `${r.tag} on paper, only the app's pages`, paper);
  await finish(r);

  r = await run(engine, APPS.print, 'two');
  r.tag = `[${engine} print two]`;
  const two = (r.prints || [])[0];
  check(r.line === 'pages=2 printed' && r.prints.length === 1 && two.title === 'Rate card and terms' && two.pages.length === 2 && near(two.pages[0].top, BLUE) && near(two.pages[1].top, ORANGE),
    `${r.tag} two pages, each the window as it was when added, in one dialog`, { line: r.line, prints: r.prints });
  await finish(r);

  r = await run(engine, APPS.print, 'bad-window');
  r.tag = `[${engine} print bad-window]`;
  check(r.line === 'error=invalid-window' && r.prints.length === 0, `${r.tag} a window that is not the app's is refused, and nothing printed`, { line: r.line, prints: r.prints });
  await finish(r);

  r = await run(engine, APPS.print, 'empty');
  r.tag = `[${engine} print empty]`;
  check(r.line === 'error=other' && r.prints.length === 0, `${r.tag} a document with no pages is refused, and nothing printed`, { line: r.line, prints: r.prints });
  await finish(r);
}

// ----------------------------------- (e) browsers without the APIs
for (const engine of ['webkit', 'firefox']) {
  if (!(await browserFor(engine))) { if (!skipped.includes(engine)) skipped.push(engine); continue; }
  for (const [kind, app, optional, words] of [['serial', APPS.serial, APPS.serialOptional, NO_SERIAL], ['usb', APPS.usb, APPS.usbOptional, NO_USB]]) {
    let r = await run(engine, app, 'pick');
    const tag = `[${engine} ${kind} required]`;
    // Asked of the browser, not assumed from its name: Firefox began
    // shipping Web Serial, and an engine that has the API is not a
    // "browser without it". Its device runs are the scripted ones above.
    const api = kind === 'serial' ? 'serial' : 'usb';
    if (await r.page.evaluate((n) => Boolean(navigator[n]), api).catch(() => false)) {
      console.log(`note ${tag} this ${engine} has navigator.${api} natively, so the checks for a browser without it do not apply`);
      await r.context.close();
      continue;
    }
    const row = r.card.rows.find((x) => /device/.test(x.text));
    check(r.card.state === 'consent' && row && row.away && row.text.includes(ELSEWHERE), `${tag} the card says where the device works`, r.card);
    check(/This browser cannot reach one: open the app in Chrome or Edge, or in Krate on a computer\./.test(r.card.note) && r.card.allow === 'Open in Krate',
      `${tag} the app is not run, and the card offers to open it in Krate`, r.card);
    if (shots) await r.page.screenshot({ path: join(shots, `devices-proof-${engine}-${kind}-card.png`) });
    check(r.errors.length === 0, `${tag} no console errors`, r.errors);
    await r.context.close();

    r = await run(engine, optional, 'pick');
    r.tag = `[${engine} ${kind} optional]`;
    const greyed = r.card.rows.find((x) => /device/.test(x.text));
    check(greyed && greyed.away && greyed.text.includes(ELSEWHERE), `${r.tag} the device is greyed out on the card, where it works said`, r.card);
    check(r.line === `pick: unavailable: ${words}`, `${r.tag} the app runs, and hears unavailable in plain words`, r.line);
    check(r.sheets.length === 0, `${r.tag} and no sheet was shown`, r.sheets);
    await finish(r);
  }
}

for (const b of Object.values(launched)) if (b) await b.close();
server.close();
console.log(failed ? `${failed} failed` : skipped.length ? `passed, but not run in ${skipped.join(', ')}` : 'all passed');
process.exit(failed ? 1 : skipped.length ? 2 : 0);
