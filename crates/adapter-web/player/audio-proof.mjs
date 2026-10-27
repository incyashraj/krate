// Sound in the tab (K-1134), opened in Chromium the way a person opens a
// shared link -- a desktop and a phone -- against the hub Worker itself,
// running here in Node over the worker tests' stand-ins, and the Player in
// cloud/worker/player-dist (scripts/build-web-player.sh).
//
//   node --experimental-wasm-modules audio-proof.mjs [shot-dir]
//
// Two apps:
//   - the gallery's Jungle Assault (e3a1160d), as published: music from
//     loaded notes, shots, jumps and explosions, all load-sound/play-sound;
//     the app the bug was found with;
//   - Sound Check (fixtures/sound-check, evidence/web-player/sound-check.krate):
//     a tone streamed through write and paced by queued, and a click played
//     twice at once on every press -- every playback path, deterministic.
//     Remade with:
//       (cd fixtures/sound-check && cargo component build --release)
//       node pack.mjs fixtures/sound-check/manifest.toml \
//         fixtures/sound-check/target/wasm32-wasip1/release/krate_sound_check.wasm \
//         ../../../evidence/web-player/sound-check.krate
//
// It checks, by what the page and its audio thread actually did:
//   (a) the app opens and paints with no tap, and no line says sound only
//       works in Krate on a computer; nothing is heard yet, and a stream
//       still drains at a speaker's pace so the app is never left waiting;
//   (b) after one click (a tap on the phone) the AudioContext is running,
//       the audio thread renders, and what reaches the graph's output is
//       not silence (an AnalyserNode after the worklet);
//   (c) two plays of one sound sound at once, mixed;
//   (d) nothing on the console is an error, and the app keeps drawing.
// Chromium runs headless and muted: the proof hears nothing on this machine.
// PLAYWRIGHT_MODULE and CHROMIUM may point at a playwright and a browser
// already on the machine.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
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

const publish = async (file) => {
  const bytes = new Uint8Array(readFileSync(join(repo, 'evidence', 'web-player', file)));
  const up = await call(env, 'alice', 'POST', '/publish', { bytes });
  if (up.status !== 200) throw new Error(`publish ${file}: ${up.status} ${JSON.stringify(up.body)}`);
  return up.body.id;
};
const APPS = [
  { name: 'Jungle Assault', short: 'jungle', hash: await publish('jungle-assault.krate') },
  { name: 'Sound Check', short: 'check', hash: await publish('sound-check.krate') },
];

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

// Muted: nothing is played through this machine's speakers.
const browser = await chromium.launch({ headless: true, args: ['--mute-audio'], ...(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}) });
let failed = 0;
const check = (ok, what, detail) => {
  if (!ok) failed += 1;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}${ok ? '' : ` ${JSON.stringify(detail)}`}`);
};
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1';
// audio.js's stats words.
const S = { quanta: 0, loud: 1, voices: 2, maxVoices: 3, maxCopies: 4, peak: 5, clamped: 6, played: 7 };
const named = (a) => (a ? Object.fromEntries(Object.entries(S).map(([k, i]) => [k, a[i]])) : null);
// Playwright's own evaluate runs as a user gesture, which would count as the
// person's tap and let the sound start; before the tap the page is read
// through CDP with userGesture off, so only the clicks below are gestures.
const quietly = async (cdp, fn) => {
  const r = await cdp.send('Runtime.evaluate', { expression: `JSON.stringify((${fn})())`, returnByValue: true, userGesture: false });
  if (r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
  return JSON.parse(r.result.value);
};
const look = () => ({
  activated: navigator.userActivation.hasBeenActive,
  state: window.__krate.state,
  frames: window.__krate.frames,
  trap: window.__krate.trap,
  notices: window.__krate.notices,
  notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText,
  audio: window.__krate.audio,
  stats: window.__krateAudio && window.__krateAudio.stats(),
  streams: window.__krateAudio && window.__krateAudio.streams(),
  at: performance.now(),
  canvas: (() => { const c = document.getElementById('screen'); const r = c.getBoundingClientRect(); return { w: r.width, h: r.height, vw: innerWidth, scroll: document.documentElement.scrollWidth }; })(),
  // Not a black screen: the pixels on the canvas vary.
  colours: (() => {
    const c = document.getElementById('screen');
    const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
    const seen = new Set();
    for (let i = 0; i < d.length; i += 4 * 97) seen.add((d[i] << 16) | (d[i + 1] << 8) | d[i + 2]);
    return seen.size;
  })(),
});

for (const app of APPS) {
  for (const [label, viewport, ua] of [['desktop', { width: 1280, height: 800 }, undefined], ['phone', { width: 375, height: 812 }, IPHONE]]) {
    const tag = `[${app.short} ${label}]`;
    const context = await browser.newContext({ viewport, userAgent: ua, isMobile: Boolean(ua), hasTouch: Boolean(ua) });
    const page = await context.newPage();
    const errors = [];
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
    page.on('pageerror', (e) => errors.push(String(e)));
    const cdp = await context.newCDPSession(page);
    await page.goto(`${base}/run/${app.hash}?probe=1`);

    // (a) open and paint, no tap.
    for (let t = 0; t < 300; t++) {
      if (await quietly(cdp, () => Boolean(window.__krate && window.__krate.frames > 30))) break;
      await page.waitForTimeout(100);
    }
    await page.waitForTimeout(500);
    const first = await quietly(cdp, look);
    await page.waitForTimeout(1000);
    const before = await quietly(cdp, look);
    check(!before.activated, `${tag} nothing has tapped the page yet`, before.activated);
    check(before.state === 'app' && before.frames > 30 && !before.trap, `${tag} ${app.name} opens and draws with no tap`, before);
    check(before.colours > 5, `${tag} the screen is the app, not a black canvas`, before.colours);
    check(before.canvas.w > 0 && before.canvas.w <= before.canvas.vw && before.canvas.scroll <= before.canvas.vw, `${tag} the app fits the tab, nothing scrolls sideways`, before.canvas);
    check(!before.notices.some((n) => /audio/.test(n.iface)) && !/sound/i.test(before.notice),
      `${tag} nothing says sound works only in Krate on a computer`, { notices: before.notices, notice: before.notice });
    check(before.audio.streams === 1 && before.audio.sounds >= 1 && before.audio.state === 'none' && before.stats[S.quanta] === 0 && before.stats[S.played] === 0,
      `${tag} the app opened its stream and loaded its sounds; nothing plays before a tap`, { audio: before.audio, stats: named(before.stats) });
    if (app.short === 'check') {
      // A speaker's pace with nobody listening: ~48,000 frames a second
      // written and drained, the queue held at the app's tenth of a second.
      const secs = (before.at - first.at) / 1000;
      const wrote = (before.streams[0][0] - first.streams[0][0]) / secs;
      const played = (before.streams[0][1] - first.streams[0][1]) / secs;
      const queued = before.streams[0][0] - before.streams[0][1];
      check(wrote > 40000 && wrote < 56000 && played > 40000 && played < 56000 && queued <= 6000,
        `${tag} before the tap the stream drains at a speaker's pace (${Math.round(played)} frames/s) and the app keeps writing`, { wrote, played, queued });
    }
    if (shots) await page.screenshot({ path: join(shots, `audio-proof-${app.short}-${label}-before-tap.png`) });

    // (b) one click, as a person starts the game.
    const box = await page.locator('#screen').boundingBox();
    const press = () => (ua
      ? page.tap('#screen', { position: { x: box.width / 2, y: box.height / 2 } })
      : page.mouse.click(box.x + box.width / 2, box.y + box.height / 2));
    await press();
    await page.waitForFunction(() => window.__krate.audio.state === 'running' && window.__krate.audio.worklet, null, { timeout: 10000 }).catch(() => {});
    await page.waitForTimeout(500);
    let level = 0;
    const listen = async () => { level = Math.max(level, await page.evaluate(() => window.__krateAudio.level() || 0)); };
    if (app.short === 'jungle') {
      // Play: fire and jump for a while, over the music.
      await page.locator('#screen').focus();
      for (let i = 0; i < 12; i++) {
        await page.keyboard.down('z');
        await page.waitForTimeout(60);
        await listen();
        await page.keyboard.up('z');
        if (i % 4 === 0) await page.keyboard.press('x');
        await page.waitForTimeout(60);
        await listen();
      }
    } else {
      // The tone alone, then presses: each plays the click twice at once.
      for (let i = 0; i < 5; i++) { await listen(); await page.waitForTimeout(50); }
      for (let i = 0; i < 3; i++) { await press(); await page.waitForTimeout(40); await listen(); await page.waitForTimeout(150); }
    }
    const after = await page.evaluate(() => ({
      audio: window.__krate.audio,
      stats: window.__krateAudio.stats(),
      frames: window.__krate.frames,
      trap: window.__krate.trap,
      notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText,
    }));
    const st = named(after.stats);
    check(after.audio.state === 'running' && after.audio.worklet, `${tag} after the first ${ua ? 'tap' : 'click'} the AudioContext is running with its worklet`, after.audio);
    check(st.quanta > 100 && st.loud > 20 && st.peak > 50, `${tag} the audio thread renders the app's sound (not silence)`, st);
    check(level > 0.01, `${tag} sound reaches the graph's output (AnalyserNode RMS ${level.toFixed(4)})`, level);
    // (c) overlap. Jungle Assault's overlaps depend on its own timing (a
    // repeated note 13 ms over the next), so they are reported; Sound Check's
    // two plays per press must overlap.
    if (app.short === 'check') {
      check(st.played >= 6 && st.maxCopies >= 2, `${tag} play-sound twice sounds twice at once (most copies of one sound: ${st.maxCopies})`, st);
    } else {
      check(st.played > 10 && st.maxVoices >= 2, `${tag} effects mix over the music (most sounds at once: ${st.maxVoices}, most copies of one: ${st.maxCopies})`, st);
    }
    check(st.peak <= 4000, `${tag} the mix stays in bounds before the clamp`, st);
    // (d) still running, nothing wrong.
    check(after.frames > before.frames + 30 && !after.trap, `${tag} the app keeps drawing with sound on`, { before: before.frames, after: after.frames, trap: after.trap });
    check(!/sound/i.test(after.notice), `${tag} no notice about sound after playing`, after.notice);
    check(errors.length === 0, `${tag} no console errors`, errors);
    console.log(`     ${tag} stats ${JSON.stringify(st)} level ${level.toFixed(4)}`);
    if (shots) await page.screenshot({ path: join(shots, `audio-proof-${app.short}-${label}-playing.png`) });
    await context.close();
  }
}

await browser.close();
server.close();
console.log(failed ? `${failed} failed` : 'all passed');
process.exit(failed ? 1 : 0);
