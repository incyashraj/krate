// The device's senses for a Krate app in a browser tab, the page's half
// (IC-974): notifications, where the device is, the share sheet, speaking
// aloud, the camera and the microphone.
//
// hosts.js, in the app's worker, has already checked the app's grant and
// what it handed over, exactly as the desktop does; nothing reaches here for
// an app without the capability, so the browser never asks the person about
// one. Here the browser is asked, and its answer becomes the desktop's: a
// "no" from the browser is the refusal the desktop gives when the system
// says no (camera system-denied, location not-allowed, notifications
// denied, the microphone "not allowed").
//
// What a browser allows only inside the person's own tap -- asking about
// notifications, opening the share sheet -- happens inside it when the app
// asked from a tap, and otherwise one tap on the Player's sheet away, the
// way the file picker does (player.js withTap).
//
// Nothing here blocks the app while the browser shows its own prompt for
// the camera, the microphone or the location: as on desktop, the app keeps
// running and the frames, the sound or the position arrive once allowed.

import { MIC_CHANNELS, MIC_FAILED, MIC_FAILURES, MIC_RATE } from './hosts.js';

const ok = (value) => ({ json: { ok: value === undefined ? null : value } });
const no = (tag, val) => ({ json: { err: val === undefined ? { tag } : { tag, val } } });

/// What a browser calls a file's kind, by its extension, for the share
/// sheet (Chrome shares only the kinds it knows).
const KINDS = {
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
  txt: 'text/plain', md: 'text/markdown', csv: 'text/csv', pdf: 'application/pdf',
  mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg', m4a: 'audio/mp4', mp4: 'video/mp4', webm: 'video/webm', mov: 'video/quicktime',
};
const kindOf = (fileName) => KINDS[(String(fileName).split('.').pop() || '').toLowerCase()] || 'application/octet-stream';

const permission = async (what) => {
  try { return (await navigator.permissions.query({ name: what })).state; } catch { return 'unknown'; }
};
const inputs = async (kind) => {
  try { return (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === kind); } catch { return []; }
};
const stopTracks = (stream) => { if (stream) for (const t of stream.getTracks()) t.stop(); };

const NO_NOTIFY = 'this browser has no notifications';
const NO_NOTIFY_PHONE = 'a phone\'s browser shows notifications only from an app on the Home Screen: tap Share, then Add to Home Screen, and open it from there';
const NO_SHARE = 'this browser has no share sheet';
const NO_VOICES = 'this browser has no speech voices Krate can use';
const NO_CAMERA = 'this browser cannot use a camera';
const NO_MIC = 'this browser cannot use a microphone';

/// Before the app runs: an iPhone speaks only once a page has spoken
/// inside a tap, so the person's first tap (the consent card's Allow, for
/// most apps) says nothing, silently, for an app that speaks.
export function primeSpeech() {
  if (typeof speechSynthesis === 'undefined') return;
  let primed = false;
  const prime = () => {
    if (primed) return;
    primed = true;
    try {
      const u = new SpeechSynthesisUtterance(' ');
      u.volume = 0;
      speechSynthesis.speak(u);
    } catch { /* nothing to prime */ }
  };
  for (const type of ['pointerup', 'keydown', 'click']) document.addEventListener(type, prime, { capture: true, once: true, passive: true });
}

/// ctx: { name, report, probe, phone, sheet(title, body, yes, no) -> Promise<bool>,
///        withTap(go(how), ask()) }
export function pageSenses({ name, report, probe, phone, sheet, withTap }) {
  const state = (report.senses = { asked: [], notified: [], shares: [], said: [], spoken: [], speechErrors: [], speechEvents: [], cameras: 0, microphones: 0 });

  // ------------------------------------------------------- notifications
  // Through a service worker everywhere: the only way a phone's browser
  // shows one, and the same code on a computer (notify-sw.js).
  let registration = null;
  const worker = () => {
    if (!('serviceWorker' in navigator)) return Promise.resolve(null);
    if (!registration) {
      registration = navigator.serviceWorker.register('/player/notify-sw.js', { scope: '/player/notify/' }).then(async (reg) => {
        const w = reg.installing || reg.waiting;
        if (!reg.active && w) {
          await new Promise((resolve) => {
            const on = () => { if (w.state === 'activated' || w.state === 'redundant') { w.removeEventListener('statechange', on); resolve(); } };
            w.addEventListener('statechange', on);
            on();
          });
        }
        return reg.active ? reg : null;
      }).catch(() => null);
    }
    return registration;
  };
  // "Not now" on the sheet is the answer for the rest of this run.
  let declined = false;
  const askNotify = () => new Promise((resolve) => {
    const go = (how) => {
      state.asked.push({ what: 'notifications', how });
      let p;
      try { p = Notification.requestPermission(); } catch { resolve(Notification.permission); return; }
      Promise.resolve(p).then((answer) => resolve(answer || Notification.permission), () => resolve(Notification.permission));
    };
    withTap(go, () => sheet(`${name} wants to send you notifications`, 'Your browser asks you next.', 'Allow notifications', 'Not now')
      .then((yes) => {
        if (yes) { go('sheet'); return; }
        declined = true;
        state.asked.push({ what: 'notifications', how: 'declined' });
        resolve('denied');
      }));
  });
  const notifyShow = async ({ title, body }) => {
    if (typeof Notification === 'undefined') return no('unavailable', phone ? NO_NOTIFY_PHONE : NO_NOTIFY);
    let answer = Notification.permission;
    if (answer === 'default' && !declined) answer = await askNotify();
    if (answer !== 'granted') return no('denied');
    // The app is the sender, from what the person agreed to, never from
    // the app's own text: its title is the first line under the name.
    const opts = { body: body ? `${title}\n${body}` : title, data: { page: location.href } };
    try {
      const reg = await worker();
      if (reg) await reg.showNotification(name, opts);
      else new Notification(name, opts); // eslint-disable-line no-new
      state.notified.push({ title: name, body: opts.body, how: reg ? 'worker' : 'page' });
      return ok();
    } catch (e) {
      return no('unavailable', `the browser did not show it (${(e && e.message) || e})`);
    }
  };

  // ------------------------------------------------------------ location
  // One watch, started by the first ask (which answers finding), read by
  // every later one; stopped when the app ends.
  let geo = null;
  const locationNow = () => {
    if (!navigator.geolocation) return no('unavailable');
    if (!geo) {
      geo = { fix: null, code: 0 };
      state.asked.push({ what: 'location' });
      try {
        geo.watch = navigator.geolocation.watchPosition(
          (p) => { geo.fix = p; geo.at = Date.now(); geo.code = 0; },
          (e) => {
            geo.code = e.code;
            // A "no" is the answer for this run, as on a phone.
            if (e.code === 1) { navigator.geolocation.clearWatch(geo.watch); geo.fix = null; }
          },
          { enableHighAccuracy: true, maximumAge: 0 },
        );
      } catch { geo.code = 2; }
      return no('finding');
    }
    if (geo.code === 1) return no('not-allowed');
    if (geo.fix) {
      const c = geo.fix.coords;
      const now = Date.now();
      // A fix's own time when it is a time (some engines report another
      // clock); otherwise when it reached this page.
      const ts = geo.fix.timestamp;
      const when = ts > now - 86400000 && ts <= now + 60000 ? ts : geo.at;
      return ok({ latitude: c.latitude, longitude: c.longitude, accuracy: c.accuracy, altitude: c.altitude, speed: c.speed, course: c.heading, age_ms: Math.max(0, now - when) });
    }
    if (geo.code === 2) return no('unavailable');
    return no('finding');
  };

  // --------------------------------------------------------------- share
  const shareNow = ({ text, url, files }) => new Promise((resolve) => {
    if (typeof navigator.share !== 'function') { resolve(no('unavailable', NO_SHARE)); return; }
    const data = {};
    if (text) data.text = text;
    if (url) data.url = url;
    if (files.length) data.files = files.map((f) => new File([f.data], f.name, { type: kindOf(f.name) }));
    if (typeof navigator.canShare === 'function' && !navigator.canShare(data)) {
      resolve(no('unavailable', files.length ? 'this browser cannot share these files' : 'this browser cannot share this'));
      return;
    }
    const go = (how) => {
      state.shares.push({ how, text: data.text, url: data.url, files: files.map((f) => `${f.name}:${f.data.length}`) });
      navigator.share(data).then(() => resolve(ok(true)), (e) => {
        const kind = e && e.name;
        if (kind === 'AbortError') resolve(ok(false));
        else if (kind === 'TypeError' || kind === 'DataError') resolve(no('invalid', String((e && e.message) || e)));
        else if (kind === 'NotAllowedError') resolve(no('unavailable', 'the browser did not open its share sheet'));
        else resolve(no('unavailable', String((e && e.message) || e)));
      });
    };
    const what = text || url || (files.length === 1 ? files[0].name : `${files.length} files`);
    withTap(go, () => sheet(`${name} wants to share`, what, 'Share', 'Cancel').then((yes) => {
      if (yes) { go('sheet'); return; }
      state.shares.push({ how: 'cancelled' });
      resolve(ok(false));
    }));
  });

  // ------------------------------------------------------ speaking aloud
  const synth = typeof speechSynthesis === 'undefined' ? null : speechSynthesis;
  // A line said before the person has tapped this page waits for the tap,
  // as sound does (audio.js): a browser speaks only after one.
  let waiting = null;
  const sayWaiting = () => {
    if (!waiting) return;
    const u = waiting;
    waiting = null;
    synth.speak(u);
  };
  if (synth) for (const type of ['pointerup', 'keydown', 'click']) document.addEventListener(type, sayWaiting, { capture: true, passive: true });
  const voicesNow = () => new Promise((resolve) => {
    const now = synth.getVoices();
    if (now.length) { resolve(now); return; }
    const done = () => resolve(synth.getVoices());
    synth.addEventListener('voiceschanged', done, { once: true });
    setTimeout(done, 1500);
  });
  const voices = async () => {
    if (!synth) return no('unsupported', NO_VOICES);
    const list = await voicesNow();
    if (!list.length) return no('unsupported', NO_VOICES);
    return ok(list.map((v) => ({ id: v.voiceURI, name: v.name, language: v.lang })));
  };
  const say = async ({ text, voice, rate }) => {
    if (!synth) return no('unsupported', NO_VOICES);
    // A browser with no voices at all accepts a line and then fails to say
    // it; the app is told so instead, in the words voices() uses.
    const list = await voicesNow();
    if (!list.length && (voice === null || voice === undefined)) return no('unsupported', NO_VOICES);
    let chosen = null;
    if (voice !== null && voice !== undefined) {
      chosen = list.find((v) => v.voiceURI === voice);
      if (!chosen) return no('invalid-request', `this browser has no voice ${voice}`);
    }
    // A new line replaces the old one rather than queueing behind it.
    synth.cancel();
    waiting = null;
    const u = new SpeechSynthesisUtterance(text);
    u.rate = rate;
    if (chosen) { u.voice = chosen; u.lang = chosen.lang; }
    // The proofs speak at no volume, as KRATE_SILENT_SPEECH does.
    if (probe) u.volume = 0;
    u.onstart = () => { state.speechEvents.push('start'); state.spoken.push(text); };
    u.onerror = (e) => { state.speechEvents.push(`error ${e.error}`); if (e.error !== 'interrupted' && e.error !== 'canceled') state.speechErrors.push(e.error); };
    u.onend = () => { state.speechEvents.push('end'); };
    const tapped = !navigator.userActivation || navigator.userActivation.hasBeenActive;
    if (tapped) synth.speak(u); else waiting = u;
    state.said.push({ text, voice: chosen ? chosen.voiceURI : null, rate, waited: !tapped });
    return ok();
  };

  // -------------------------------------------------------------- camera
  // A stream opens without the camera; start asks the browser for it (its
  // prompt the first time) and its light comes on; stop puts it off.
  const cams = new Map();
  let nextCam = 1;
  let camRefused = false;
  const camFail = (e) => {
    const kind = e && e.name;
    if (kind === 'NotAllowedError' || kind === 'SecurityError') { camRefused = true; return { tag: 'system-denied' }; }
    if (kind === 'NotFoundError' || kind === 'OverconstrainedError') return { tag: 'device-unavailable' };
    if (kind === 'NotReadableError' || kind === 'AbortError') return { tag: 'platform', val: 'the camera could not start: another app may be using it' };
    if (kind === 'NotSupportedError') return { tag: 'unsupported', val: NO_CAMERA };
    return { tag: 'platform', val: String((e && e.message) || e) };
  };
  const camera = (id) => cams.get(String(id));
  const camHalt = (s) => {
    s.started = false;
    s.run += 1;
    stopTracks(s.stream);
    s.stream = null;
    s.track = null;
    if (s.video) { s.video.srcObject = null; s.video.remove(); s.video = null; }
    // Nothing captured while the light was on is handed out after.
    s.read = s.seq;
  };
  const camBegin = (s) => {
    const run = s.run;
    const video = { width: { ideal: s.width }, height: { ideal: s.height }, frameRate: { ideal: s.fps } };
    if (s.device) video.deviceId = { exact: s.device };
    navigator.mediaDevices.getUserMedia({ video, audio: false }).then((stream) => {
      if (!s.started || s.run !== run) { stopTracks(stream); return; }
      s.stream = stream;
      s.track = stream.getVideoTracks()[0] || null;
      const v = document.createElement('video');
      v.muted = true;
      v.playsInline = true;
      v.autoplay = true;
      v.className = 'feed';
      v.srcObject = stream;
      document.body.appendChild(v);
      v.play().catch(() => {});
      s.video = v;
      if (typeof v.requestVideoFrameCallback === 'function') {
        const tick = () => { s.seq += 1; if (s.video === v) v.requestVideoFrameCallback(tick); };
        v.requestVideoFrameCallback(tick);
      } else {
        s.seq = -1;
      }
      state.cameras += 1;
    }).catch((e) => { if (s.run === run) s.error = camFail(e); });
  };
  const camHandlers = {
    'camera.devices': async () => {
      if (!navigator.mediaDevices) return no('unsupported', NO_CAMERA);
      if (camRefused || (await permission('camera')) === 'denied') return no('system-denied');
      // Before the person has allowed the camera a browser names none of
      // them: an empty id is the default camera, which open takes.
      return ok((await inputs('videoinput')).map((d, i) => ({ id: d.deviceId, label: d.label || `Camera ${i + 1}` })));
    },
    'camera.open': async ({ device, width, height, fps }) => {
      if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== 'function') return no('unsupported', NO_CAMERA);
      if (cams.size >= 4) return no('platform', '4 camera streams are already open');
      if (camRefused || (await permission('camera')) === 'denied') return no('system-denied');
      const list = await inputs('videoinput');
      if (!list.length) return no('device-unavailable');
      if (device && list.some((d) => d.deviceId) && !list.some((d) => d.deviceId === device)) return no('device-unavailable');
      const id = nextCam++;
      cams.set(String(id), { device, width, height, fps, started: false, run: 0, seq: 0, read: 0, stream: null, track: null, video: null, error: null, t0: null, canvas: null });
      return ok(id);
    },
    'camera.info': ({ id }) => {
      const s = camera(id);
      if (!s) return no('invalid-stream');
      const v = s.video;
      const settings = s.track && s.track.getSettings ? s.track.getSettings() : {};
      // What the camera really sends once it has sent it; until then, what
      // was asked for.
      const width = (v && v.videoWidth) || s.width;
      const height = (v && v.videoHeight) || s.height;
      const fps = Math.max(1, Math.round(settings.frameRate || s.fps));
      return ok({ width, height, fps });
    },
    'camera.start': ({ id }) => {
      const s = camera(id);
      if (!s) return no('invalid-stream');
      if (s.started) return ok();
      s.started = true;
      s.error = null;
      if (s.t0 === null) s.t0 = performance.now();
      camBegin(s);
      return ok();
    },
    'camera.stop': ({ id }) => {
      const s = camera(id);
      if (!s) return no('invalid-stream');
      if (s.started) camHalt(s);
      return ok();
    },
    'camera.read': ({ id }) => {
      const s = camera(id);
      if (!s) return no('invalid-stream');
      if (s.error) return { json: { err: s.error } };
      if (!s.started || !s.video) return ok(null);
      const v = s.video;
      const width = v.videoWidth;
      const height = v.videoHeight;
      if (!width || !height || v.readyState < 2) return ok(null);
      // The newest picture, once: nothing new since the last read is none.
      // The frame callback says when a picture is new; until it has fired
      // once, the video's clock does. Linux WebKit never fires it for this
      // hidden video, so waiting on it alone read no frame at all there.
      if (s.seq > 0 ? s.seq === s.read : v.currentTime === s.lastTime) return ok(null);
      if (!s.canvas) s.canvas = document.createElement('canvas');
      if (s.canvas.width !== width || s.canvas.height !== height) { s.canvas.width = width; s.canvas.height = height; s.g = null; }
      if (!s.g) s.g = s.canvas.getContext('2d', { willReadFrequently: true });
      s.g.drawImage(v, 0, 0, width, height);
      const pixels = s.g.getImageData(0, 0, width, height).data;
      s.read = s.seq;
      s.lastTime = v.currentTime;
      return { json: { ok: { width, height, elapsed: Math.round(performance.now() - s.t0) } }, body: new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength) };
    },
    'camera.close': ({ id }) => {
      const s = camera(id);
      if (!s) return no('invalid-stream');
      camHalt(s);
      cams.delete(String(id));
      return ok();
    },
  };

  // ---------------------------------------------------------- microphone
  // As the camera: open without the microphone, start asks the browser
  // for it, and what it hears goes to the app's ring through an
  // AudioWorklet (audio-worklet.js krate-listen) -- hosts.js reads it there.
  const mics = new Map();
  let nextMic = 1;
  let micRefused = false;
  let listening = null;
  const context = () => {
    if (!listening) {
      const ctx = new AudioContext({ latencyHint: 'interactive' });
      listening = { ctx, ready: ctx.audioWorklet.addModule('/player/audio-worklet.js') };
    }
    return listening;
  };
  const micHalt = (m) => {
    m.started = false;
    m.run += 1;
    stopTracks(m.stream);
    m.stream = null;
    try { if (m.source) m.source.disconnect(); if (m.node) m.node.disconnect(); } catch { /* already gone */ }
    m.source = null;
    m.node = null;
  };
  const micBegin = (m) => {
    const run = m.run;
    // The microphone as it is, as on desktop: no echo cancelling, noise
    // removal or level riding, which would change what a tuner or a
    // recorder hears.
    navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false }, video: false }).then(async (stream) => {
      if (!m.started || m.run !== run) { stopTracks(stream); return; }
      m.stream = stream;
      const { ctx, ready } = context();
      await ready;
      if (!m.started || m.run !== run) return;
      const track = stream.getAudioTracks()[0];
      const settings = track && track.getSettings ? track.getSettings() : {};
      const channels = Math.min(2, Math.max(1, settings.channelCount || 1));
      Atomics.store(m.ctrl, MIC_CHANNELS, channels);
      Atomics.store(m.ctrl, MIC_RATE, ctx.sampleRate);
      m.source = ctx.createMediaStreamSource(stream);
      m.node = new AudioWorkletNode(ctx, 'krate-listen', {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        channelCount: channels,
        channelCountMode: 'explicit',
        channelInterpretation: 'speakers',
        processorOptions: { ring: m.ring, ctrl: m.ctrl.buffer, channels },
      });
      m.source.connect(m.node);
      // Pulled by the context so it runs; it writes nothing out.
      m.node.connect(ctx.destination);
      if (ctx.state !== 'running') ctx.resume().catch(() => {});
      state.microphones += 1;
    }).catch((e) => {
      if (m.run !== run) return;
      const kind = e && e.name;
      let code = 3;
      if (kind === 'NotAllowedError' || kind === 'SecurityError') { micRefused = true; code = 1; } else if (kind === 'NotFoundError' || kind === 'OverconstrainedError') code = 2;
      else if (kind === 'NotSupportedError') code = 4;
      Atomics.store(m.ctrl, MIC_FAILED, code);
    });
  };
  const microphone = (id) => mics.get(String(id));
  const micHandlers = {
    'mic.open': async ({ ring, ctrl }) => {
      if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== 'function' || typeof AudioWorkletNode === 'undefined') return no('unsupported', NO_MIC);
      if (micRefused || (await permission('microphone')) === 'denied') return { json: { err: MIC_FAILURES[1] } };
      if (!(await inputs('audioinput')).length) return no('device-unavailable');
      const id = nextMic++;
      mics.set(String(id), { ring, ctrl: new Int32Array(ctrl), started: false, run: 0, stream: null, source: null, node: null });
      return ok(id);
    },
    'mic.start': ({ id }) => {
      const m = microphone(id);
      if (!m) return no('invalid-stream');
      if (m.started) return ok();
      m.started = true;
      Atomics.store(m.ctrl, MIC_FAILED, 0);
      micBegin(m);
      return ok();
    },
    'mic.stop': ({ id }) => {
      const m = microphone(id);
      if (!m) return no('invalid-stream');
      if (m.started) micHalt(m);
      return ok();
    },
  };

  return {
    facts: { geolocation: Boolean(navigator.geolocation) },
    handlers: {
      'notify.show': notifyShow,
      'location.current': locationNow,
      share: shareNow,
      'speech.voices': voices,
      'speech.say': say,
      'speech.stop': () => { if (synth) synth.cancel(); waiting = null; return ok(); },
      'speech.speaking': () => ok(Boolean(synth && (synth.speaking || synth.pending))),
      ...camHandlers,
      ...micHandlers,
    },
    /// The app ended: its camera, microphone, location and voice with it.
    close() {
      for (const s of cams.values()) camHalt(s);
      for (const m of mics.values()) micHalt(m);
      if (geo && geo.watch !== undefined) navigator.geolocation.clearWatch(geo.watch);
      if (synth) synth.cancel();
      waiting = null;
      if (listening) listening.ctx.close().catch(() => {});
    },
  };
}
