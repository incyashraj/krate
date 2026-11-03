// The device's senses in the tab (IC-974): krate:ui/notify, ui/location,
// ui/share, speech/synthesis, camera/capture and audio/capture, opened the
// way a person opens a shared link -- Chromium on a computer and as a
// phone, WebKit as an iPhone -- against the hub Worker itself, running here
// in Node over the worker tests' stand-ins, and the Player in
// cloud/worker/player-dist (scripts/build-web-player.sh).
//
//   node --experimental-wasm-modules senses-proof.mjs [shot-dir]
//
// The app is Senses Check (fixtures/senses-check), packed twice:
//   (cd fixtures/senses-check && cargo component build --release)
//   W=fixtures/senses-check/target/wasm32-wasip1/release/krate_senses_check.wasm
//   node pack.mjs fixtures/senses-check/manifest.toml $W ../../../evidence/web-player/senses-check.krate
//   node pack.mjs fixtures/senses-check/manifest-ungranted.toml $W ../../../evidence/web-player/senses-check-ungranted.krate
//
// Each engine opens it three times:
//   granted  the browser allows location, camera and microphone (its fake
//            camera and microphone), and says yes to notifications;
//   refused  the browser says no to all four;
//   bare     the app without the capabilities.
// It checks, by what the app printed and what the browser was asked:
//   (a) the consent card does not send any of them to a computer;
//   (b) what the desktop refuses before a device is touched is refused in
//       the desktop's words;
//   (c) granted: the location the browser gives, after a first finding; a
//       line said and stopped; a camera frame of the size it says, opaque
//       and lit, none before start or after stop, the stream gone after
//       close; two seconds of sound converted to 16 kHz mono 16-bit; every
//       camera and microphone track ended once the app stopped them;
//   (d) a notification asked for with no tap behind it is one tap on the
//       sheet away, asked inside that tap, and shown under the app's name;
//       one from a tap needs no sheet;
//   (e) share: where the engine has Web Share, its share sheet opens (from
//       the sheet, and from a tap) and says sent; where it has none,
//       unavailable in words;
//   (f) refused: location not-allowed, camera system-denied, the
//       microphone "not allowed", a notification denied, a cancelled
//       share false -- the desktop's refusals;
//   (g) bare: every sense refused with the desktop's refusal, and the
//       browser asked nothing at all;
//   (h) no console errors and no notice that anything needs a computer.
//
// Each check that needs an ability -- speech voices, Web Share, a camera
// that sends frames -- asks the engine in the page whether it has it
// (engine-abilities.mjs). Where it has not (Linux builds: Chromium has no
// voices, WebKit no Web Share and a fake camera with no frames), the app's
// honest refusal is checked instead, a SKIP line names the engine and the
// ability, and the last line lists what was not proven where.
//
// Headless browsers have no notification service at all (Chromium answers
// "denied" whatever it is granted, WebKit "default"), so the browser's
// notification answer is a stand-in here (`fakeNotify`): the Player's
// service worker registration is real, its showNotification and the
// permission prompt are the stand-in's. PLAYWRIGHT_MODULE and CHROMIUM may
// point at a playwright and a browser already on the machine.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, worker } from '../../../cloud/worker/test/org-tools.mjs';
import { consoleError, unproven, voiceProbe, shareProbe, cameraProbe } from './engine-abilities.mjs';

const engines = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const shots = process.argv[2] || null;
const port = 8938;
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
const granted = await publish('senses-check.krate');
const ungranted = await publish('senses-check-ungranted.krate');

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

// In every page: what the browser was asked (so a refusal can be shown to
// have asked nothing), and the stand-in for its notification service.
function watch(answer) {
  const asked = (window.__asked = { media: [], geo: 0, share: 0, streams: [] });
  if (typeof MediaDevices !== 'undefined' && MediaDevices.prototype.getUserMedia) {
    const real = MediaDevices.prototype.getUserMedia;
    MediaDevices.prototype.getUserMedia = function getUserMedia(c) {
      asked.media.push(Object.keys(c || {}).filter((k) => c[k]).join('+'));
      return real.call(this, c).then((s) => { asked.streams.push(s); return s; });
    };
  }
  if (navigator.geolocation) {
    for (const f of ['watchPosition', 'getCurrentPosition']) {
      const real = navigator.geolocation[f].bind(navigator.geolocation);
      navigator.geolocation[f] = (...a) => { asked.geo += 1; return real(...a); };
    }
  }
  if (navigator.share) {
    const real = navigator.share.bind(navigator);
    navigator.share = (d) => { asked.share += 1; return real(d); };
  }
  const fake = (window.__fakeNotify = { state: 'default', answer, asked: [], shown: [] });
  if (typeof Notification !== 'undefined') {
    Object.defineProperty(Notification, 'permission', { configurable: true, get: () => fake.state });
    Notification.requestPermission = () => {
      fake.asked.push({ inTap: Boolean(navigator.userActivation && navigator.userActivation.isActive) });
      if (fake.state === 'default') fake.state = fake.answer;
      return Promise.resolve(fake.state);
    };
  }
  if (typeof ServiceWorkerRegistration !== 'undefined') {
    ServiceWorkerRegistration.prototype.showNotification = function showNotification(title, opts) {
      if (fake.state !== 'granted') return Promise.reject(new TypeError('No notification permission has been granted for this origin.'));
      fake.shown.push({ title, body: opts && opts.body, scope: this.scope, active: Boolean(this.active) });
      return Promise.resolve();
    };
  }
}

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
const ORIGIN = base;
const launched = {};
const measured = [];
const not = unproven();
const MIC_REFUSED = 'Err(Platform("the microphone was not allowed: the browser said no. Allow it in the browser\'s settings for this site, then open the app again"))';
const NO_SHARE = 'Err(Unavailable("this browser has no share sheet"))';
const NO_VOICES = 'Unsupported("this browser has no speech voices Krate can use")';
const SHARE = 'share sheet';

for (const [engine, label, viewport, ua] of RUNS) {
  const touch = Boolean(ua);
  // A camera and a microphone that exist. Headless Chromium has no prompt
  // of its own for them (without one it answers NotSupportedError), so its
  // fake prompt answers: yes in one browser, no in the other. WebKit's
  // answer is the permission the context was given.
  const launch = (answer) => engines[engine].launch({
    headless: true,
    args: engine === 'chromium' ? ['--use-fake-device-for-media-stream', `--use-fake-ui-for-media-stream${answer === 'no' ? '=deny' : ''}`] : [],
    ...(engine === 'chromium' && process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {}),
  });
  const yes = (launched[`${engine}-yes`] ||= await launch('yes'));
  const nay = engine === 'chromium' ? (launched[`${engine}-no`] ||= await launch('no')) : yes;
  // Whether this engine's camera sends frames at all, asked outside the
  // app: in a context of its own, granted the camera, on a blank page of
  // the Player's origin.
  const camProbe = await (async () => {
    const c = await yes.newContext({ viewport, userAgent: ua, isMobile: touch && engine === 'chromium', hasTouch: touch });
    await c.grantPermissions(['camera'], { origin: ORIGIN });
    await c.route(`${base}/engine-probe`, (r) => r.fulfill({ contentType: 'text/html', body: '<!doctype html><title>probe</title>' }));
    const p = await c.newPage();
    await p.goto(`${base}/engine-probe`);
    const got = await p.evaluate(cameraProbe);
    await c.close();
    return got;
  })();
  const open = async (kind, hash) => {
    const tag = `[${engine} ${label} ${kind}]`;
    const browser = kind === 'granted' ? yes : nay;
    const context = await browser.newContext({ viewport, userAgent: ua, isMobile: touch && engine === 'chromium', hasTouch: touch });
    if (kind === 'granted') {
      await context.grantPermissions(['geolocation', 'camera', 'microphone'], { origin: ORIGIN });
      await context.setGeolocation({ latitude: 51.5007, longitude: -0.1246, accuracy: 12 });
    }
    await context.addInitScript(watch, kind === 'granted' ? 'granted' : 'denied');
    // What every video in the page is doing, sampled: when the app is handed
    // no camera frame, this says whether the Player's video ever decoded.
    await context.addInitScript(() => {
      window.__vids = [];
      setInterval(() => {
        for (const v of document.querySelectorAll('video')) {
          const t = v.srcObject && v.srcObject.getVideoTracks ? v.srcObject.getVideoTracks()[0] : null;
          window.__vids.push({ cls: v.className, ready: v.readyState, w: v.videoWidth, h: v.videoHeight, time: Math.round(v.currentTime * 100) / 100, paused: v.paused, track: t ? `${t.readyState} muted=${t.muted} enabled=${t.enabled}` : 'none' });
        }
        if (window.__vids.length > 400) window.__vids.splice(0, 200);
      }, 500);
    });
    const page = await context.newPage();
    const errors = [];
    page.on('console', (m) => { if (consoleError(m)) errors.push(`${m.text()} @${JSON.stringify(m.location())}`); });
    page.on('pageerror', (e) => errors.push(String(e)));
    const press = (sel) => (touch ? page.tap(sel) : page.click(sel));
    const lines = () => page.evaluate(() => window.__krate.out.split('\n').filter(Boolean));
    const line = async (prefix, timeout = 20000) => {
      await page.waitForFunction((p) => window.__krate.out.split('\n').some((l) => l.startsWith(p)) || window.__krate.trap, prefix, { timeout }).catch(() => {});
      const l = (await lines()).find((x) => x.startsWith(prefix));
      // Rust's Debug names the error type ("NotifyError::Denied"): the case
      // is what is checked.
      return l ? l.slice(prefix.length).replace(/\b[A-Z]\w*Error::/g, '') : `(none) trap=${await page.evaluate(() => window.__krate.trap)}`;
    };
    const sheetUp = (timeout = 12000) => page.waitForFunction(() => !document.getElementById('askSheet').hidden, null, { timeout }).then(() => true).catch(() => false);
    const sheetWords = () => page.evaluate(() => ({ title: document.getElementById('askTitle').textContent, body: document.getElementById('askBody').textContent, yes: document.getElementById('askYes').textContent, no: document.getElementById('askNo').hidden ? '' : document.getElementById('askNo').textContent }));
    await page.goto(`${base}/run/${hash}?probe=1`);
    await page.waitForFunction(() => window.__krate && (window.__krate.state === 'consent' || window.__krate.state === 'app'), null, { timeout: 30000 }).catch(() => {});
    const card = await page.evaluate(() => ({ state: window.__krate.state, text: document.body.innerText }));
    return { tag, context, page, errors, press, lines, line, sheetUp, sheetWords, card };
  };
  const finish = async (o) => {
    const r = await o.page.evaluate(() => ({ trap: window.__krate.trap, notices: window.__krate.notices, notice: document.getElementById('notice').hidden ? '' : document.getElementById('notice').innerText }));
    const seen = o.errors.filter((e) => !(shots && /style/i.test(e)));
    check(!r.trap && !r.notices.length && !/computer/i.test(r.notice), `${o.tag} nothing says a sense needs a computer`, r);
    check(seen.length === 0, `${o.tag} no console errors`, seen);
    if (shots) await o.page.screenshot({ path: join(shots, `senses-proof-${engine}-${label}-${o.tag.split(' ')[2].replace(']', '')}.png`) });
    await o.context.close();
  };

  // ------------------------------------------------------------ granted
  {
    const o = await open('granted', granted);
    const { tag, line, press, sheetUp, sheetWords, page } = o;
    // (a)
    check(o.card.state === 'consent' && /notifications/i.test(o.card.text) && /where you are/i.test(o.card.text) && /camera/i.test(o.card.text) && /microphone/i.test(o.card.text) && !/computer/i.test(o.card.text),
      `${tag} the card asks for notifications, location, camera and microphone, and sends none to a computer`, o.card.text.slice(0, 600));
    await press('#consentAllow');
    // (b)
    const words = {
      'notify-empty: ': 'Err(InvalidContent("a notification needs a title"))',
      'notify-long: ': 'Err(InvalidContent("the notification text was too long"))',
      'notify-control: ': 'Err(InvalidContent("the notification text contained control characters"))',
      'camera-zero: ': 'Err(Unsupported("width and height must both be greater than zero"))',
      'mic-rate: ': 'Err(Unsupported("sample rate must be between 8000 and 192000 Hz"))',
      'say-empty: ': 'Err(InvalidRequest("there is no text to say"))',
      'say-rate: ': 'Err(InvalidRequest("rate 3 is outside 0.5 to 2"))',
      'share-nothing: ': 'Err(Invalid("nothing to share"))',
      'share-path: ': 'Err(Invalid("a file name must be a plain name, not \\"a/b.txt\\""))',
      'location-available: ': 'true',
    };
    const said = {};
    for (const p of Object.keys(words)) said[p] = await line(p);
    check(Object.entries(words).every(([p, w]) => said[p] === w), `${tag} what the desktop refuses before a device is touched is refused in its words`, said);
    // (c) location
    const first = await line('location-first: ');
    const fix = await line('location: ');
    check(first === 'Finding' && /^51\.5007,-0\.1246 accuracy=12 age-ms<60000=true$/.test(fix), `${tag} the location is the browser's, after a first finding`, { first, fix });
    // (c) speech
    const speech = {};
    for (const p of ['voices: ', 'say-voice: ', 'say: ', 'speaking: ', 'stop: ', 'speaking-after-stop: ']) speech[p] = await line(p);
    const spoken = await page.evaluate(() => window.__krate.senses);
    const SPOKEN = 'a line is said aloud (the browser started it), and stopped';
    if (await page.evaluate(voiceProbe)) {
      check(speech['voices: '] === 'some=true' && speech['say-voice: '] === 'Err(InvalidRequest("this browser has no voice no-such-voice"))'
        && speech['say: '] === 'Ok(())' && speech['speaking: '] === 'Ok(true)' && speech['stop: '] === 'Ok(())' && speech['speaking-after-stop: '] === 'Ok(false)'
        && spoken.spoken.includes('Hello from Senses Check.') && !spoken.speechErrors.length,
      `${tag} ${SPOKEN}`, { speech, spoken: spoken.spoken, errors: spoken.speechErrors, events: spoken.speechEvents, said: spoken.said });
    } else {
      not.skip(tag, SPOKEN, engine, 'speech voices');
      // The honest answer: no voices, in words; nothing claims to speak.
      // (say itself answers Ok today and the browser's synthesis fails;
      // a say that refused with the same words would be as honest.)
      check(speech['voices: '] === NO_VOICES && speech['say-voice: '] === 'Err(InvalidRequest("this browser has no voice no-such-voice"))'
        && (speech['say: '] === 'Ok(())' || speech['say: '] === `Err(${NO_VOICES})`) && speech['speaking: '] === 'Ok(false)'
        && speech['stop: '] === 'Ok(())' && speech['speaking-after-stop: '] === 'Ok(false)' && !spoken.spoken.length,
      `${tag} with no voices, the app hears "no speech voices" in words, and nothing claims to speak`, { speech, spoken: spoken.spoken, events: spoken.speechEvents });
    }
    // (c) camera
    const cam = {};
    for (const p of ['camera-devices: ', 'camera-info-before: ', 'camera-read-before-start: ', 'camera: ', 'camera-info: ', 'camera-stop: ', 'camera-read-after-stop: ', 'camera-close: ', 'camera-read-after-close: ']) cam[p] = await line(p, 30000);
    check(Number(cam['camera-devices: ']) >= 1 && cam['camera-info-before: '] === '320x240' && cam['camera-read-before-start: '] === 'Ok(false)',
      `${tag} the camera is there, says the size asked for before a frame, and gives nothing before start`, cam);
    const FRAME = 'a camera frame arrives, of the size it says, opaque and lit';
    if (camProbe.frame) {
      const framed = cam['camera: '] === 'frame 320x240 fits=true opaque=true lit=true' && cam['camera-info: '] === '320x240';
      // No frame where the engine's own camera sends them: say why, in the
      // same run -- the Player's video as it was, and the probe again in
      // the app's page, plain and with the Player's hidden style.
      const why = framed ? {} : {
        videos: await page.evaluate(() => (window.__vids || []).filter((x) => x.cls === 'pick').slice(-4)),
        probeInPage: await page.evaluate(cameraProbe).catch((e) => String(e)),
        probeInPageHidden: await page.evaluate(cameraProbe, 'pick').catch((e) => String(e)),
      };
      check(framed, `${tag} ${FRAME}`, { ...cam, ...why });
    } else {
      not.skip(tag, FRAME, engine, `frames from its fake camera (${camProbe.why})`);
      // The app waited its fifteen seconds and was handed nothing: no
      // made-up frame, no error.
      check(cam['camera: '] === 'no frame in 15 s' && cam['camera-info: '].startsWith('(none)'),
        `${tag} with a camera that sends no frame, the app is handed none and no error`, cam);
    }
    check(cam['camera-stop: '] === 'Ok(())' && cam['camera-read-after-stop: '] === 'Ok(false)' && cam['camera-close: '] === 'Ok(())' && cam['camera-read-after-close: '] === 'Err(InvalidStream)',
      `${tag} after stop nothing is handed out, and after close the stream is gone`, cam);
    // (c) microphone
    const mic = {};
    for (const p of ['mic-before-start: ', 'mic: ', 'mic-stop: ', 'mic-after-stop: ', 'mic-unknown: ']) mic[p] = await line(p, 30000);
    const heard = /^bytes=(\d+) peak=(\d+)$/.exec(mic['mic: ']);
    // Two seconds at 16 kHz mono 16-bit is 64000 bytes; the browser takes
    // a moment to start, so at least a quarter of it.
    check(mic['mic-before-start: '] === 'Ok(0)' && heard && Number(heard[1]) >= 16000 && Number(heard[1]) <= 72000 && Number(heard[1]) % 2 === 0,
      `${tag} two seconds heard, converted to 16 kHz mono 16-bit`, mic);
    check(heard && Number(heard[2]) > 0, `${tag} what was heard is not silence`, mic);
    check(mic['mic-stop: '] === 'Ok(())' && mic['mic-after-stop: '] === 'Ok(0)' && mic['mic-unknown: '] === 'Err(InvalidStream)', `${tag} after stop nothing is heard; an unknown stream is invalid`, mic);
    const tracks = await page.evaluate(() => ({ asked: window.__asked.media, live: window.__asked.streams.flatMap((s) => s.getTracks()).filter((t) => t.readyState !== 'ended').map((t) => t.kind), videos: document.querySelectorAll('video').length }));
    check(tracks.asked.includes('video') && tracks.asked.includes('audio') && !tracks.live.length && tracks.videos === 0,
      `${tag} the camera and the microphone are off once the app stopped them`, tracks);
    // (d) a notification with no tap behind it
    const up = await sheetUp(20000);
    const nwords = up ? await sheetWords() : null;
    if (up && shots) await page.screenshot({ path: join(shots, `senses-proof-${engine}-${label}-notify.png`) });
    if (up) await press('#askYes');
    const nstart = await line('notify-start: ');
    const fake = await page.evaluate(() => window.__fakeNotify);
    check(up && nwords.title === 'Senses Check wants to send you notifications' && nwords.yes === 'Allow notifications' && nwords.no === 'Not now',
      `${tag} with no tap behind it, asking about notifications is one tap on a sheet away`, nwords);
    check(nstart === 'Ok(())' && fake.asked.length === 1 && fake.asked[0].inTap && fake.shown.length === 1
      && fake.shown[0].title === 'Senses Check' && fake.shown[0].body === 'Timer done\nTea is ready' && /\/player\/notify\/$/.test(fake.shown[0].scope),
    `${tag} the browser is asked inside that tap, and the notification is shown under the app's name through the Player's service worker`, { nstart, fake });
    // (e) share with no tap behind it
    let sstart;
    let swords = null;
    const canShare = await page.evaluate(shareProbe);
    if (canShare) {
      const sup = await sheetUp();
      swords = sup ? await sheetWords() : null;
      if (sup) await press('#askYes');
      sstart = await line('share-start: ');
      check(sup && swords.title === 'Senses Check wants to share' && swords.yes === 'Share' && sstart === 'Ok(true)',
        `${tag} with no tap behind it, the share sheet is one tap on a sheet away, and says sent`, { swords, sstart });
    } else {
      not.skip(tag, 'with no tap behind it, the share sheet is one tap on a sheet away, and says sent', engine, SHARE);
      sstart = await line('share-start: ');
      check(sstart === NO_SHARE, `${tag} where the browser has no share sheet, unavailable in words`, sstart);
    }
    await line('ready');
    // (d) from a tap
    const sheetsBefore = (await page.evaluate(() => window.__krate.senses.asked.length));
    await press('#screen');
    const ntap = await line('notify-tap: ');
    const after = await page.evaluate(() => ({ fake: window.__fakeNotify, asked: window.__krate.senses.asked.length, sheet: !document.getElementById('askSheet').hidden }));
    check(ntap === 'Ok(())' && after.fake.shown.length === 2 && after.asked === sheetsBefore && !after.sheet, `${tag} once allowed, a notification from a tap is shown with no sheet`, { ntap, after });
    // (e) from a tap
    await press('#screen');
    const stap = await line('share-tap: ');
    const shares = await page.evaluate(() => window.__krate.senses.shares);
    if (canShare) {
      const last = shares[shares.length - 1] || {};
      check(stap === 'Ok(true)' && (last.how === 'release' || last.how === 'direct') && last.text === 'From a tap',
        `${tag} from a tap the share sheet opens at once, and says sent`, { stap, shares });
    } else {
      not.skip(tag, 'from a tap the share sheet opens at once, and says sent', engine, SHARE);
      check(stap === NO_SHARE, `${tag} from a tap too, unavailable in words`, stap);
    }
    measured.push({ run: `[${engine} ${label}]`, mic: mic['mic: '], camera: cam['camera: '], share: shares.map((s) => s.how).join(',') || 'none' });
    await finish(o);
  }

  // ------------------------------------------------------------ refused
  {
    const o = await open('refused', granted);
    const { tag, line, press, sheetUp, sheetWords, page } = o;
    await press('#consentAllow');
    const got = {};
    for (const p of ['location-first: ', 'location: ', 'camera: ', 'mic: ']) got[p] = await line(p, 30000);
    // (f)
    check(got['location: '] === 'NotAllowed', `${tag} the browser's no to the location is not-allowed`, got);
    check(got['camera: '] === 'SystemDenied', `${tag} the browser's no to the camera is system-denied`, got);
    check(`Err(${got['mic: ']})` === MIC_REFUSED, `${tag} the browser's no to the microphone is "not allowed", in words`, got);
    const up = await sheetUp(20000);
    if (up) await press('#askYes');
    const nstart = await line('notify-start: ');
    const fake = await page.evaluate(() => window.__fakeNotify);
    check(up && nstart === 'Err(Denied)' && fake.asked.length === 1 && !fake.shown.length, `${tag} the browser's no to notifications is denied`, { up, nstart, fake });
    if (await page.evaluate(shareProbe)) {
      const sup = await sheetUp();
      const w = sup ? await sheetWords() : null;
      if (sup) await press('#askNo');
      const sstart = await line('share-start: ');
      const asked = await page.evaluate(() => window.__asked.share);
      check(sup && w.no === 'Cancel' && sstart === 'Ok(false)' && asked === 0, `${tag} a share the person cancels is false, and the browser's sheet never opens`, { w, sstart, asked });
    } else {
      not.skip(tag, 'a share the person cancels is false, and the browser\'s sheet never opens', engine, SHARE);
      check((await line('share-start: ')) === NO_SHARE, `${tag} where the browser has no share sheet, unavailable`, await o.lines());
    }
    await line('ready');
    await finish(o);
  }

  // --------------------------------------------------------------- bare
  {
    const o = await open('bare', ungranted);
    const { tag, line, page } = o;
    if (o.card.state === 'consent') await o.press('#consentAllow');
    const got = {};
    for (const p of ['notify-empty: ', 'location: ', 'camera-devices: ', 'camera-open: ', 'mic-open: ', 'notify-start: ']) got[p] = await line(p);
    await line('ready');
    await page.waitForTimeout(500);
    const asked = await page.evaluate(() => ({ media: window.__asked.media, geo: window.__asked.geo, share: window.__asked.share, notify: window.__fakeNotify.asked, sheet: !document.getElementById('askSheet').hidden, dialogs: window.__krate.dialogs, senses: window.__krate.senses.asked }));
    // (g)
    check(got['notify-empty: '] === 'Err(Denied)' && got['notify-start: '] === 'Err(Denied)' && got['location: '] === 'Err(NotAllowed)'
      && got['camera-devices: '] === 'Err(PermissionDenied)' && got['camera-open: '] === 'Err(PermissionDenied)' && got['mic-open: '] === 'Err(PermissionDenied)',
    `${tag} without the capabilities, each sense answers the desktop's refusal`, got);
    check(!asked.media.length && !asked.geo && !asked.share && !asked.notify.length && !asked.sheet && !asked.senses.length,
      `${tag} without the capabilities, the browser is asked nothing and no sheet shows`, asked);
    await finish(o);
  }
}

for (const b of Object.values(launched)) await b.close();
server.close();
console.log('what each run heard, saw and shared:');
for (const m of measured) console.log(`  ${m.run} mic ${m.mic}; camera ${m.camera}; share ${m.share}`);
process.exit(not.end(failed));
