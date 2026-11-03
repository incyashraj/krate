// Dialogs and picked files in the tab (IC-974): krate:ui/dialog and
// fs.files.open-chosen, opened the way a person opens a shared link --
// Chromium on a computer and as a phone, WebKit as an iPhone -- against the
// hub Worker itself, running here in Node over the worker tests' stand-ins,
// and the Player in cloud/worker/player-dist (scripts/build-web-player.sh).
//
//   node --experimental-wasm-modules file-proof.mjs [shot-dir]
//
// The app is File Check (fixtures/file-check), packed twice:
//   (cd fixtures/file-check && cargo component build --release)
//   W=fixtures/file-check/target/wasm32-wasip1/release/krate_file_check.wasm
//   node pack.mjs fixtures/file-check/manifest.toml $W ../../../evidence/web-player/file-check.krate
//   node pack.mjs fixtures/file-check/manifest-ungranted.toml $W ../../../evidence/web-player/file-check-ungranted.krate
//
// It checks, by what the app printed and what the browser did:
//   (a) the consent card does not send the file dialogs to a computer;
//   (b) a picker asked for with no tap behind it (seven seconds after the
//       last one) is a one-tap sheet, and
//       the file chosen through it reads back exactly: whole, by seeking,
//       its size from the handle, nothing past its end, no writing to it;
//   (c) a picker asked for from a tap opens the browser's picker (on a
//       computer from a held press, inside its release), and the file
//       reads back exactly;
//   (d) a cancelled picker is none, not an error;
//   (e) a save: the app writes a note and lets go of it, the person gets a
//       download of that name with exactly those bytes, and the same token
//       reads back what was written;
//   (f) yes/no answers yes and no, a message answers ok;
//   (g) a dropped text file reaches the app with a token and reads back
//       exactly; a dropped picture does not (the app takes text only);
//   (h) a token nobody was handed is not found;
//   (i) the same app with no file dialog granted is refused with
//       permission-denied, as on desktop, and shows no picker and no sheet;
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
const port = 8936;
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

const publish = async (file) => {
  const bytes = new Uint8Array(readFileSync(join(repo, 'evidence', 'web-player', file)));
  const up = await call(env, 'alice', 'POST', '/publish', { bytes });
  if (up.status !== 200) throw new Error(`publish ${file}: ${up.status} ${JSON.stringify(up.body)}`);
  return up.body.id;
};
const granted = await publish('file-check.krate');
const ungranted = await publish('file-check-ungranted.krate');

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

// The app's own hash, over the bytes it was given.
const fnv = (bytes) => {
  let h = 0xcbf29ce484222325n;
  for (const b of bytes) { h ^= BigInt(b); h = (h * 0x100000001b3n) & 0xffffffffffffffffn; }
  return h.toString(16).padStart(16, '0');
};
const hex = (bytes) => Buffer.from(bytes).toString('hex');
// What the app should say about a file: whole, at byte 100, its end.
const expect = (name, bytes) => `${name} size=${bytes.length} len=${bytes.length} fnv=${fnv(bytes)} seek=100 at100=${hex(bytes.subarray(100, 104))} end=${bytes.length} past-end=0 write=permission-denied`;
// A file larger than one read window (1 MiB) and with every byte value.
const BIG = Buffer.alloc((1 << 20) + 4099);
for (let i = 0; i < BIG.length; i++) BIG[i] = (i * 31 + (i >> 9)) & 0xff;
const SMALL = Buffer.from(`Picked from a tab.\n${'0123456789'.repeat(20)}\n`);
const DROPPED = Buffer.from(`Dropped on the app.\n${'abcdefghij'.repeat(15)}\n`);
const NOTE = Buffer.concat([Buffer.from('Saved from a tab\n'), Buffer.from(Array.from({ length: 256 }, (_, i) => i))]);

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
const measured = [];

for (const [engine, label, viewport, ua] of RUNS) {
  const tag = `[${engine} ${label}]`;
  const touch = Boolean(ua);
  const browser = (launched[engine] ||= await engines[engine].launch({ headless: true, ...(engine === 'chromium' && process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) }));
  const context = await browser.newContext({ viewport, userAgent: ua, isMobile: touch, hasTouch: touch, acceptDownloads: true });
  const page = await context.newPage();
  const errors = [];
  page.on('console', (m) => { if (consoleError(m)) errors.push(`${m.text()} @${JSON.stringify(m.location())}`); });
  page.on('pageerror', (e) => errors.push(String(e)));
  const press = (sel) => (touch ? page.tap(sel) : page.click(sel));
  const lines = () => page.evaluate(() => window.__krate.out.split('\n').filter(Boolean));
  const line = async (prefix, timeout = 15000) => {
    await page.waitForFunction((p) => window.__krate.out.split('\n').some((l) => l.startsWith(p)) || window.__krate.trap, prefix, { timeout }).catch(() => {});
    return (await lines()).find((l) => l.startsWith(prefix)) || `(none) trap=${await page.evaluate(() => window.__krate.trap)}`;
  };
  const sheetUp = () => page.waitForFunction(() => !document.getElementById('askSheet').hidden, null, { timeout: 8000 }).then(() => true).catch(() => false);
  const sheetWords = () => page.evaluate(() => ({ title: document.getElementById('askTitle').textContent, body: document.getElementById('askBody').textContent, yes: document.getElementById('askYes').textContent, no: document.getElementById('askNo').hidden ? '' : document.getElementById('askNo').textContent }));
  // The app's own picker: the browser's chooser, caught; or the sheet,
  // whose tap opens it.
  const choose = async (act, files, timeout = 8000) => {
    const chooser = page.waitForEvent('filechooser', { timeout }).catch(() => null);
    await act();
    let fc = await Promise.race([chooser, page.waitForFunction(() => !document.getElementById('askSheet').hidden, null, { timeout }).then(() => 'sheet').catch(() => null)]);
    let words = null;
    if (fc === 'sheet') {
      words = await sheetWords();
      if (shots) await page.screenshot({ path: join(shots, `file-proof-${engine}-${label}-choose.png`) });
      const again = page.waitForEvent('filechooser', { timeout: 8000 }).catch(() => null);
      await press('#askYes');
      fc = await again;
    }
    if (fc && fc !== 'sheet') await fc.setFiles(files);
    const how = await page.evaluate(() => window.__krate.dialogs.filter((d) => d.kind === 'open').map((d) => d.how).pop());
    return { opened: Boolean(fc && fc !== 'sheet'), how, words, accept: fc && fc !== 'sheet' ? await fc.element().getAttribute('accept') : null };
  };

  await page.goto(`${base}/run/${granted}?probe=1`);
  await page.waitForFunction(() => window.__krate && window.__krate.state === 'consent', null, { timeout: 30000 }).catch(() => {});
  const card = await page.evaluate(() => document.body.innerText);
  // (a)
  check(/choose a file|file/i.test(card) && !/computer/i.test(card), `${tag} the card asks for the file dialogs and does not send them to a computer`, card.slice(0, 500));
  await press('#consentAllow');

  // (b) on start, nobody's tap behind it
  const start = await choose(async () => {}, { name: 'large.bin', mimeType: 'application/octet-stream', buffer: BIG }, 20000);
  check(start.opened && start.how === 'sheet', `${tag} with no tap behind it, the picker is one tap on a sheet away`, start);
  check(start.words && start.words.yes === 'Choose a file' && start.words.no === 'Cancel', `${tag} the sheet says "Choose a file"`, start.words);
  check(start.accept === '.bin,.txt', `${tag} the picker is told the app's filter`, start.accept);
  const startLine = await line('start-open: ', 30000);
  check(startLine === `start-open: ${expect('large.bin', BIG)}`, `${tag} a 1 MiB+ file reads back exactly, by seeking too, nothing past its end, and is not writable`, { got: startLine, want: `start-open: ${expect('large.bin', BIG)}` });
  // (h)
  check((await line('forged-token: ')) === 'forged-token: not-found', `${tag} a token nobody was handed is not found`, await lines());
  await line('ready');

  // (c) from a tap
  // On a computer the button is held a moment, so the app asks while it is
  // still down: the picker opens inside the release.
  const holdPress = async () => {
    const box = await page.locator('#screen').boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.waitForTimeout(400);
    await page.mouse.up();
  };
  const tapped = await choose(touch ? () => press('#screen') : holdPress, { name: 'picked.txt', mimeType: 'text/plain', buffer: SMALL });
  const tapLine = await line('tap-open: ');
  check(tapped.opened && tapLine === `tap-open: ${expect('picked.txt', SMALL)}`, `${tag} from a tap the picker opens and the file reads back exactly`, { tapped, got: tapLine, want: `tap-open: ${expect('picked.txt', SMALL)}` });
  measured.push({ run: tag, start: start.how, tap: tapped.how });

  // (d) cancelled
  const cancelled = await choose(() => press('#screen'), []);
  check(cancelled.opened && (await line('tap-cancel: ')) === 'tap-cancel: none', `${tag} a cancelled picker is none`, { cancelled, lines: await lines() });
  if (cancelled.how) measured[measured.length - 1].cancel = cancelled.how;

  // (e) a save
  const download = page.waitForEvent('download', { timeout: 10000 }).catch(() => null);
  await press('#screen');
  let dl = await Promise.race([download, page.waitForFunction(() => !document.getElementById('askSheet').hidden, null, { timeout: 10000 }).then(() => 'sheet').catch(() => null)]);
  let saveWords = null;
  if (dl === 'sheet') {
    saveWords = await sheetWords();
    const again = page.waitForEvent('download', { timeout: 10000 }).catch(() => null);
    await press('#askYes');
    dl = await again;
  }
  const saveHow = await page.evaluate(() => window.__krate.dialogs.filter((d) => d.kind === 'save').map((d) => d.how).pop());
  measured[measured.length - 1].save = saveHow;
  const saveLine = await line('save: ');
  const got = dl && dl !== 'sheet' ? await readFile(await dl.path()) : null;
  check(saveLine === `save: note.txt wrote=${NOTE.length} fnv=${fnv(NOTE)} reread=${fnv(NOTE)}`, `${tag} the app writes the note, lets go of it, and reads back what it wrote`, saveLine);
  check(got && dl.suggestedFilename() === 'note.txt' && Buffer.compare(got, NOTE) === 0, `${tag} the person gets note.txt as a download, byte for byte`, { name: dl && dl !== 'sheet' && dl.suggestedFilename(), size: got && got.length, saveWords, saveHow });

  // (f) yes, no, ok
  const answers = [];
  for (const [button, prefix] of [['#askYes', 'confirm: '], ['#askNo', 'confirm: '], ['#askYes', 'message: ']]) {
    const before = (await lines()).length;
    await press('#screen');
    const up = await sheetUp();
    const words = up ? await sheetWords() : null;
    if (up && shots && answers.length === 0) await page.screenshot({ path: join(shots, `file-proof-${engine}-${label}-confirm.png`) });
    if (up) await press(button);
    await page.waitForFunction((n) => window.__krate.out.split('\n').filter(Boolean).length > n, before, { timeout: 8000 }).catch(() => {});
    answers.push({ words, said: (await lines()).slice(before).find((l) => l.startsWith(prefix)) });
  }
  check(answers[0].said === 'confirm: yes' && answers[1].said === 'confirm: no' && answers[2].said === 'message: ok', `${tag} yes/no answers yes and no, a message answers ok`, answers);
  check(answers[0].words && answers[0].words.title === 'Keep going?' && answers[0].words.yes === 'Yes' && answers[0].words.no === 'No' && answers[2].words && answers[2].words.yes === 'OK' && !answers[2].words.no,
    `${tag} the sheet carries the app's own title, Yes/No for a question and OK for a message`, answers.map((a) => a.words));

  // (g) drops: a picture the app does not take, then a text file it does
  const drop = (fileName, type, bytes) => page.evaluate(([n, t, b]) => {
    const dt = new DataTransfer();
    dt.items.add(new File([new Uint8Array(b)], n, { type: t }));
    const canvas = document.getElementById('screen');
    const r = canvas.getBoundingClientRect();
    const at = { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + 20, clientY: r.top + 20 };
    canvas.dispatchEvent(new DragEvent('dragenter', at));
    canvas.dispatchEvent(new DragEvent('dragover', at));
    canvas.dispatchEvent(new DragEvent('drop', at));
  }, [fileName, type, [...bytes]]);
  await drop('photo.png', 'image/png', SMALL);
  await drop('dropped.txt', 'text/plain', DROPPED);
  const dropLine = await line('dropped: ');
  check(dropLine === `dropped: ${expect('dropped.txt', DROPPED)}`, `${tag} a dropped text file reaches the app and reads back exactly`, { got: dropLine, want: `dropped: ${expect('dropped.txt', DROPPED)}` });
  const all = await lines();
  check(!all.some((l) => /photo\.png|hovering/.test(l)), `${tag} a dropped picture and the drag over it do not reach an app that takes text only`, all);

  // (j)
  const r = await page.evaluate(() => ({ trap: window.__krate.trap, notices: window.__krate.notices, notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText }));
  // Playwright's screenshot injects a style the Player's CSP refuses.
  const seen = errors.filter((e) => !(shots && /style/i.test(e)));
  check(!r.trap && !r.notices.length && !/computer/i.test(r.notice), `${tag} nothing says a file dialog needs a computer`, r);
  check(seen.length === 0, `${tag} no console errors`, seen);
  if (shots) await page.screenshot({ path: join(shots, `file-proof-${engine}-${label}.png`) });
  await page.close();

  // (i) the same app, no file dialog granted
  const bare = await context.newPage();
  const bareErrors = [];
  bare.on('console', (m) => { if (consoleError(m)) bareErrors.push(m.text()); });
  bare.on('pageerror', (e) => bareErrors.push(String(e)));
  let chooserShown = false;
  bare.on('filechooser', () => { chooserShown = true; });
  await bare.goto(`${base}/run/${ungranted}?probe=1`);
  await bare.waitForFunction(() => window.__krate && (window.__krate.state === 'consent' || window.__krate.state === 'app'), null, { timeout: 30000 }).catch(() => {});
  if (await bare.evaluate(() => window.__krate.state === 'consent')) await (touch ? bare.tap('#consentAllow') : bare.click('#consentAllow'));
  await bare.waitForFunction(() => /ready/.test(window.__krate.out) || window.__krate.trap, null, { timeout: 20000 }).catch(() => {});
  await bare.evaluate(([b]) => {
    const dt = new DataTransfer();
    dt.items.add(new File([new Uint8Array(b)], 'dropped.txt', { type: 'text/plain' }));
    const canvas = document.getElementById('screen');
    for (const t of ['dragenter', 'dragover', 'drop']) canvas.dispatchEvent(new DragEvent(t, { bubbles: true, cancelable: true, dataTransfer: dt }));
  }, [[...DROPPED]]);
  await bare.waitForTimeout(600);
  const b = await bare.evaluate(() => ({ out: window.__krate.out.split('\n').filter(Boolean), sheet: !document.getElementById('askSheet').hidden, dialogs: window.__krate.dialogs, trap: window.__krate.trap, notices: window.__krate.notices }));
  check(b.out[0] === 'start-open: permission-denied' && b.out[1] === 'start-save: permission-denied',
    `${tag} without the grant, open-file and save-file answer permission-denied, as on desktop`, b.out);
  check(!chooserShown && !b.sheet && !b.dialogs.length && !b.out.some((l) => l.startsWith('dropped')),
    `${tag} without the grant, no picker, no sheet, and a drop reaches nothing`, { chooserShown, ...b });
  check(!b.trap && !b.notices.length && bareErrors.length === 0, `${tag} without the grant, no trap, notice or console error`, { trap: b.trap, notices: b.notices, bareErrors });
  await bare.close();
  await context.close();
}

for (const b of Object.values(launched)) await b.close();
server.close();
console.log('how each picker and save reached the person (sheet = one tap on the sheet; release/direct = inside the tap):');
for (const m of measured) console.log(`  ${m.run} start=${m.start} tap=${m.tap} cancel=${m.cancel} save=${m.save}`);
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
