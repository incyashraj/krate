// What the browser engine in front of a proof can do, asked of the engine
// itself in the page -- never guessed from the operating system's name.
//
// The browser-tab proofs were written on a Mac and an iPhone; CI runs them
// on Linux, where the same engines are built with less: Chromium has no
// speech voices, WebKit no Web Share and a fake camera that sends no
// frame. Where an engine has an ability, its check is as strict as ever.
// Where it has not, the proof checks the app's honest refusal instead,
// prints "SKIP <tag> <check>: <engine> has no <ability>", and its last
// line names every ability left unproven and in which engine.

// run.html's viewport meta names interactive-widget=resizes-content, which
// Chromium uses so the phone keyboard resizes the page; the Player keeps
// it. WebKit on Linux does not know the key and logs this line about our
// markup -- not a fault in the app or the Player. It is matched by its
// exact text only, so any other console error, viewport ones included,
// still fails the proof.
export const VIEWPORT_WARNING = 'Viewport argument key "interactive-widget" not recognized and ignored.';

/// A console message the "no console errors" checks count.
export const consoleError = (m) => m.type() === 'error' && m.text() !== VIEWPORT_WARNING;

/// Collects what was not proven, and writes the proof's last line.
export function unproven() {
  const missing = [];
  return {
    skip(tag, what, engine, ability) {
      missing.push({ engine, ability });
      console.log(`SKIP ${tag} ${what}: ${engine} has no ${ability}`);
    },
    /// The last line; returns the exit code (1 on any FAIL, else 0).
    end(failed) {
      const by = new Map();
      for (const { engine, ability } of missing) {
        if (!by.has(ability)) by.set(ability, new Set());
        by.get(ability).add(engine);
      }
      const not = [...by].map(([ability, engines]) => `${ability} in ${[...engines].join(', ')}`).join('; ');
      console.log(`${failed ? `${failed} failed` : 'all passed'}${not ? ` -- not proven, the engine has none: ${not}` : ''}`);
      return failed ? 1 : 0;
    },
  };
}

// --------------------------------------------- probes, run in the page

/// The speech voices the engine offers, once it has had time to load them.
export const voiceProbe = () => new Promise((resolve) => {
  if (typeof speechSynthesis === 'undefined') { resolve(0); return; }
  const now = speechSynthesis.getVoices();
  if (now.length) { resolve(now.length); return; }
  const done = () => resolve(speechSynthesis.getVoices().length);
  speechSynthesis.addEventListener('voiceschanged', done, { once: true });
  setTimeout(done, 3000);
});

/// Whether the engine has Web Share, as the Player asks it.
export const shareProbe = () => typeof navigator.share === 'function';

/// Whether the engine's camera sends a frame at all: the camera opened
/// straight through getUserMedia, outside any app, into a video element,
/// for up to ten seconds; then put away.
export const cameraProbe = () => new Promise((resolve) => {
  if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== 'function') { resolve({ frame: false, why: 'no getUserMedia' }); return; }
  navigator.mediaDevices.getUserMedia({ video: { width: { ideal: 320 }, height: { ideal: 240 } }, audio: false }).then((stream) => {
    const v = document.createElement('video');
    v.muted = true;
    v.playsInline = true;
    v.autoplay = true;
    v.srcObject = stream;
    document.body.appendChild(v);
    v.play().catch(() => {});
    const end = Date.now() + 10000;
    const look = () => {
      const frame = v.readyState >= 2 && v.videoWidth > 0;
      if (frame || Date.now() > end) {
        const got = { frame, why: frame ? `${v.videoWidth}x${v.videoHeight}` : 'no frame in 10 s' };
        for (const t of stream.getTracks()) t.stop();
        v.srcObject = null;
        v.remove();
        resolve(got);
        return;
      }
      setTimeout(look, 100);
    };
    look();
  }, (e) => resolve({ frame: false, why: `getUserMedia: ${e && e.name}` }));
});
