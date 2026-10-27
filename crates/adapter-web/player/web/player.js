// The Krate Player page: a .krate link opened in this tab (IC-956 Phase 1).
//
// What happens, in order, each step saying so in words:
//   1. fetch the app (the bundle and the glue the hub made for it at
//      publish) -- with this tab's sign-in for an organisation's app;
//   2. check it: the bytes are the ones the link names (SHA-256), and the
//      bundle passes crates/validator-wasm -- the same rules the hub's door
//      and `krate run` apply, signature included; every core module about
//      to run is one of the judged component's own;
//   3. ask the person, with the desktop's own words for each capability;
//   4. run it in a Web Worker, answering its async work (fetch, storage,
//      the clipboard) over the bridge and putting its frames on the canvas.
//
// Nothing here decides a rule; the Player module (crates/adapter-web) and the
// validator do. window.__krate carries the timings the proofs read.

import init, { consent } from './krate_web.js';
import { makeBridgeMemory, pageBridge } from './bridge.js';
import { makeRing, pageRing } from './ring.js';
import { TeamData } from './org-data.js';
// The browser as an organisation's device (window.KrateSigning): the same
// file the console uses on krate.tech, served here beside the Player.
import './org-device.js';

const $ = (id) => document.getElementById(id);
const report = (window.__krate = { marks: {}, frames: 0, latencies: [], paintLatencies: [], notices: [], out: '', state: 'loading' });
const mark = (name) => { report.marks[name] = performance.now(); };
const SITE = 'https://krate.tech';
const params = new URLSearchParams(location.search);
// /run/<hash> opens one release; /run/org/<slug>/<app-id> opens an
// organisation's app as it is now (the address a home screen keeps).
const RUN = location.pathname.match(/^\/run\/(?:([0-9a-f]{64})|org\/([a-z][a-z0-9-]{1,38})\/([a-z0-9][a-z0-9._-]{0,127}))\/?$/) || [];
let HASH = RUN[1];
const ORG_APP = RUN[2] ? { slug: RUN[2], app: RUN[3] } : null;
const FROM = ORG_APP ? `player:org/${ORG_APP.slug}/${ORG_APP.app}` : `player:${HASH}`;
// The post a link came from (`?via=reddit`), carried to the open's count.
const VIA = ((params.get('via') || '').toLowerCase().match(/^[a-z0-9][a-z0-9_-]{0,31}$/) || [''])[0];
// A phone or a tablet: nothing here may tell it to use "your computer".
const PHONE = /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent) || (navigator.maxTouchPoints > 1 && /Macintosh/.test(navigator.userAgent));
const SESSION_KEY = 'krate-player-session';
const NONCE_KEY = 'krate-player-nonce';

// ------------------------------------------------------------ look and words
const forced = params.get('theme');
if (forced === 'dark' || forced === 'light') document.documentElement.dataset.theme = forced;
const dark = () => (forced ? forced === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches);

const STATES = ['loading', 'consent', 'signin', 'refused', 'app'];
function show(state) {
  report.state = state;
  for (const s of STATES) $(s).hidden = s !== state;
}
function say(words) { $('loadingWords').textContent = words; }
function refuse(title, words, { act = 'krate', detail } = {}) {
  $('refusedTitle').textContent = title;
  $('refusedWords').textContent = words;
  $('refusedDetails').hidden = !detail;
  $('refusedDetail').textContent = detail || '';
  const a = $('refusedAct');
  a.onclick = null;
  if (act === 'krate' && PHONE) { a.textContent = 'Send it to a computer'; a.href = '#'; a.onclick = sendToComputer; }
  else if (act === 'krate') { a.textContent = 'Open in Krate'; a.href = openInKrate(); }
  else if (act === 'home') { a.textContent = 'Go to krate.tech'; a.href = SITE + '/'; }
  report.refused = { title, words, detail };
  show('refused');
}
/// A phone cannot keep the app as one: hand the link to the share sheet
/// (AirDrop, a note, a message to oneself), or copy it.
async function sendToComputer(e) {
  if (e) e.preventDefault();
  const button = e && e.currentTarget;
  const url = `${location.origin}/a/${HASH}`;
  if (navigator.share) {
    try { await navigator.share({ title: (report.app && report.app.name) || 'A Krate app', url }); return; }
    catch (err) { if (err && err.name === 'AbortError') return; }
  }
  try {
    await navigator.clipboard.writeText(url);
    if (button) { const was = button.textContent; button.textContent = 'Link copied'; setTimeout(() => { button.textContent = was; }, 1500); }
  } catch { prompt('Copy this link:', url); }
}
function openInKrate() {
  return HASH ? `${SITE}/open/?a=${HASH}${report.private ? '&private=1' : ''}` : `${SITE}/open/`;
}

// What a tab cannot do yet, in the words a person uses.
const WHAT = {
  'krate:gfx/canvas2d': 'drawing its own screen',
  'krate:gfx/canvas-images': 'drawing pictures',
  'krate:gfx/scene3d': '3D drawing',
  'krate:gfx/video': 'video',
  'krate:gpu/gpu': 'your graphics card',
  'krate:audio/playback': 'sound',
  'krate:audio/playback-queue': 'sound',
  'krate:audio/capture': 'your microphone',
  'krate:camera/capture': 'your camera',
  'krate:speech/transcription': 'speech',
  'krate:speech/synthesis': 'speaking aloud',
  'krate:fs/files': 'your files',
  'krate:fs/watch': 'your files',
  'krate:store/sql': 'its own database',
  'krate:store/org': "your team's shared data",
  'krate:store/secret': 'saved sign-in details',
  'krate:store/shared': 'data shared by invite code',
  'krate:store/group': 'data shared with other apps',
  'krate:net/ws': 'live connections',
  'krate:net/sockets': 'network connections',
  'krate:resources/assets': 'its own pictures and files',
  'krate:ui/dialog': 'dialogs and file pickers',
  'krate:ui/notify': 'notifications',
  'krate:ui/reminders': 'reminders',
  'krate:ui/print': 'printing',
  'krate:ui/menu': 'its own menus',
  'krate:ui/bluetooth': 'Bluetooth',
  'krate:ui/location': 'your location',
  'krate:ui/photos': 'your photos',
  'krate:ui/share': 'sharing',
  'krate:ui/sign-in': 'signing in to another service',
  'krate:ui/web-page': 'web pages inside it',
};
const whatOf = (iface) => WHAT[iface.replace(/@.*$/, '')] || 'something only Krate on a computer has';

function notice(words, act = true) {
  $('noticeWords').textContent = words;
  // On a phone the button would lead to a computer's install page.
  $('noticeAct').hidden = !act || PHONE;
  $('noticeAct').href = openInKrate();
  $('notice').hidden = false;
}
$('noticeClose').onclick = () => { $('notice').hidden = true; };

// ------------------------------------------------------- around the app
// One small line under the app, for as long as it is open: who made the
// thing it runs on and how to make one (the receiver is the next maker),
// and a way to report it that needs no account. Not on a team's private
// app: a coworker is not a lead, and the team answers for its own apps.
function beacon(event) {
  if (!HASH) return Promise.resolve(null);
  const me = session();
  return fetch('/tab', {
    method: 'POST',
    keepalive: true,
    headers: { 'content-type': 'application/json', ...(me && me.token ? { authorization: `Bearer ${me.token}` } : {}) },
    body: JSON.stringify({ event, app: HASH, via: VIA }),
  }).then((r) => (r.status === 200 ? r.json() : { ok: r.ok })).catch(() => ({ ok: false }));
}
/// The app is on the screen: count one open (K-1132), once.
function opened() {
  if (report.counted !== undefined) return;
  report.counted = null;
  beacon('open').then((b) => { report.counted = b; });
  keepLater();
}

// ------------------------------------------------------------ keep it
// A phone that used the app for a while is offered a way to keep it: one
// card, once per app, with the home screen steps for that phone. A home
// screen app also keeps its own data, which Safari otherwise deletes after
// seven days without a visit. `?keep_after=<ms>` shortens the wait for proofs.
let installPrompt = null;
addEventListener('beforeinstallprompt', (e) => { e.preventDefault(); installPrompt = e; });
const standalone = () => matchMedia('(display-mode: standalone)').matches || navigator.standalone === true;
const isIos = () => /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.maxTouchPoints > 1 && /Macintosh/.test(navigator.userAgent));
const KEEP_AFTER = (() => {
  const n = Number(new URLSearchParams(location.search).get('keep_after'));
  return Number.isFinite(n) && n >= 0 && n <= 120000 && new URLSearchParams(location.search).has('keep_after') ? n : 30000;
})();
const SHARE_GLYPH = '<svg viewBox="0 0 24 24" aria-label="Share"><path d="M12 15V3.5M8 7.5l4-4 4 4M6.5 11H5.5v9.5h13V11h-1" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';
function keepKey() { return `krate-kept:${(report.install && report.install.base) || HASH}`; }
function keepLater() {
  report.keep = { offered: false };
  if (!PHONE || standalone() || !report.install) return;
  try { if (localStorage.getItem(keepKey())) { report.keep.already = true; return; } } catch { /* private mode: offer it */ }
  setTimeout(showKeep, KEEP_AFTER);
}
function keepDone(how) {
  $('keep').hidden = true;
  report.keep.answer = how;
  try { localStorage.setItem(keepKey(), how); } catch { /* nothing to remember it in */ }
}
function showKeep() {
  if (document.visibilityState !== 'visible' || !$('reportSheet').hidden || !$('notice').hidden) {
    // Not now: the person is looking elsewhere or reading something else.
    setTimeout(showKeep, 15000);
    return;
  }
  const name = report.install.name || 'this app';
  $('keepTitle').textContent = `Keep ${name}`;
  const icon = document.querySelector('link[rel="apple-touch-icon"]');
  if (icon) $('keepIcon').src = icon.href; else $('keepIcon').hidden = true;
  const how = $('keepHow');
  if (installPrompt) {
    how.textContent = 'It opens like an app, without the browser around it.';
    $('keepAdd').hidden = false;
    report.keep.kind = 'prompt';
  } else if (isIos()) {
    how.innerHTML = `Tap ${SHARE_GLYPH} Share, then <b>Add to Home Screen</b>.`;
    report.keep.kind = 'ios';
  } else {
    how.innerHTML = 'Open your browser\'s menu, then <b>Add to Home screen</b>.';
    report.keep.kind = 'menu';
  }
  $('keep').hidden = false;
  report.keep.offered = true;
}
$('keepAdd').onclick = async () => {
  if (!installPrompt) return keepDone('added');
  installPrompt.prompt();
  const choice = await installPrompt.userChoice.catch(() => ({ outcome: 'dismissed' }));
  installPrompt = null;
  keepDone(choice.outcome === 'accepted' ? 'added' : 'not-now');
};
$('keepClose').onclick = () => keepDone('not-now');
addEventListener('appinstalled', () => { if (report.keep) keepDone('added'); });

/// Ask the browser to keep what the app stores. Granted quietly by Chrome
/// for a site the person uses, and by Safari for a home screen app.
function keepData() {
  if (!navigator.storage || !navigator.storage.persist) return;
  navigator.storage.persist().then((ok) => { report.persisted = ok; }).catch(() => { report.persisted = false; });
}
function madeWith(teamApp) {
  if (teamApp || !HASH) return;
  $('makeOwn').href = `${SITE}/app/?via=player&from=${HASH}`;
  $('makeOwn').onclick = () => { beacon('make'); };
  $('made').hidden = false;
  document.body.classList.add('has-made');
}
function reportSheet(open) {
  const sheet = $('reportSheet');
  sheet.hidden = !open;
  if (open) {
    $('reportSay').textContent = '';
    const first = sheet.querySelector('input[name="why"]');
    if (first) first.focus();
  } else {
    $('reportOpen').focus();
  }
}
$('reportOpen').onclick = () => reportSheet(true);
$('reportCancel').onclick = () => reportSheet(false);
$('reportSheet').addEventListener('click', (e) => { if (e.target === $('reportSheet')) reportSheet(false); });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !$('reportSheet').hidden) reportSheet(false); });
$('reportForm').onsubmit = async (e) => {
  e.preventDefault();
  const picked = $('reportForm').querySelector('input[name="why"]:checked');
  if (!picked) { $('reportSay').textContent = 'Choose what is wrong first.'; return; }
  const send = $('reportSend');
  send.disabled = true;
  $('reportSay').textContent = 'Sending...';
  const me = session();
  const res = await fetch(`/report/app/${HASH}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...(me && me.token ? { authorization: `Bearer ${me.token}` } : {}) },
    body: JSON.stringify({ why: picked.value, note: $('reportNote').value }),
  }).catch(() => null);
  send.disabled = false;
  if (res && res.ok) {
    report.reported = picked.value;
    $('reportTitle').textContent = 'Sent. Thank you.';
    $('reportLede').textContent = 'An app that breaks Krate’s rules is taken down, and anyone who opens it is told why.';
    $('reportWhy').hidden = true;
    $('reportNote').hidden = true;
    $('reportSay').textContent = '';
    send.hidden = true;
    $('reportCancel').textContent = 'Close';
    $('reportCancel').focus();
    $('reportOpen').textContent = 'Reported';
    $('reportOpen').disabled = true;
  } else {
    $('reportSay').textContent = res && res.status === 429
      ? 'Too many reports from here just now. Wait a minute and send it again.'
      : 'It did not send. Check the connection and try again.';
  }
};

// ------------------------------------------------------------------ sign-in
function session() {
  try { return JSON.parse(localStorage.getItem(SESSION_KEY) || 'null'); } catch { return null; }
}
// A finished sign-in comes back in the fragment (it never leaves the
// browser), with the nonce this tab made: taken only when it matches.
function takeSignIn() {
  if (!location.hash.includes('token=')) return;
  const got = new URLSearchParams(location.hash.slice(1));
  let expected = null;
  try { expected = sessionStorage.getItem(NONCE_KEY); sessionStorage.removeItem(NONCE_KEY); } catch {}
  history.replaceState(null, '', location.pathname + location.search);
  if (!expected || got.get('nonce') !== expected) return;
  try {
    localStorage.setItem(SESSION_KEY, JSON.stringify({ token: got.get('token'), login: got.get('login') || '', name: got.get('name') || '' }));
  } catch {}
}
function signIn(words) {
  const nonce = Array.from(crypto.getRandomValues(new Uint8Array(18)), (b) => b.toString(16).padStart(2, '0')).join('');
  try { sessionStorage.setItem(NONCE_KEY, nonce); } catch {}
  const q = (path) => `${path}?${new URLSearchParams({ from: FROM, nonce })}`;
  $('signinGithub').href = q('/login/start');
  $('signinGoogle').href = q('/login/google/start');
  if (words) $('signinWords').textContent = words;
  show('signin');
}

// ------------------------------------------------------------------ the app
const hex = (buf) => Array.from(new Uint8Array(buf), (b) => b.toString(16).padStart(2, '0')).join('');
const sha256 = async (bytes) => hex(await crypto.subtle.digest('SHA-256', bytes));

/// [u32 bundle length][bundle][u32 header length][header JSON][files...]
function unpack(buf) {
  const dv = new DataView(buf);
  const blen = dv.getUint32(0, true);
  const bundle = new Uint8Array(buf, 4, blen);
  let at = 4 + blen;
  const hlen = dv.getUint32(at, true);
  const meta = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, at + 4, hlen)));
  at += 4 + hlen;
  const files = new Map();
  for (const f of meta.files || []) {
    files.set(f.name, new Uint8Array(buf, at, f.size));
    at += f.size;
  }
  return { bundle, meta, files };
}

async function judge(validatorModule, bytes) {
  const v = (await WebAssembly.instantiate(validatorModule, {})).exports;
  const input = v.krate_alloc(bytes.length);
  new Uint8Array(v.memory.buffer, input, bytes.length).set(bytes);
  const out = v.krate_judge_for_tab(input, bytes.length);
  const len = new DataView(v.memory.buffer).getUint32(out, true);
  const text = new TextDecoder().decode(new Uint8Array(v.memory.buffer, out + 4, len));
  v.krate_free(out, 4 + len);
  v.krate_free(input, bytes.length);
  return JSON.parse(text);
}

/// The release an organisation's app opens now, with the person's session.
/// True when HASH is set; otherwise the page already says what to do.
async function resolveOrgApp() {
  const me = session();
  if (!me || !me.token) {
    report.private = true;
    signIn(`It belongs to ${ORG_APP.slug}. Sign in with the account your team invited.`);
    return false;
  }
  const res = await fetch(`/orgs/${ORG_APP.slug}/apps/${encodeURIComponent(ORG_APP.app)}/current`, {
    headers: { accept: 'application/json', authorization: `Bearer ${me.token}` },
    cache: 'no-store',
  }).catch(() => null);
  if (!res) {
    refuse('This app did not open', 'Krate could not be reached. Check the connection and try again.', { act: 'home' });
    return false;
  }
  const body = await res.json().catch(() => ({}));
  if (res.ok && /^[0-9a-f]{64}$/.test(body.digest || '')) {
    HASH = body.digest;
    report.private = true;
    return true;
  }
  if (res.status === 401) { signIn(); return false; }
  if (body.error === 'removed') refuse('You are no longer in this team', body.message || 'Ask the team to add you again.', { act: 'home' });
  else if (res.status === 403) refuse('This app was not shared with you', body.message || 'Ask whoever sent the link to invite you.', { act: 'home' });
  else refuse('This app is not on Krate', body.message || 'The link may be mistyped, or the app was removed.', { act: 'home' });
  return false;
}

/// Make the page addable to a home screen as the app: its name, its icon,
/// and for an organisation's app the address that always opens the version
/// the team released last.
async function installable(name, slug, appId) {
  const id = String(appId || '').toLowerCase();
  const base = slug && /^[a-z0-9][a-z0-9._-]{0,127}$/.test(id) ? `/run/org/${slug}/${id}` : `/run/${HASH}`;
  const link = (rel, href) => {
    let el = document.querySelector(`link[rel="${rel}"]`);
    if (!el) { el = document.createElement('link'); el.rel = rel; document.head.appendChild(el); }
    el.href = href;
  };
  link('manifest', `${base}/app.webmanifest?name=${encodeURIComponent(name)}`);
  let meta = document.querySelector('meta[name="apple-mobile-web-app-title"]');
  if (!meta) { meta = document.createElement('meta'); meta.name = 'apple-mobile-web-app-title'; document.head.appendChild(meta); }
  meta.content = name;
  report.install = { base, name };
  // An iPhone takes its home screen picture from here, not the manifest:
  // the same initial on the same colour the hub draws (web-player.js).
  try {
    await document.fonts.load('600 100px Geist');
    const c = document.createElement('canvas');
    c.width = c.height = 180;
    const g = c.getContext('2d');
    g.fillStyle = colourOf(base);
    g.fillRect(0, 0, 180, 180);
    g.fillStyle = '#fff';
    g.font = '600 100px Geist, -apple-system, BlinkMacSystemFont, sans-serif';
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    const m = /[\p{L}\p{N}]/u.exec(name);
    g.fillText(m ? m[0].toLocaleUpperCase() : 'K', 90, 96);
    link('apple-touch-icon', c.toDataURL('image/png'));
  } catch { /* the page still works; the home screen takes a picture of it */ }
}
const ICON_COLOURS = ['#3d6df0', '#22a35a', '#b7791f', '#7c4dff', '#e5484d', '#0f9d9a', '#d6409f', '#52525b'];
function colourOf(text) {
  let h = 5381;
  for (const c of text) h = ((h * 33) ^ c.charCodeAt(0)) >>> 0;
  return ICON_COLOURS[h % ICON_COLOURS.length];
}

async function main() {
  mark('start');
  takeSignIn();
  if (ORG_APP && self.crossOriginIsolated && !(await resolveOrgApp())) return;
  if (!HASH) {
    refuse('This is not an app link', 'A Krate app link ends in the app’s 64-character address.', { act: 'home' });
    return;
  }
  report.private = params.get('private') === '1';
  $('inKrate').href = openInKrate();
  if (!self.crossOriginIsolated || typeof SharedArrayBuffer !== 'function') {
    refuse(PHONE ? 'Open this in your browser' : 'This browser cannot open Krate apps in a tab',
      PHONE
        ? 'This in-app browser cannot run Krate apps. Open the link in Safari or Chrome.'
        : 'It does not allow the shared memory a Krate app needs here. The app opens in Krate on this computer.',
      { detail: `crossOriginIsolated=${self.crossOriginIsolated}` });
    return;
  }

  say('Opening the app...');
  const slow = setTimeout(() => say('Still opening... a slow connection'), 4000);
  const me = session();
  const headers = me && me.token ? { authorization: `Bearer ${me.token}` } : {};
  const [playerModule, validatorModule, res] = await Promise.all([
    WebAssembly.compileStreaming(fetch('/player/krate_web_bg.wasm')),
    WebAssembly.compileStreaming(fetch('/player/validator.wasm')),
    fetch(`/a/${HASH}/web`, { headers, cache: 'no-store' }),
  ]);
  mark('fetched');
  clearTimeout(slow);
  await init({ module_or_path: playerModule });

  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    const reason = body.error || '';
    if (reason === 'not-signed-in' || res.status === 401) {
      report.private = true;
      $('inKrate').href = openInKrate();
      signIn();
    } else if (reason === 'not-a-member' || reason === 'not-in-audience') {
      refuse('This app was not shared with you', body.message || 'Ask whoever sent the link to invite you.', { act: 'home' });
    } else if (reason === 'removed') {
      refuse('You are no longer in this team', body.message || 'Ask the team to add you again.', { act: 'home' });
    } else if (res.status === 410) {
      refuse('This version was withdrawn', body.message || 'Ask whoever sent it for the current link.', { act: 'home' });
    } else if (res.status === 451) {
      refuse('This app was taken down', body.reason ? `Krate took it down: ${body.reason}` : 'Krate took it down, and it does not open.', { act: 'home' });
    } else if (res.status === 404) {
      refuse('This app is not on Krate', 'The link may be mistyped, or the app was removed.', { act: 'home' });
    } else if (reason === 'no-tab') {
      refuse('This app opens in Krate, not in a tab yet', body.message || 'It opens in Krate on a computer.', { detail: body.detail });
    } else {
      refuse('This app did not open', body.message || `The hub answered ${res.status}. Try again in a moment.`, { act: 'home' });
    }
    return;
  }

  say('Checking it is the app that was sent...');
  const { bundle, meta, files } = unpack(await res.arrayBuffer());
  // The bytes are the ones the link names.
  const digest = await sha256(bundle);
  if (digest !== HASH) {
    refuse('This app was changed on the way', 'What arrived is not the app the link names, so it was not opened.',
      { act: 'home', detail: `expected ${HASH}\nreceived ${digest}` });
    return;
  }
  // The bundle passes the same rules the hub and `krate run` apply.
  const judged = await judge(validatorModule, bundle);
  mark('judged');
  if (!judged.ok) {
    refuse('This app did not pass Krate’s checks', 'It was not opened.', { act: 'home', detail: judged.problem });
    return;
  }
  const j = judged.judgement;
  const sig = j.signature.state;
  if (sig !== 'absent' && sig !== 'valid') {
    refuse('This app was changed after it was signed', 'It was not opened: a signed app that no longer matches its signature could be anyone’s.',
      { act: 'home', detail: `${sig}: ${j.signature.detail || ''}` });
    return;
  }
  // Every core module about to run is one of the judged component's own.
  const modules = [];
  for (const [name, bytes] of files) {
    if (!name.endsWith('.wasm')) continue;
    const d = await sha256(bytes);
    if (!judged.modules.includes(d)) {
      refuse('This app opens in Krate, not in a tab yet', 'Part of what a tab would run could not be matched to the checked app, so it was not run.',
        { detail: `${name} ${d} is not a module of the judged component` });
      return;
    }
    modules.push([name, bytes.slice()]);
  }
  const glue = files.get('app.js');
  if (!glue) {
    refuse('This app opens in Krate, not in a tab yet', 'The hub has nothing for a tab to run for it.');
    return;
  }

  const app = j.manifest.app;
  const name = app.name || 'This app';
  document.title = `${name} - Krate`;
  $('whoName').textContent = name;
  const release = sig === 'valid' && j.release;
  $('whoSig').textContent = release ? `Signed by ${release.namespace.split('/')[0]}` : 'Not signed';
  $('who').hidden = false;
  report.app = { id: app.id, name, world: app.world, signature: sig };
  madeWith(report.private || Boolean(sig === 'valid' && j.org_mark));
  installable(name, sig === 'valid' ? j.org_mark : null, app.id);

  // ---- consent: the desktop's words, the tab's reach
  const rows = JSON.parse(consent(JSON.stringify(j.manifest.capabilities || []))).rows || [];
  const blocked = rows.filter((r) => r.required && !r.in_tab);
  const granted = rows.filter((r) => r.in_tab).map((r) => r.cap);
  const storeKey = release ? `${app.id}@${release.authority.slice(0, 16)}` : app.id;
  if (rows.length) {
    $('consentTitle').textContent = `“${name}” wants to:`;
    const list = $('consentRows');
    list.textContent = '';
    for (const r of rows) {
      const li = document.createElement('li');
      if (!r.in_tab) li.className = 'away';
      li.innerHTML = '<i><svg viewBox="0 0 16 16" fill="none"><path d="M3.5 8.5l3 3 6-6.5" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg></i><span><b></b><em></em></span>';
      li.querySelector('b').textContent = r.plain;
      li.querySelector('em').textContent = r.in_tab ? r.rationale : `${r.rationale ? r.rationale + ' - ' : ''}works in Krate on a computer`;
      list.appendChild(li);
    }
    if (blocked.length) {
      $('consentNote').textContent = `${name} needs ${blocked.map((r) => r.plain).join(', ')}, which works in Krate on a computer, not in a tab yet.`;
      $('consentAllow').textContent = PHONE ? 'Send it to a computer' : 'Open in Krate';
      $('consentAllow').onclick = PHONE ? sendToComputer : () => { location.href = openInKrate(); };
    } else {
      $('consentNote').textContent = rows.some((r) => !r.in_tab)
        ? 'Greyed out: works in Krate on a computer. The app opens here without it.' : '';
      $('consentAllow').textContent = 'Allow and open';
    }
    $('consentCancel').onclick = () => refuse('Not opened', `${name} did not open. Nothing was allowed.`, { act: 'home' });
    show('consent');
    mark('consent');
    if (blocked.length) return;
    await new Promise((resolve) => { $('consentAllow').onclick = resolve; });
    mark('allowed');
  } else {
    mark('allowed');
  }

  say(`Starting ${name}...`);
  show('loading');
  // An organisation's app, signed and marked for it, opened by a signed-in
  // member: its team data, as on a computer (store.org).
  const org = { slug: (sig === 'valid' && j.org_mark) || null, appId: app.id, digest: HASH, session: me, validatorModule };
  await start({ meta, modules, glue: new TextDecoder().decode(glue), granted, storeKey, playerModule, world: app.world, name, org });
}

// ------------------------------------------------------------- team data
const SYNC_WORDS = {
  synced: 'Synced',
  syncing: 'Syncing',
  offline: 'Offline',
  'waiting-for-key': 'Waiting for the team key',
  'no-key': 'Team data not set up',
  refused: 'No team data',
};
const isSafari = () => /Safari\//.test(navigator.userAgent) && !/Chrome\/|Chromium\/|Edg\/|Android/.test(navigator.userAgent);

/// What the chip by the app's name says, and the line under it when it is
/// opened.
function showSync(state) {
  report.sync = state;
  const chip = $('sync');
  chip.hidden = false;
  chip.dataset.link = state.link;
  const n = state.pending;
  let words = SYNC_WORDS[state.link] || 'Syncing';
  if (state.link === 'offline' && n) words = `Offline, ${n} change${n === 1 ? '' : 's'} kept here`;
  $('syncWords').textContent = words;
  const org = state.org || 'your team';
  const say = {
    synced: `Your changes reach everyone in ${org}, and theirs show up here.`,
    syncing: n ? `Sending ${n} change${n === 1 ? '' : 's'} to ${org}.` : `Checking with ${org} for changes.`,
    offline: 'You can keep working. Changes are kept in this browser and go to your team when you are back online.',
    'waiting-for-key': `This browser is new to ${org}. A teammate's Krate gives it the team key by itself the next time it is open. Until then your changes are kept here.`,
    'no-key': `${org} has not set up its team key yet. An owner sets it up once in Krate. Until then your changes are kept here.`,
    refused: (state.refused && state.refused.message) || 'Your team did not give this app its data. Ask an owner.',
  }[state.link] || '';
  $('syncSay').textContent = say;
  // WebKit deletes what a site keeps after seven days of Safari use without
  // a visit; said where it matters, in the person's words.
  $('syncSafari').hidden = !(isSafari() && (n > 0 || state.link === 'offline' || state.link === 'waiting-for-key'));
}
$('sync').onclick = () => {
  const open = $('syncInfo').hidden;
  $('syncInfo').hidden = !open;
  $('sync').setAttribute('aria-expanded', String(open));
};

/// This tab as one of the organisation's devices, and its copy of the app's
/// team data. `mode`: on, not-org (no team data, as on a computer), or
/// unsupported (this browser cannot keep the team key).
async function openTeam({ slug, appId, digest, session: me, validatorModule }, windowed) {
  if (!slug || !me || !me.token) return { mode: 'not-org' };
  const S = window.KrateSigning;
  try {
    await crypto.subtle.generateKey({ name: 'X25519' }, false, ['deriveBits']);
    if (!window.indexedDB) throw new Error('no IndexedDB');
  } catch {
    return { mode: 'unsupported' };
  }
  const mod = await S.loadModule(validatorModule);
  const store = S.indexedStore('krate-org');
  const device = await S.openDevice({ store });
  // The organisation's own name, for the words the page shows before the
  // first sync says it.
  const named = await fetch(`/orgs/${slug}`, { headers: { accept: 'application/json', authorization: `Bearer ${me.token}` } })
    .then((r) => (r.ok ? r.json() : null)).catch(() => null);
  const orgName = (named && named.org && named.org.name) || slug;
  const team = new TeamData({ mod, signing: S, hub: location.origin, token: me.token, slug, appId, digest, me: me.login || me.name || '', store, device, windowed, orgName, onState: showSync });
  await team.open();
  team.start();
  // For the proofs: the tab's team data, outside the report they serialise.
  window.__krateTeam = team;
  return { mode: 'on', team };
}

// ------------------------------------------------------------- storage
const DB = 'krate-player';
function db() {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open(DB, 1);
    open.onupgradeneeded = () => open.result.createObjectStore('kv');
    open.onsuccess = () => resolve(open.result);
    open.onerror = () => reject(open.error);
  });
}
const tx = (d, mode, fn) => new Promise((resolve, reject) => {
  const t = d.transaction('kv', mode);
  const out = fn(t.objectStore('kv'));
  t.oncomplete = () => resolve(out && out.result);
  t.onerror = () => reject(t.error);
  t.onabort = () => reject(t.error || new Error('the browser stopped the write'));
});
const range = (store) => IDBKeyRange.bound([store, ''], [store, '￿']);
async function loadKv(d, store) {
  const keys = await tx(d, 'readonly', (s) => s.getAllKeys(range(store)));
  const values = await tx(d, 'readonly', (s) => s.getAll(range(store)));
  return keys.map((k, i) => [k[1], new Uint8Array(values[i])]);
}

// ------------------------------------------------------------- running
async function start({ meta, modules, glue, granted, storeKey, playerModule, world, name, org }) {
  const gui = /\/gui@/.test(world || '');
  const d = gui || granted.includes('store.kv') ? await db().catch(() => null) : null;
  if (d && granted.includes('store.kv')) keepData();
  const team = granted.includes('store.org') ? await openTeam(org, gui).catch((e) => {
    report.teamError = String((e && e.message) || e);
    return { mode: 'unsupported' };
  }) : { mode: 'off' };
  report.teamMode = team.mode;
  const teamCall = (fn) => async (args) => {
    if (!team.team) return { json: { err: { tag: team.mode === 'unsupported' ? 'unsupported' : 'not-an-org-app' } } };
    const out = await fn(team.team, args);
    return { json: out.err ? { err: out.err } : { ok: out.ok === undefined ? null : out.ok } };
  };
  if (team.team) {
    // Once more when the tab goes away, so what the person did last reaches
    // the team; what does not make it waits here for the next open.
    document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'hidden') team.team.kick(); });
  }
  const kv = d && granted.includes('store.kv') ? await loadKv(d, storeKey).catch(() => []) : [];

  const ringBuf = makeRing();
  const ring = pageRing(ringBuf);
  const bridgeMem = makeBridgeMemory();
  const worker = new Worker('/player/app-worker.js', { type: 'module' });
  const canvas = $('screen');
  const ctx2d = canvas.getContext('2d');
  const term = $('term');
  const windows = new Map();
  // Where each window's text fields are, for the phone's keyboard.
  const textRects = new Map();
  let current = null;
  const now = () => performance.timeOrigin + performance.now();

  // ---- async host work, answered over the bridge
  const fetches = new Map();
  let nextFetch = 1;
  const MAX = 1024 * 1024;
  const CORS = 'the server did not answer this tab. It may not allow requests from a browser tab (CORS), or be unreachable from here; the app reaches it from Krate on a computer.';
  const doFetch = async ({ method, url, headers, body, timeout }) => {
    if (location.protocol === 'https:' && /^http:/i.test(url)) {
      return { json: { err: { tag: 'connect-failure', val: 'a page on https cannot reach a plain http address; the app reaches it from Krate on a computer.' } } };
    }
    const abort = new AbortController();
    const timer = timeout ? setTimeout(() => abort.abort('timeout'), timeout) : null;
    try {
      const res = await fetch(url, {
        method,
        headers: (headers || []).map((h) => [h.name, h.value]),
        body: body && method !== 'GET' && method !== 'HEAD' ? body : undefined,
        // Redirects are not followed, as on desktop: a granted host must not
        // be able to send the app's request anywhere it likes. A tab cannot
        // even see where a redirect points, so the request stops here.
        redirect: 'manual',
        credentials: 'omit',
        cache: 'no-store',
        referrerPolicy: 'no-referrer',
        signal: abort.signal,
      });
      if (res.type === 'opaqueredirect') {
        return { json: { err: { tag: 'other', val: 'the server answered with a redirect, which a browser tab does not follow or show; the app can follow it from Krate on a computer.' } } };
      }
      const reader = res.body ? res.body.getReader() : null;
      const parts = [];
      let total = 0;
      while (reader) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.length;
        if (total > MAX) { abort.abort('too-large'); return { json: { err: { tag: 'body-too-large' } } }; }
        parts.push(value);
      }
      const out = new Uint8Array(total);
      let at = 0;
      for (const p of parts) { out.set(p, at); at += p.length; }
      return { json: { ok: { status: res.status, headers: [...res.headers].map(([n, v]) => ({ name: n, value: v })) } }, body: out };
    } catch (e) {
      if (abort.signal.aborted && abort.signal.reason === 'timeout') return { json: { err: { tag: 'timeout' } } };
      return { json: { err: { tag: 'connect-failure', val: CORS } } };
    } finally {
      if (timer) clearTimeout(timer);
    }
  };
  const kvKey = (key) => [storeKey, key];
  const kvWrite = async (fn) => {
    try { await tx(d, 'readwrite', fn); return { json: { ok: null } }; }
    catch (e) { return { json: { err: { tag: 'io', val: `the browser did not save it (${(e && e.message) || e})` } } }; }
  };
  const bridge = pageBridge(bridgeMem, {
    'http.fetch': doFetch,
    'http.begin': (args) => {
      const id = nextFetch++;
      const entry = { state: 'pending' };
      entry.done = doFetch(args).then((a) => { entry.answer = a; entry.state = 'done'; });
      fetches.set(String(id), entry);
      return { json: { ok: id } };
    },
    'http.poll': ({ handle }) => {
      const entry = fetches.get(handle);
      if (!entry) return { json: { state: 'unknown' } };
      if (entry.state === 'pending') return { json: { state: 'pending' } };
      fetches.delete(handle);
      return entry.answer;
    },
    'http.cancel': ({ handle }) => { fetches.delete(handle); return { json: { ok: null } }; },
    'kv.set': ({ key, value }) => (d ? kvWrite((s) => s.put(value, kvKey(key))) : { json: { err: { tag: 'io', val: 'this browser keeps no storage for this tab' } } }),
    'kv.delete': ({ key }) => (d ? kvWrite((s) => s.delete(kvKey(key))) : { json: { ok: null } }),
    'kv.clear': () => (d ? kvWrite((s) => s.delete(range(storeKey))) : { json: { ok: null } }),
    'clipboard.read': async () => {
      try { return { json: { ok: await navigator.clipboard.readText() } }; }
      catch { return { json: { err: { tag: 'platform', val: 'the browser did not let this tab read the clipboard' } } }; }
    },
    'clipboard.write': async ({ text }) => {
      try { await navigator.clipboard.writeText(text); return { json: { ok: null } }; }
      catch { return { json: { err: { tag: 'platform', val: 'the browser did not let this tab write to the clipboard' } } }; }
    },
    'org.put': teamCall((t, a) => t.put(a.collection, a.id, a.fields)),
    'org.delete': teamCall((t, a) => t.delete(a.collection, a.id)),
    'org.get': teamCall((t, a) => t.get(a.collection, a.id)),
    'org.records': teamCall((t, a) => t.records(a.collection, a.since)),
    'org.sync': teamCall((t) => t.sync()),
    'org.who': teamCall((t) => t.who()),
    'org.status': teamCall((t) => t.standing()),
    'open-url': ({ url }) => {
      const opened = window.open(url, '_blank', 'noopener,noreferrer');
      if (!opened) notice(`${name} wants to open ${url}. Your browser held it back.`, false);
      report.opened = (report.opened || []).concat(url);
      return { json: { ok: null } };
    },
  }, (op, e) => { report.bridgeErrors = (report.bridgeErrors || []).concat(`${op}: ${e}`); });

  // ---- frames
  let scratch = null;
  // The worker bumps a shared counter after each frame and the page wakes on
  // it (Atomics.waitAsync), so a frame reaches the canvas as soon as it is
  // painted; the 'frame' message that follows carries only what it was for.
  const signalBuf = new SharedArrayBuffer(8);
  const signal = new Int32Array(signalBuf);
  const blitted = new Map();
  const blit = (id) => {
    const w = windows.get(id);
    if (!w || !scratch) return now();
    // ImageData cannot wrap shared memory, so one copy out of it.
    scratch.set(new Uint8Array(w.frame));
    ctx2d.putImageData(new ImageData(scratch, w.pw, w.ph), 0, 0);
    report.frames += 1;
    if (report.marks.firstFrame === undefined) { report.marks.firstFrame = performance.now(); opened(); }
    return now();
  };
  let seen = 0;
  const listen = () => {
    if (typeof Atomics.waitAsync !== 'function') return;
    const r = Atomics.waitAsync(signal, 0, seen);
    (r.async ? r.value : Promise.resolve()).then(() => {
      const seq = Atomics.load(signal, 0);
      if (seq !== seen) {
        seen = seq;
        blitted.set(seq, blit(String(Atomics.load(signal, 1))));
      }
      listen();
    });
  };
  listen();
  worker.onmessage = ({ data }) => {
    switch (data.t) {
      case 'call':
      case 'more':
        bridge.onMessage(data);
        break;
      case 'window': {
        windows.set(data.id, data);
        current = data.id;
        canvas.width = data.pw;
        canvas.height = data.ph;
        canvas.style.width = `${data.lw}px`;
        canvas.style.height = `${data.lh}px`;
        scratch = new Uint8ClampedArray(data.pw * data.ph * 4);
        if (report.state !== 'app') { show('app'); $('frame').hidden = false; $('closeApp').hidden = false; canvas.focus(); }
        break;
      }
      case 'frame': {
        if (!windows.get(data.id)) break;
        const done = blitted.get(data.seq) ?? blit(data.id);
        blitted.delete(data.seq);
        if (data.inputAt !== undefined) {
          report.latencies.push(done - data.inputAt);
          // The worker's half: input to the frame painted in shared memory.
          (report.paintLatencies = report.paintLatencies || []).push(data.paintedAt - data.inputAt);
        }
        if (data.screen) report.screen = JSON.parse(data.screen);
        if (data.fields) textRects.set(data.id, data.fields);
        break;
      }
      case 'title': document.title = `${data.title} - Krate`; break;
      case 'out': {
        report.out += data.text;
        if (!gui || data.fd === 2) {
          if (term.hidden) { term.hidden = false; if (!gui) show('app'); }
          const span = document.createElement('span');
          if (data.fd === 2) span.className = 'err';
          span.textContent = data.text;
          term.appendChild(span);
          if (report.marks.firstOutput === undefined) { report.marks.firstOutput = performance.now(); opened(); }
        }
        break;
      }
      case 'notice': {
        report.notices.push({ iface: data.iface, fname: data.fname });
        notice(`${name} wanted to use ${whatOf(data.iface)}, which works in Krate on a computer, not in a tab yet.`);
        break;
      }
      case 'text-entry': if (data.on) $('ime').focus({ preventScroll: true }); else canvas.focus(); break;
      case 'marks': report.workerMarks = data.marks; break;
      case 'closed': windows.delete(data.id); break;
      case 'exit':
        report.exit = data.code;
        finish(data.code === 0 ? `${name} finished.` : `${name} stopped (exit ${data.code}).`);
        break;
      case 'trap':
        report.trap = data.message;
        finish(data.stopped ? data.message : `${name} stopped: ${data.message}`, !data.stopped);
        break;
      default: break;
    }
  };
  worker.onerror = (e) => { report.trap = `worker: ${e.message}`; finish(`${name} stopped: ${e.message}`); };
  function finish(words, crashed) {
    $('closeApp').hidden = true;
    if (report.state !== 'app') show('app');
    $('frame').hidden = !gui || windows.size === 0;
    const ended = $('ended');
    ended.textContent = words;
    ended.hidden = false;
    if (crashed) notice(words);
    worker.terminate();
  }

  // ---- input, in logical pixels, to the app's ring
  const mods = (e) => (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
  const at = (e) => performance.timeOrigin + e.timeStamp;
  const win = () => current;
  // A finger is not a mouse: the Player module maps it as a phone does (a
  // tap presses, a drag scrolls, a canvas follows the finger).
  const touchPhase = { pointerdown: 'start', pointermove: 'move', pointerup: 'end', pointercancel: 'cancel' };
  const touch = (e) => {
    ring.push({ type: 'touch', window: win(), phase: touchPhase[e.type], finger: e.pointerId, x: e.offsetX, y: e.offsetY, at: at(e) });
    // The keyboard comes up only inside the tap itself, so the page raises
    // it here when the finger lifts on a text field.
    if (e.type === 'pointerup') {
      const r = textRects.get(win()) || [];
      for (let i = 0; i + 3 < r.length; i += 4) {
        if (e.offsetX >= r[i] && e.offsetX < r[i] + r[i + 2] && e.offsetY >= r[i + 1] && e.offsetY < r[i + 1] + r[i + 3]) {
          resetIme();
          ime.focus({ preventScroll: true });
          break;
        }
      }
    }
    e.preventDefault();
  };
  canvas.addEventListener('pointerdown', (e) => {
    if (e.pointerType === 'touch') {
      canvas.setPointerCapture(e.pointerId);
      touch(e);
      return;
    }
    canvas.focus();
    canvas.setPointerCapture(e.pointerId);
    ring.push({ type: 'down', window: win(), x: e.offsetX, y: e.offsetY, button: e.button, mods: mods(e), at: at(e) });
    e.preventDefault();
  });
  canvas.addEventListener('pointerup', (e) => {
    if (e.pointerType === 'touch') return touch(e);
    ring.push({ type: 'up', window: win(), x: e.offsetX, y: e.offsetY, button: e.button, mods: mods(e), at: at(e) });
  });
  canvas.addEventListener('pointercancel', (e) => { if (e.pointerType === 'touch') touch(e); });
  canvas.addEventListener('pointermove', (e) => {
    if (e.pointerType === 'touch') return touch(e);
    ring.push({ type: 'move', window: win(), x: e.offsetX, y: e.offsetY });
  });
  canvas.addEventListener('pointerleave', (e) => { if (e.pointerType !== 'touch') ring.push({ type: 'leave', window: win() }); });
  canvas.addEventListener('contextmenu', (e) => { if (matchMedia('(pointer: coarse)').matches) e.preventDefault(); });
  canvas.addEventListener('wheel', (e) => {
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? canvas.clientHeight : 1;
    ring.push({ type: 'wheel', window: win(), x: e.offsetX, y: e.offsetY, dx: e.deltaX * unit, dy: e.deltaY * unit, mods: mods(e), at: at(e) });
    e.preventDefault();
  }, { passive: false });
  const key = (pressed) => (e) => {
    if (current === null) return;
    const typed = pressed && e.key.length === 1 && !e.ctrlKey && !e.metaKey ? e.key : undefined;
    ring.push({ type: 'key', window: win(), key: e.key, pressed, mods: mods(e), text: typed, at: at(e) });
    // Keys the app takes stay with the app; browser shortcuts (Cmd/Ctrl)
    // stay with the browser.
    if (!e.ctrlKey && !e.metaKey) e.preventDefault();
  };
  canvas.addEventListener('keydown', key(true));
  canvas.addEventListener('keyup', key(false));
  canvas.addEventListener('paste', (e) => {
    const text = e.clipboardData && e.clipboardData.getData('text/plain');
    if (text) ring.push({ type: 'text', window: win(), text, at: performance.timeOrigin + performance.now() });
    e.preventDefault();
  });
  // A phone's keyboard types into the hidden field; its text goes to the
  // app. One placeholder character stays in the field so Backspace always
  // has something to delete (an empty field sends no event for it), and a
  // word being composed (predictive text, accents, CJK) goes once it is
  // committed, not letter by letter.
  const ime = $('ime');
  const HOLD = ' ';
  function resetIme() {
    ime.value = HOLD;
    try { ime.setSelectionRange(1, 1); } catch {}
  }
  resetIme();
  let composing = false;
  const typed = (text, e) => { if (text) ring.push({ type: 'text', window: win(), text, at: at(e) }); };
  const press = (name, e) => {
    ring.push({ type: 'key', window: win(), key: name, pressed: true, mods: 0, at: at(e) });
    ring.push({ type: 'key', window: win(), key: name, pressed: false, mods: 0, at: at(e) });
  };
  ime.addEventListener('compositionstart', () => { composing = true; });
  ime.addEventListener('compositionend', (e) => {
    composing = false;
    typed(ime.value.replace(HOLD, '') || e.data || '', e);
    resetIme();
  });
  ime.addEventListener('input', (e) => {
    if (composing || e.isComposing) return;
    if (e.inputType === 'deleteContentBackward') press('Backspace', e);
    else if (e.inputType === 'insertLineBreak') press('Enter', e);
    else typed(ime.value.replace(HOLD, ''), e);
    resetIme();
  });
  // Keys a phone keyboard sends as keys: Enter, Tab, arrows, Escape.
  ime.addEventListener('keydown', (e) => {
    if (composing || !/^(Enter|Tab|Escape|Arrow\w+)$/.test(e.key)) return;
    ring.push({ type: 'key', window: win(), key: e.key, pressed: true, mods: mods(e), at: at(e) });
    e.preventDefault();
  });
  ime.addEventListener('keyup', (e) => {
    if (composing || !/^(Enter|Tab|Escape|Arrow\w+)$/.test(e.key)) return;
    ring.push({ type: 'key', window: win(), key: e.key, pressed: false, mods: mods(e), at: at(e) });
  });
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (!forced) ring.push({ type: 'theme', dark: dark() });
  });
  window.addEventListener('blur', () => ring.push({ type: 'blur' }));
  // The window takes the room the tab has, up to what the app asked for:
  // on a phone, above the keyboard when it is up (the visual viewport).
  const room = () => {
    const stage = $('stage');
    const s = stage.getBoundingClientRect();
    const cs = getComputedStyle(stage);
    const padX = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
    const padY = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
    const vv = window.visualViewport;
    const bottom = vv ? Math.min(s.bottom, vv.offsetTop + vv.height) : s.bottom;
    return { width: Math.max(1, Math.floor(s.width - padX)), height: Math.max(1, Math.floor(bottom - s.top - padY)) };
  };
  const resized = () => { if (current !== null) ring.push({ type: 'resize', window: win(), ...room() }); };
  new ResizeObserver(resized).observe($('stage'));
  if (window.visualViewport) window.visualViewport.addEventListener('resize', resized);
  $('closeApp').onclick = () => {
    if (current === null) { worker.terminate(); finish(`${name} closed.`); return; }
    ring.push({ type: 'close', window: win() });
    // An app that does not answer its close is ended, as Krate's own Close
    // ends one (IC-934).
    setTimeout(() => { if (report.exit === undefined && !report.trap) finish(`${name} closed.`); }, 2000);
  };

  report.marks.workerStarted = performance.now();
  worker.postMessage({
    t: 'start',
    playerModule,
    meta,
    modules,
    glue,
    granted,
    kv,
    storeKey,
    org: team.mode,
    args: params.getAll('arg').join('\n'),
    scale: Math.min(4, Math.max(1, window.devicePixelRatio || 1)),
    dark: dark(),
    touch: matchMedia('(pointer: coarse)').matches,
    probe: params.get('probe') === '1',
    avail: room(),
    ring: ringBuf,
    signal: signalBuf,
    bridge: bridgeMem,
  });
}

main().catch((e) => {
  report.error = String((e && e.stack) || e);
  refuse('This app did not open', 'Something went wrong in this tab. Reload to try again.', { act: 'home', detail: String((e && e.message) || e) });
});
