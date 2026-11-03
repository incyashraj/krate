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

import init, { consent, Grants } from './krate_web.js';
import { makeBridgeMemory, pageBridge } from './bridge.js';
import { makeRing, pageRing } from './ring.js';
import { makeAudioMemory, pageAudio } from './audio.js';
import { TeamData } from './org-data.js';
import { SharedStore, SharedFail } from './shared-store.js';
import { SecretStore, SecretFail } from './secret-store.js';
import { pageSenses, primeSpeech } from './senses.js';
// The browser as an organisation's device (window.KrateSigning): the same
// file the console uses on krate.tech, served here beside the Player.
import './org-device.js';

const $ = (id) => document.getElementById(id);
const report = (window.__krate = { marks: {}, frames: 0, latencies: [], paintLatencies: [], notices: [], out: '', state: 'loading', dialogs: [], saved: [] });
const mark = (name) => { report.marks[name] = performance.now(); };
const SITE = 'https://krate.tech';
const params = new URLSearchParams(location.search);
// /run/<hash> opens one release; /run/org/<slug>/<app-id> opens an
// organisation's app as it is now, and /run/v<line> a public app as it is
// now (IC-976): the addresses a home screen keeps, so an update reaches it.
// /run/file opens a .krate file the person has, without sending it anywhere.
const RUN = location.pathname.match(/^\/run\/(?:([0-9a-f]{64})|org\/([a-z][a-z0-9-]{1,38})\/([a-z0-9][a-z0-9._-]{0,127})|(v[0-9a-f]{11})|(file))\/?$/) || [];
let HASH = RUN[1];
const ORG_APP = RUN[2] ? { slug: RUN[2], app: RUN[3] } : null;
const LINE = RUN[4] || null;
const FILE = RUN[5] === 'file';
const FROM = ORG_APP ? `player:org/${ORG_APP.slug}/${ORG_APP.app}` : `player:${HASH || LINE}`;
// The post a link came from (`?via=reddit`), carried to the open's count.
const VIA = ((params.get('via') || '').toLowerCase().match(/^[a-z0-9][a-z0-9_-]{0,31}$/) || [''])[0];
// A phone or a tablet: nothing here may tell it to use "your computer".
const PHONE = /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent) || (navigator.maxTouchPoints > 1 && /Macintosh/.test(navigator.userAgent));
// A chat or social app's own browser (Instagram, Facebook, LinkedIn,
// TikTok, Snapchat, LINE, X): it may not give a tab what an app needs, and
// the way out is the phone's own browser, never a computer.
const IN_APP = (() => {
  const ua = navigator.userAgent;
  for (const [name, re] of [['Instagram', /Instagram/], ['Facebook', /FBAN|FBAV|FB_IAB|FBIOS/], ['LinkedIn', /LinkedInApp/],
    ['TikTok', /TikTok|musical_ly|BytedanceWebview/], ['Snapchat', /Snapchat/], ['LINE', /\bLine\//], ['X', /Twitter for/]]) {
    if (re.test(ua)) return name;
  }
  return /; wv\)/.test(ua) ? 'this app' : null;
})();
const SESSION_KEY = 'krate-player-session';
const NONCE_KEY = 'krate-player-nonce';

// ------------------------------------------------------------ look and words
const forced = params.get('theme');
if (forced === 'dark' || forced === 'light') document.documentElement.dataset.theme = forced;
const dark = () => (forced ? forced === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches);

const STATES = ['loading', 'file', 'consent', 'signin', 'refused', 'app'];
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
  // A file is not on the hub: the way on is another file, or (on a
  // computer) Krate itself.
  if (FILE && (act === 'home' || (act === 'krate' && PHONE))) act = 'another';
  if (act === 'another') { a.textContent = 'Choose another file'; a.href = '/run/file'; }
  else if (act === 'browser' && /Android/.test(navigator.userAgent)) {
    // Android hands an intent: link to Chrome, out of the app's browser.
    a.textContent = 'Open in Chrome';
    a.href = `intent://${location.host}${location.pathname}${location.search}#Intent;scheme=${location.protocol.replace(':', '')};package=com.android.chrome;end`;
  } else if (act === 'browser') { a.textContent = 'Copy the link'; a.href = '#'; a.onclick = copyLink; }
  else if (act === 'krate' && PHONE) { a.textContent = 'Send it to a computer'; a.href = '#'; a.onclick = sendToComputer; }
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
/// This page's address on the clipboard, to paste into the phone's browser.
async function copyLink(e) {
  if (e) e.preventDefault();
  const button = e && e.currentTarget;
  const url = location.origin + location.pathname + location.search;
  try {
    await navigator.clipboard.writeText(url);
    report.copied = url;
    if (button) { const was = button.textContent; button.textContent = 'Link copied'; setTimeout(() => { button.textContent = was; }, 1500); }
  } catch { prompt('Copy this link:', url); }
}
function openInKrate() {
  // A file opens in Krate by a double-click once it is installed.
  if (FILE) return `${SITE}/open/`;
  return HASH ? `${SITE}/open/?a=${HASH}${report.private ? '&private=1' : ''}` : `${SITE}/open/`;
}

// What a tab cannot do yet, in the words a person uses.
const WHAT = {
  'krate:gfx/canvas2d': 'drawing its own screen',
  'krate:gfx/canvas-images': 'drawing pictures',
  'krate:gfx/scene3d': '3D drawing',
  'krate:gfx/video': 'video',
  'krate:gpu/gpu': 'your graphics card',
  'krate:speech/transcription': 'speech',
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
  'krate:ui/dialog': 'choosing a folder',
  'krate:ui/reminders': 'reminders',
  'krate:ui/print': 'printing',
  'krate:ui/menu': 'its own menus',
  'krate:ui/bluetooth': 'Bluetooth',
  'krate:ui/photos': 'your photos',
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
  // A file opened here is the person's own business: the hub never hears of
  // it, not even its digest.
  if (!HASH || FILE) return Promise.resolve(null);
  const me = session();
  return fetch('/tab', {
    method: 'POST',
    keepalive: true,
    headers: { 'content-type': 'application/json', ...(me && me.token ? { authorization: `Bearer ${me.token}` } : {}) },
    // A proof run says so: the hub counts it as nobody and mails nobody.
    body: JSON.stringify({ event, app: HASH, via: VIA, ...(params.get('probe') === '1' ? { probe: true } : {}) }),
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
  if (FILE) {
    // Nothing about the file goes with the link, and there is nothing on
    // the hub to report: a file is taken down by not sending it.
    $('makeOwn').href = `${SITE}/app/?via=player-file`;
    $('reportOpen').hidden = true;
  } else {
    $('makeOwn').href = `${SITE}/app/?via=player&from=${HASH}`;
    $('makeOwn').onclick = () => { beacon('make'); };
  }
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
// The proof that this browser started a sign-in (K-909), as the site keeps
// it (docs/landing/login/signin.js): a few nonces in this origin's
// localStorage, each used once and dropped after an hour. localStorage, not
// sessionStorage, because an emailed link opens in a new tab.
const NONCE_MAX_MS = 60 * 60 * 1000;
function nonces() {
  try {
    const list = JSON.parse(localStorage.getItem(NONCE_KEY) || '[]');
    return Array.isArray(list) ? list.filter((e) => e && typeof e.n === 'string' && Date.now() - e.at < NONCE_MAX_MS) : [];
  } catch { return []; }
}
function keepNonces(list) {
  try { localStorage.setItem(NONCE_KEY, JSON.stringify(list.slice(-5))); } catch {}
}
function beginSignIn() {
  const n = Array.from(crypto.getRandomValues(new Uint8Array(18)), (b) => b.toString(16).padStart(2, '0')).join('');
  keepNonces([...nonces(), { n, at: Date.now() }]);
  return n;
}
function acceptSignIn(n) {
  if (!n) return false;
  const list = nonces();
  const at = list.findIndex((e) => e.n === n);
  if (at >= 0) { list.splice(at, 1); keepNonces(list); return true; }
  // A sign-in started before this Player kept its nonces here.
  let legacy = null;
  try { legacy = sessionStorage.getItem(NONCE_KEY); sessionStorage.removeItem(NONCE_KEY); } catch {}
  return legacy === n;
}
// A finished sign-in comes back in the fragment (it never leaves the
// browser), with a nonce this browser made: taken only when it matches.
function takeSignIn() {
  if (!location.hash.includes('token=')) return;
  const got = new URLSearchParams(location.hash.slice(1));
  history.replaceState(null, '', location.pathname + location.search);
  if (!acceptSignIn(got.get('nonce'))) return;
  try {
    localStorage.setItem(SESSION_KEY, JSON.stringify({ token: got.get('token'), login: got.get('login') || '', name: got.get('name') || '' }));
  } catch {}
}
function signIn(words) {
  const nonce = beginSignIn();
  const q = (path, extra = {}) => `${path}?${new URLSearchParams({ ...extra, from: FROM, nonce })}`;
  $('signinGithub').href = q('/login/start');
  $('signinGoogle').href = q('/login/google/start');
  // An organisation's app: its own sign-in first (Okta, Entra, Google
  // Workspace, SAML -- the hub sends it on to whichever the company set
  // up), with GitHub stepping back beside Google, as on krate.tech/login.
  if (ORG_APP) {
    $('signinCompany').href = q('/login/oidc/start', { org: ORG_APP.slug });
    $('signinCompany').hidden = false;
    $('signinGithub').className = 'btn ghost';
  }
  if (words) $('signinWords').textContent = words;
  show('signin');
}
// Sign in by a link in an email: the hub mails it, and opening it on this
// device comes back here signed in.
$('signinMail').onsubmit = async (e) => {
  e.preventDefault();
  const email = $('signinEmail').value.trim();
  const said = $('signinSaid');
  said.hidden = false;
  if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
    said.textContent = 'Type the email address your team invited.';
    $('signinEmail').focus();
    return;
  }
  $('signinSend').disabled = true;
  said.textContent = 'Sending…';
  const res = await fetch('/login/email', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ email, from: FROM, nonce: beginSignIn() }),
  }).catch(() => null);
  $('signinSend').disabled = false;
  if (res && res.ok) {
    said.textContent = `We sent a link to ${email}. Open it on this device to sign in; it works for 15 minutes.`;
  } else if (res && res.status === 429) {
    said.textContent = 'Too many links asked for just now. Wait a minute and try again.';
  } else {
    said.textContent = res ? ((await res.text().catch(() => '')) || 'The link did not send. Try again.') : 'Krate could not be reached. Check the connection and try again.';
  }
};

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

/// A public app's own address (/run/v<line>): the release it opens now.
/// True when HASH is set; otherwise the page already says what to do.
let lineOf = Promise.resolve(null);
async function resolveLine() {
  const res = await fetch(`/meta/${LINE}`, { cache: 'no-store' }).catch(() => null);
  if (!res) {
    refuse('This app did not open', 'Krate could not be reached. Check the connection and try again.', { act: 'home' });
    return false;
  }
  const body = await res.json().catch(() => ({}));
  if (res.ok && /^[0-9a-f]{64}$/.test(body.id || '')) { HASH = body.id; return true; }
  if (res.status === 451) refuse('This app was taken down', body.reason ? `Krate took it down: ${body.reason}` : 'Krate took it down, and it does not open.', { act: 'home' });
  else refuse('This app is not on Krate', 'The link may be mistyped, or the app was removed.', { act: 'home' });
  return false;
}

/// Make the page addable to a home screen as the app: its name, its icon,
/// and for an organisation's app the address that always opens the version
/// the team released last.
async function installable(name, slug, appId) {
  const id = String(appId || '').toLowerCase();
  const line = LINE || (await lineOf);
  const base = slug && /^[a-z0-9][a-z0-9._-]{0,127}$/.test(id) ? `/run/org/${slug}/${id}` : line ? `/run/${line}` : `/run/${HASH}`;
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

/// The tab can run an app: it is cross-origin isolated (the shared memory
/// the app's worker needs). Otherwise the page says what to do, and false.
function isolated() {
  if (self.crossOriginIsolated && typeof SharedArrayBuffer === 'function') return true;
  const android = /Android/.test(navigator.userAgent);
  const browser = android ? 'Chrome' : 'Safari';
  const whose = IN_APP && IN_APP !== 'this app' ? `${IN_APP}'s browser` : 'This in-app browser';
  if (PHONE) {
    refuse(`Open this in ${browser}`,
      `${whose} cannot run Krate apps. ` + (android
        ? 'Tap Open in Chrome, or tap ⋮ and choose Open in Chrome.'
        : 'Tap ⋯ or the share button and choose Open in Safari, or copy the link and paste it there.'),
      { act: 'browser', detail: `crossOriginIsolated=${self.crossOriginIsolated}` });
  } else {
    refuse('This browser cannot open Krate apps in a tab',
      'It does not allow the shared memory a Krate app needs here. The app opens in Krate on this computer.',
      { detail: `crossOriginIsolated=${self.crossOriginIsolated}` });
  }
  return false;
}

// ------------------------------------------------------------ a file
// /run/file: a .krate the person has -- from a chat, AirDrop, an email, a
// download -- opened in this tab without being sent anywhere. The page does
// here what the hub does for a link (web-player.js ensureWebPack): the
// validator judges the file and hands over its component exactly as judged,
// and the transpiler the hub uses makes the web pack from it. From there it
// is the link's own path (openChecked): the same checks, the same consent
// card, the same hosts. Its storage is keyed by its own bytes.

/// The hub's limit for making a web pack (MAX_PACK_SOURCE_BYTES), and its
/// words for a bundle past it.
const MAX_FILE_BYTES = 16 * 1024 * 1024;
let fileModules = null;
/// The Player, the validator and the transpiler, fetched and compiled once,
/// from the moment the page knows it is for a file.
function loadForFile() {
  if (!fileModules) {
    const compiled = (path, name) => WebAssembly.compileStreaming(fetch(path)).then((m) => { mark(`${name}Compiled`); return m; });
    fileModules = Promise.all([
      compiled('/player/krate_web_bg.wasm', 'player'),
      compiled('/player/validator.wasm', 'validator'),
      compiled('/player/transpiler.wasm', 'transpiler'),
    ]);
  }
  return fileModules;
}
/// The component of a bundle exactly as the validator judged it, as the hub
/// asks for it (index.js judgedComponent): `{ ok, bytes }` or `{ ok, problem }`.
async function judgedComponent(validatorModule, bytes) {
  const v = (await WebAssembly.instantiate(validatorModule, {})).exports;
  const input = v.krate_alloc(bytes.length);
  new Uint8Array(v.memory.buffer, input, bytes.length).set(bytes);
  const out = v.krate_judged_component(input, bytes.length);
  const length = new DataView(v.memory.buffer).getUint32(out, true);
  const body = new Uint8Array(v.memory.buffer, out + 4, length).slice();
  v.krate_free(out, 4 + length);
  v.krate_free(input, bytes.length);
  return body[0] === 1 ? { ok: true, bytes: body.subarray(1) } : { ok: false, problem: new TextDecoder().decode(body.subarray(1)) };
}
/// The web pack for a judged component, as the hub makes it
/// (web-player.js transpileComponent): `{ header, bytes }`.
async function transpileHere(transpilerModule, component) {
  const t = (await WebAssembly.instantiate(transpilerModule, {})).exports;
  const input = t.krate_alloc(component.length);
  new Uint8Array(t.memory.buffer, input, component.length).set(component);
  const out = t.krate_transpile(input, component.length);
  const length = new DataView(t.memory.buffer).getUint32(out, true);
  const bytes = new Uint8Array(t.memory.buffer, out + 4, length).slice();
  t.krate_free(out, 4 + length);
  t.krate_free(input, component.length);
  const hlen = new DataView(bytes.buffer).getUint32(0, true);
  return { header: JSON.parse(new TextDecoder().decode(bytes.subarray(4, 4 + hlen))), bytes };
}

let opening = false;
/// Open one file the person chose, dropped, shared or double-clicked.
function openFile(file) {
  return openFileNow(file).catch(failed);
}
async function openFileNow(file) {
  if (opening || !file) return;
  opening = true;
  report.file = { name: String(file.name || 'app.krate'), size: file.size };
  mark('fileChosen');
  say('Opening the file...');
  show('loading');
  if (file.size > MAX_FILE_BYTES) {
    refuse('This app opens in Krate, not in a tab yet', 'This app is too big to open in a tab. It opens in Krate on a computer.');
    return;
  }
  let bytes;
  try { bytes = new Uint8Array(await file.arrayBuffer()); } catch (e) {
    refuse('This file did not open', 'The browser could not read it. Choose it again.', { act: 'another', detail: String((e && e.message) || e) });
    return;
  }
  const [playerModule, validatorModule, transpilerModule] = await loadForFile();
  await init({ module_or_path: playerModule });
  HASH = await sha256(bytes);
  say('Checking it...');
  // The validator's verdict first, in its own words, before anything of the
  // file is made into something to run -- as the hub's door refuses a bad
  // upload before it is ever a link.
  const first = await judge(validatorModule, bytes);
  if (!first.ok) {
    refuse('This app did not pass Krate’s checks', 'It was not opened.', { act: 'home', detail: first.problem });
    return;
  }
  say('Getting it ready for this tab...');
  const component = await judgedComponent(validatorModule, bytes);
  if (!component.ok) {
    refuse('This app did not pass Krate’s checks', 'It was not opened.', { act: 'home', detail: component.problem });
    return;
  }
  const started = performance.now();
  const pack = await transpileHere(transpilerModule, component.bytes);
  report.transpileMs = performance.now() - started;
  mark('transpiled');
  if (!pack.header.ok) {
    refuse('This app opens in Krate, not in a tab yet', 'This app opens in Krate, not in a tab yet.', { detail: pack.header.problem });
    return;
  }
  // [u32 bundle length][bundle][pack]: what /a/<hash>/web answers.
  const buf = new Uint8Array(4 + bytes.length + pack.bytes.length);
  new DataView(buf.buffer).setUint32(0, bytes.length, true);
  buf.set(bytes, 4);
  buf.set(pack.bytes, 4 + bytes.length);
  say('Checking it is the app in the file...');
  await openChecked(buf.buffer, { told: '', validatorModule, playerModule, me: session() });
}

/// The file card, and every way a file reaches it.
async function fileMode() {
  document.title = 'Open a .krate file - Krate';
  $('inKrate').href = openInKrate();
  if (PHONE) $('inKrate').hidden = true;
  // The Player as an app that opens .krate files (file_handlers,
  // share_target), and the worker that takes a shared file in the browser.
  const manifest = document.createElement('link');
  manifest.rel = 'manifest';
  manifest.href = '/run/file/app.webmanifest';
  document.head.appendChild(manifest);
  report.launchQueue = 'launchQueue' in window;
  if (!isolated()) return;
  // A file double-clicked on a computer that installed the Player. The
  // browser holds the launch until a consumer is set.
  if (report.launchQueue) {
    window.launchQueue.setConsumer(async (launch) => {
      const handle = launch && launch.files && launch.files[0];
      if (!handle) return;
      report.launched = true;
      let file;
      try { file = await handle.getFile(); } catch (e) {
        refuse('This file did not open', 'The browser could not read it. Choose it again.', { act: 'another', detail: String((e && e.message) || e) });
        return;
      }
      openFile(file);
    });
  }
  if ('serviceWorker' in navigator) {
    navigator.serviceWorker.register('/player/file-sw.js', { scope: '/run/file' })
      .then(() => { report.fileWorker = true; }).catch((e) => { report.fileWorker = String((e && e.message) || e); });
  }
  loadForFile().catch((e) => { report.error = String((e && e.stack) || e); });

  // A file shared to the installed Player (Android): file-sw.js kept it in
  // this origin's cache. Read once, then gone; nothing else is kept there.
  const sharedId = params.get('shared');
  if (window.caches) {
    try {
      const cache = await caches.open('krate-file-share');
      const keys = await cache.keys();
      let shared = null;
      for (const key of keys) {
        if (sharedId && new URL(key.url).pathname === `/run/file/shared/${sharedId}`) {
          const res = await cache.match(key);
          if (res) shared = new File([await res.blob()], decodeURIComponent(res.headers.get('x-krate-name') || 'app.krate'));
        }
        await cache.delete(key);
      }
      if (sharedId) history.replaceState(null, '', location.pathname);
      if (shared) { report.shared = true; openFile(shared); return; }
    } catch { /* the card below */ }
  }

  const input = $('fileInput');
  // An iPhone or an Android phone greys out every file whose type it does
  // not know when told to accept .krate, so a phone is not told.
  if (!PHONE) input.accept = '.krate,application/x-krate';
  input.addEventListener('change', () => { const f = input.files && input.files[0]; input.value = ''; openFile(f); });
  $('filePick').onclick = () => input.click();
  const zone = $('fileDrop');
  const dragging = (e) => Boolean(e.dataTransfer && Array.from(e.dataTransfer.types || []).includes('Files'));
  // Dropped anywhere on the card's page, the file opens here; the browser
  // never opens it in place of the page.
  document.addEventListener('dragover', (e) => {
    if (!dragging(e)) return;
    e.preventDefault();
    if (report.state === 'file') { e.dataTransfer.dropEffect = 'copy'; zone.classList.add('over'); }
  });
  document.addEventListener('dragleave', (e) => { if (!e.relatedTarget) zone.classList.remove('over'); });
  document.addEventListener('drop', (e) => {
    if (!dragging(e)) return;
    e.preventDefault();
    zone.classList.remove('over');
    if (report.state !== 'file') return;
    report.dropped = true;
    openFile(e.dataTransfer.files && e.dataTransfer.files[0]);
  });

  // What this device can do beyond the picker, in its own words.
  if (PHONE) $('fileDropSay').hidden = true;
  const tip = $('fileTip');
  const android = /Android/.test(navigator.userAgent);
  const chromium = /Chrome\/|Chromium\//.test(navigator.userAgent) && !/Edg\/|OPR\//.test(navigator.userAgent);
  if (params.get('share') === 'lost' || params.get('share') === 'empty') {
    tip.textContent = 'The shared file did not reach this page. Choose it here instead.';
  } else if (isIos()) {
    tip.textContent = 'Got it in Messages, Mail or WhatsApp? Save it to Files first, then choose it here.';
  } else if (android && chromium) {
    tip.textContent = standalone()
      ? 'From any app, share a .krate and choose Krate.'
      : 'Add this page to your home screen, and sharing a .krate from any app opens it here.';
  } else if (!PHONE && report.launchQueue) {
    tip.textContent = standalone()
      ? 'A .krate file on this computer opens here: choose Open With, then Krate.'
      : 'Install this page, and a .krate file on this computer opens here like any other app.';
    const offer = () => { $('fileInstall').hidden = !installPrompt || standalone(); };
    addEventListener('beforeinstallprompt', () => setTimeout(offer));
    $('fileInstall').onclick = async () => {
      if (!installPrompt) return;
      installPrompt.prompt();
      await installPrompt.userChoice.catch(() => null);
      installPrompt = null;
      offer();
    };
    offer();
  }
  $('fileFine').hidden = !tip.textContent;
  show('file');
  mark('fileCard');
}

async function main() {
  mark('start');
  takeSignIn();
  if (FILE) return fileMode();
  if (ORG_APP && self.crossOriginIsolated && !(await resolveOrgApp())) return;
  if (LINE && !(await resolveLine())) return;
  if (!HASH) {
    refuse('This is not an app link', 'A Krate app link ends in the app’s 64-character address.', { act: 'home' });
    return;
  }
  report.private = params.get('private') === '1';
  $('inKrate').href = openInKrate();
  // A phone or a tablet in any orientation: Krate's own app is for a
  // computer, so the bar does not offer it.
  if (PHONE) $('inKrate').hidden = true;
  // The app's own link, for the home screen (IC-976). Asked now, beside the
  // fetches below; nothing waits on it but the home screen.
  if (!LINE && !ORG_APP) lineOf = fetch(`/meta/${HASH}`, { cache: 'no-store' }).then((r) => (r.ok ? r.json() : null)).then((m) => {
    const v = m && m.version;
    const line = v && !v.newer && /\/a\/(v[0-9a-f]{11})$/.exec(String(v.app_url || ''));
    return line ? line[1] : null;
  }).catch(() => null);
  if (!isolated()) return;

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
  await openChecked(await res.arrayBuffer(), { told: res.headers.get('x-krate-store') || '', validatorModule, playerModule, me });
}

/// Steps 2 to 4 for what the hub sent for a link, or for a file the person
/// opened made into the same thing in this page: the same checks, the same
/// consent card, the same hosts.
async function openChecked(buf, { told, validatorModule, playerModule, me }) {
  const { bundle, meta, files } = unpack(buf);
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
  // An app that speaks: the person's first tap readies an iPhone's voice.
  if ((meta.imports || []).some((n) => /^krate:speech\/synthesis(@|$)/.test(n))) primeSpeech();
  document.title = `${name} - Krate`;
  $('whoName').textContent = name;
  const release = sig === 'valid' && j.release;
  $('whoSig').textContent = (release ? `Signed by ${release.namespace.split('/')[0]}` : 'Not signed') + (FILE ? ', from a file' : '');
  $('who').hidden = false;
  report.app = { id: app.id, name, world: app.world, signature: sig };
  madeWith(report.private || Boolean(sig === 'valid' && j.org_mark));
  // A file has no address to keep on a home screen; the page stays the
  // Player that opens files.
  if (!FILE) installable(name, sig === 'valid' ? j.org_mark : null, app.id);

  // ---- consent: the desktop's words, the tab's reach
  const rows = JSON.parse(consent(JSON.stringify(j.manifest.capabilities || []))).rows || [];
  // A team's data comes with the team's link, which the hub opens only for
  // a member; a file has no one to ask, so it opens without it.
  const TEAM_LINK = 'opens from your team’s link, not from a file';
  if (FILE) for (const r of rows) if (r.cap === 'store.org' && r.in_tab) { r.in_tab = false; r.away = TEAM_LINK; }
  const blocked = rows.filter((r) => r.required && !r.in_tab);
  const granted = rows.filter((r) => r.in_tab).map((r) => r.cap);
  // Storage belongs to the publisher the hub vouched for (x-krate-store, a
  // digest of the signed-in owner), never to the manifest's id alone: the
  // author picks that id, every app shares this origin, and another
  // publisher's app declaring the same id read this one's data, secrets
  // included. With no tag, the build itself -- never the bare id.
  const owner = /^k[0-9a-f]{16}$/.test(told) ? told : `h${HASH.slice(0, 16)}`;
  // A file's storage is its own bytes' (`f`), whoever signed it: no link
  // keys anything with `f`, and an app id cannot hold `~` or `@`, so a file
  // never reaches a published app's data -- not even the same build's
  // opened from its link -- and the same file always finds its own.
  const storeKey = FILE ? `${app.id}~f${HASH.slice(0, 16)}`
    : release ? `${app.id}@${release.authority.slice(0, 16)}` : `${app.id}~${owner}`;
  report.storeKey = storeKey;
  if (rows.length) {
    $('consentTitle').textContent = `“${name}” wants to:`;
    const list = $('consentRows');
    list.textContent = '';
    for (const r of rows) {
      const li = document.createElement('li');
      if (!r.in_tab) li.className = 'away';
      li.innerHTML = '<i><svg viewBox="0 0 16 16" fill="none"><path d="M3.5 8.5l3 3 6-6.5" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/></svg></i><span><b></b><em></em></span>';
      li.querySelector('b').textContent = r.plain;
      li.querySelector('em').textContent = r.in_tab ? r.rationale : `${r.rationale ? r.rationale + ' - ' : ''}${r.away || 'works in Krate on a computer'}`;
      list.appendChild(li);
    }
    const teamOnly = (list) => list.length > 0 && list.every((r) => r.away === TEAM_LINK);
    if (blocked.length) {
      $('consentNote').textContent = teamOnly(blocked)
        ? `${name} needs your team’s shared data, which ${TEAM_LINK}.`
        : `${name} needs ${blocked.map((r) => r.plain).join(', ')}, which works in Krate on a computer, not in a tab yet.`;
      if (FILE && PHONE) {
        $('consentAllow').textContent = 'Choose another file';
        $('consentAllow').onclick = () => { location.href = '/run/file'; };
      } else {
        $('consentAllow').textContent = PHONE ? 'Send it to a computer' : 'Open in Krate';
        $('consentAllow').onclick = PHONE ? sendToComputer : () => { location.href = openInKrate(); };
      }
    } else {
      const away = rows.filter((r) => !r.in_tab);
      $('consentNote').textContent = !away.length ? ''
        : teamOnly(away) ? 'Greyed out: it opens from your team’s link. The app opens here without it.'
          : 'Greyed out: works in Krate on a computer. The app opens here without it.';
      $('consentAllow').textContent = 'Allow and open';
    }
    if (FILE) {
      $('consentFine').textContent = `Opened from the file “${report.file.name}” on this device. Nothing was uploaded. It gets nothing else, runs in this tab and stops when you close it.`;
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
  const org = { slug: (!FILE && sig === 'valid' && j.org_mark) || null, appId: app.id, digest: HASH, session: me, validatorModule };
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
  const keeps = ['store.kv', 'store.sql', 'store.shared', 'store.secret'].some((c) => granted.includes(c));
  const d = gui || keeps ? await db().catch(() => null) : null;
  if (d && keeps) keepData();
  // The app's database, as the browser kept it: one value per app under a
  // number, which no key-value key (always a string) can be or reach, so
  // store.kv's clear never touches it.
  const sqlKey = [storeKey, 0];
  const sqlImage = d && granted.includes('store.sql') ? await tx(d, 'readonly', (s) => s.get(sqlKey)).catch(() => null) : null;
  // The app's shared store (store.shared) and secrets (store.secret), under
  // numbers of their own beside the database's, for the same reason.
  const NO_STORAGE = 'this browser keeps no storage for this tab';
  const slot = (n) => ({
    get: () => (d ? tx(d, 'readonly', (s) => s.get([storeKey, n])) : Promise.reject(new Error(NO_STORAGE))),
    put: (v) => (d ? tx(d, 'readwrite', (s) => s.put(v, [storeKey, n])) : Promise.reject(new Error(NO_STORAGE))),
    add: (v) => (d ? tx(d, 'readwrite', (s) => s.add(v, [storeKey, n])) : Promise.reject(new Error(NO_STORAGE))),
  });
  const shared = granted.includes('store.shared')
    ? await new SharedStore({ hub: location.origin, windowed: gui, load: slot(1).get, save: slot(1).put }).open()
    : null;
  const secrets = granted.includes('store.secret')
    ? await new SecretStore({ storeKey, getKey: slot(3).get, addKey: slot(3).add, getSealed: slot(2).get, putSealed: slot(2).put }).open()
    : null;
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
  // Sound (K-1134): heard from the person's first tap or key.
  const audioMem = makeAudioMemory();
  const audio = pageAudio({ memory: audioMem, probe: params.get('probe') === '1', report });
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
  // One store.shared or store.secret call: the store's own answer, its
  // WIT error, or -- for anything it did not expect -- io in words, so the
  // app always hears one of its own error cases. Without the grant there
  // is no store, and the answer is denied, as on desktop.
  const storeOp = (which, Fail, fn) => async (args) => {
    const store = which();
    if (!store) return { json: { err: { tag: 'denied' } } };
    try {
      const out = await fn(store, args || {});
      if (out && typeof out === 'object' && 'bytes' in out) return out.bytes ? { json: { ok: true }, body: out.bytes } : { json: { ok: false } };
      return { json: { ok: out === undefined ? null : out } };
    } catch (e) {
      if (e instanceof Fail) return { json: { err: e.payload } };
      return { json: { err: { tag: 'io', val: String((e && e.message) || e) } } };
    }
  };
  const kvWrite = async (fn) => {
    try { await tx(d, 'readwrite', fn); return { json: { ok: null } }; }
    catch (e) { return { json: { err: { tag: 'io', val: `the browser did not save it (${(e && e.message) || e})` } } }; }
  };
  const sockets = new Map();
  let nextSocket = 1;

  // ---- the app's dialogs, and the files the person hands it (IC-974)
  // A file is held here by token, never by path: the browser's own File
  // for one picked or dropped (read only when the app reads it), a Blob for
  // what the app wrote. The list dies with the tab, so a token is good for
  // this run only, and a run holds at most 64, as on desktop
  // (crates/runtime/src/chosen_files.rs).
  const chosen = new Map();
  let issued = 0;
  const remember = (fileName, blob, kind) => {
    if (chosen.size >= 64) return null;
    issued += 1;
    const token = `chosen-${issued}`;
    chosen.set(token, { name: fileName, blob, kind, modified: (blob && blob.lastModified) || Date.now() });
    return token;
  };
  const TOO_MANY = { err: { tag: 'unsupported', val: 'too many files chosen in one run' } };
  // Whether this moment is inside the person's own tap or key: a browser
  // opens a file picker or starts a download only then. Without a way to
  // tell, the answer is no, and the person is asked to tap.
  const inGesture = () => Boolean(navigator.userActivation && navigator.userActivation.isActive);
  // A press still held: the picker opens inside its release, which is a
  // gesture on every browser (the app hears the press first).
  let held = 0;
  let onRelease = null;
  const released = () => {
    held = Math.max(0, held - 1);
    if (held === 0 && onRelease) { const go = onRelease; onRelease = null; go(); }
  };
  // What a browser allows only inside the person's own tap: a file picker,
  // the share sheet, asking about notifications. From a press still held,
  // inside its release; from a tap, at once; otherwise one tap on the sheet
  // away (`ask`).
  const withTap = (go, ask) => {
    if (held) onRelease = () => (inGesture() || !navigator.userActivation ? go('release') : ask());
    else if (inGesture()) go('direct');
    else ask();
  };
  // One sheet for every question: a message, a yes/no, "choose a file",
  // "save this file". Escape or a tap outside answers no.
  let answerSheet = null;
  const sheet = (title, body, yes, no) => new Promise((resolve) => {
    $('askTitle').textContent = title || name;
    $('askBody').textContent = body || '';
    $('askBody').hidden = !body;
    $('askYes').textContent = yes;
    $('askNo').textContent = no || '';
    $('askNo').hidden = !no;
    const done = (answer) => {
      if (answerSheet !== done) return;
      answerSheet = null;
      $('askSheet').hidden = true;
      canvas.focus({ preventScroll: true });
      resolve(answer);
    };
    answerSheet = done;
    $('askYes').onclick = () => done(true);
    $('askNo').onclick = () => done(false);
    $('askSheet').hidden = false;
    $('askYes').focus();
  });
  $('askSheet').addEventListener('click', (e) => { if (e.target === $('askSheet') && answerSheet) answerSheet(false); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && answerSheet) { e.preventDefault(); answerSheet(false); } });
  // A filter is "png,jpg": the picker is told .png,.jpg, as a hint.
  const accepts = (filter) => String(filter || '').split(',').map((e) => e.trim().replace(/^\./, ''))
    .filter((e) => /^[A-Za-z0-9]+$/.test(e)).map((e) => `.${e}`).join(',');
  const pickFile = ({ title, filter }) => new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.className = 'pick';
    input.tabIndex = -1;
    const accept = accepts(filter);
    if (accept) input.accept = accept;
    document.body.appendChild(input);
    let settled = false;
    const finish = (file) => {
      if (settled) return;
      settled = true;
      input.remove();
      if (answerSheet) answerSheet(false);
      // Cancelling is a normal answer, not a failure.
      if (!file) { resolve({ json: { ok: null } }); return; }
      const token = remember(file.name, file, 'open');
      resolve({ json: token ? { ok: { name: file.name, token } } : TOO_MANY });
    };
    input.addEventListener('change', () => finish(input.files && input.files[0]));
    input.addEventListener('cancel', () => finish(null));
    const open = (how) => { report.dialogs.push({ kind: 'open', how }); input.click(); };
    const ask = () => sheet(title || `${name} wants a file`, `${name} gets only the file you choose.`, 'Choose a file', 'Cancel')
      .then((yes) => (yes ? open('sheet') : finish(null)));
    withTap(open, ask);
  });
  // Where a save goes in a tab: the name the app suggested, cleaned as the
  // phone's save cleans it (export_path in crates/runtime), with the
  // filter's first extension when it has none.
  const saveName = (suggested, filter) => {
    let n = String(suggested || '').replace(/[\/\\\u0000-\u001f\u007f-\u009f]/g, '-').trim().replace(/^\.+/, '');
    if (!n) n = 'Untitled';
    const ext = String(filter || '').split(',').map((e) => e.trim().replace(/^\./, '')).find((e) => e && /^[A-Za-z0-9]+$/.test(e));
    if (ext && !/.\./.test(n)) n = `${n}.${ext}`;
    return n;
  };
  const download = (fileName, blob) => {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = fileName;
    a.className = 'pick';
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 60000);
    report.saved.push({ name: fileName, size: blob.size });
  };
  // The app let go of a file it wrote: hand it to the person. Inside their
  // tap it downloads at once; otherwise one tap on the sheet saves it.
  const deliver = (fileName, blob) => {
    if (inGesture()) { report.dialogs.push({ kind: 'save', how: 'direct' }); download(fileName, blob); return Promise.resolve(); }
    return sheet(`${name} saved ${fileName}`, 'Save it to this device.', 'Save', 'Not now').then((yes) => {
      report.dialogs.push({ kind: 'save', how: yes ? 'sheet' : 'declined' });
      if (yes) download(fileName, blob);
    });
  };
  // A file dropped on the app reaches it only under its ui.dropzone grant,
  // judged as the desktop judges it (by the file's name); a drag over it
  // is said only to an app that takes anything, as on desktop.
  const dropGrants = new Grants(JSON.stringify(granted));
  const dragging = (e) => Boolean(e.dataTransfer && Array.from(e.dataTransfer.types || []).includes('Files'));
  let over = false;
  const hover = (now) => {
    if (now === over) return;
    over = now;
    if (dropGrants.allows_drop('')) ring.push({ type: 'hover', window: win(), over: now });
  };
  // A file let go of anywhere else on the page is not opened by the browser
  // in place of the app.
  document.addEventListener('dragover', (e) => { if (dragging(e)) e.preventDefault(); });
  document.addEventListener('drop', (e) => { if (dragging(e)) e.preventDefault(); });

  // The device's senses (IC-974): notifications, location, share, speech,
  // the camera and the microphone (senses.js).
  const devices = pageSenses({ name, report, probe: params.get('probe') === '1', phone: PHONE, sheet, withTap });

  const bridge = pageBridge(bridgeMem, {
    ...devices.handlers,
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
    // A live connection (krate:net/ws, IC-974): events queue here until the
    // app polls; a terminal one retires the handle once it is read.
    'ws.open': ({ url }) => {
      // The desktop's limits (crates/runtime/src/async_ws.rs).
      if (sockets.size >= 16) return { json: { err: { tag: 'other', val: 'too many live connections: this app already holds 16' } } };
      let sock;
      try { sock = new WebSocket(url); } catch (e) { return { json: { err: { tag: 'invalid-url' } } }; }
      sock.binaryType = 'arraybuffer';
      const entry = { sock, queue: [], out: [], closing: false, failed: false };
      sock.onopen = () => {
        entry.queue.push({ state: 'opened' });
        for (const m of entry.out) sock.send(m);
        entry.out = [];
      };
      sock.onmessage = (ev) => {
        entry.queue.push(typeof ev.data === 'string' ? { state: 'text', text: ev.data } : { state: 'binary', bytes: new Uint8Array(ev.data) });
      };
      sock.onerror = () => { entry.failed = true; };
      sock.onclose = (ev) => {
        entry.queue.push(entry.failed || (!ev.wasClean && !entry.closing)
          ? { state: 'failed', why: `could not keep a connection to ${(() => { try { return new URL(url).host; } catch { return url; } })()}${ev.code && ev.code !== 1006 ? ` (code ${ev.code})` : ''}` }
          : { state: 'closed' });
      };
      const id = String(nextSocket++);
      sockets.set(id, entry);
      report.sockets = (report.sockets || 0) + 1;
      return { json: { ok: id } };
    },
    'ws.send': ({ handle, text, data }) => {
      const entry = sockets.get(handle);
      if (!entry || entry.closing || entry.sock.readyState > 1) return { json: { err: { tag: 'other', val: 'the connection is not open' } } };
      const message = text !== undefined ? text : data;
      const size = text !== undefined ? new TextEncoder().encode(text).length : data.byteLength;
      if (size > 1048576) return { json: { err: { tag: 'other', val: `message is ${size} bytes; the limit is 1048576` } } };
      if (entry.sock.readyState === 0) entry.out.push(message);
      else entry.sock.send(message);
      return { json: { ok: null } };
    },
    'ws.poll': ({ handle }) => {
      const entry = sockets.get(handle);
      if (!entry) return { json: { state: 'unknown' } };
      const next = entry.queue.shift();
      if (!next) return { json: { state: 'pending' } };
      if (next.state === 'closed' || next.state === 'failed') sockets.delete(handle);
      if (next.state === 'binary') return { json: { state: 'binary' }, body: next.bytes };
      return { json: next };
    },
    'ws.close': ({ handle }) => {
      const entry = sockets.get(handle);
      if (entry && !entry.closing) {
        entry.closing = true;
        try { entry.sock.close(1000); } catch (e) {}
      }
      return { json: { ok: null } };
    },
    'dialog.ask': async ({ kind, title, body }) => {
      report.dialogs.push({ kind, title });
      const yes = await sheet(title, body, kind === 'confirm' ? 'Yes' : 'OK', kind === 'confirm' ? 'No' : '');
      return { json: { ok: kind === 'confirm' ? yes : null } };
    },
    'dialog.open': pickFile,
    // Nothing to ask yet: a tab cannot choose a folder of theirs, so the
    // file goes to the person once the app has written it and let go.
    'dialog.save': ({ suggested, filter }) => {
      const fileName = saveName(suggested, filter);
      const token = remember(fileName, null, 'save');
      return { json: token ? { ok: { name: fileName, token } } : TOO_MANY };
    },
    'file.open': async ({ token, mode }) => {
      const entry = chosen.get(token);
      // A token nobody was handed, or a save not yet written, is not found.
      if (!entry || (mode === 'read' && !entry.blob)) return { json: { err: { tag: 'not-found' } } };
      const whole = (mode === 'read-write' || mode === 'append') && entry.blob;
      const body = whole ? new Uint8Array(await entry.blob.arrayBuffer()) : undefined;
      return { json: { ok: { size: entry.blob ? entry.blob.size : 0, modified: entry.modified, kind: entry.kind } }, body };
    },
    'file.read': async ({ token, at, len }) => {
      const entry = chosen.get(token);
      if (!entry || !entry.blob) return { json: { err: { tag: 'not-found' } } };
      try {
        return { json: { ok: null }, body: new Uint8Array(await entry.blob.slice(at, at + len).arrayBuffer()) };
      } catch (e) {
        // A picked file changed or moved since it was chosen.
        return { json: { err: { tag: 'io', val: `the browser could not read ${entry.name} again; choose it once more` } } };
      }
    },
    'file.close': ({ token, data, offer }) => {
      const entry = chosen.get(token);
      if (!entry) return { json: { ok: null } };
      entry.blob = new Blob([data], { type: 'application/octet-stream' });
      entry.modified = Date.now();
      // The app goes on while the person decides.
      if (offer) deliver(entry.name, entry.blob);
      return { json: { ok: null } };
    },
    'sql.save': ({ image }) => (d ? kvWrite((s) => s.put(image, sqlKey)) : { json: { err: { tag: 'io', val: 'this browser keeps no storage for this tab' } } }),
    'kv.set': ({ key, value }) => (d ? kvWrite((s) => s.put(value, kvKey(key))) : { json: { err: { tag: 'io', val: 'this browser keeps no storage for this tab' } } }),
    'kv.delete': ({ key }) => (d ? kvWrite((s) => s.delete(kvKey(key))) : { json: { ok: null } }),
    'kv.clear': () => (d ? kvWrite((s) => s.delete(range(storeKey))) : { json: { ok: null } }),
    'shared.code': storeOp(() => shared, SharedFail, (s) => s.code()),
    'shared.create': storeOp(() => shared, SharedFail, (s) => s.create()),
    'shared.join': storeOp(() => shared, SharedFail, (s, { code }) => s.join(code)),
    'shared.leave': storeOp(() => shared, SharedFail, (s) => s.leave()),
    'shared.get': storeOp(() => shared, SharedFail, (s, { key }) => ({ bytes: s.get(key) })),
    'shared.set': storeOp(() => shared, SharedFail, (s, { key, value }) => s.set(key, value)),
    'shared.delete': storeOp(() => shared, SharedFail, (s, { key }) => s.delete(key)),
    'shared.keys': storeOp(() => shared, SharedFail, (s) => s.keys()),
    'shared.sync': storeOp(() => shared, SharedFail, (s) => s.sync()),
    'secret.get': storeOp(() => secrets, SecretFail, (s, { name: n }) => ({ bytes: s.get(n) })),
    'secret.set': storeOp(() => secrets, SecretFail, (s, { name: n, value }) => s.set(n, value)),
    'secret.delete': storeOp(() => secrets, SecretFail, (s, { name: n }) => s.delete(n)),
    'secret.names': storeOp(() => secrets, SecretFail, (s) => s.names()),
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
      case 'audio': audio.onMessage(data); break;
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
    if (FILE) $('another').hidden = false;
    if (crashed) notice(words);
    worker.terminate();
    audio.close();
    devices.close();
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
  // A press held, for a file picker the app asks for while it is down.
  canvas.addEventListener('pointerdown', () => { held += 1; }, true);
  canvas.addEventListener('pointerup', released);
  canvas.addEventListener('pointercancel', () => { onRelease = null; released(); });
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
  canvas.addEventListener('dragenter', (e) => { if (dragging(e)) { e.preventDefault(); hover(true); } });
  canvas.addEventListener('dragover', (e) => { if (dragging(e)) { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; } });
  canvas.addEventListener('dragleave', () => hover(false));
  canvas.addEventListener('drop', (e) => {
    if (!dragging(e)) return;
    e.preventDefault();
    over = false;
    // One event per file, in the order the browser gave them.
    for (const file of Array.from(e.dataTransfer.files || [])) {
      if (!dropGrants.allows_drop(file.name)) continue;
      const token = remember(file.name, file, 'drop');
      if (token) ring.push({ type: 'drop', window: win(), name: file.name, token });
    }
  });
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
    sqlImage,
    storeKey,
    org: team.mode,
    senses: devices.facts,
    args: params.getAll('arg').join('\n'),
    scale: Math.min(4, Math.max(1, window.devicePixelRatio || 1)),
    dark: dark(),
    touch: matchMedia('(pointer: coarse)').matches,
    probe: params.get('probe') === '1',
    avail: room(),
    ring: ringBuf,
    signal: signalBuf,
    bridge: bridgeMem,
    audio: audioMem,
  });
}

function failed(e) {
  report.error = String((e && e.stack) || e);
  refuse('This app did not open', 'Something went wrong in this tab. Reload to try again.', { act: 'home', detail: String((e && e.message) || e) });
}
main().catch(failed);
