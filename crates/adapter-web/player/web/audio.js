// Sound out of a Krate app in a browser tab (K-1134): krate:audio playback
// and playback-queue, over Web Audio.
//
// The app runs in a Web Worker and blocks there, so its audio cannot be a
// Promise or a message it waits on. It moves through shared memory instead:
//
//   worker (this file's workerAudio, inside hosts.js)
//     write      converts the app's PCM to 48 kHz stereo floats, the way the
//                desktop converts to its device (nearest neighbour, mono to
//                both sides), into the stream's ring; full ring, fewer bytes
//                taken -- backpressure, as on desktop, never loss
//     load-sound converts once and hands the samples to the page
//     play-sound a record on the command ring, read every 128 frames
//   page (pageAudio, in player.js)
//     makes the AudioContext and the AudioWorklet inside the person's first
//     tap or key, which is when a browser lets a page make a sound
//   audio thread (audio-worklet.js)
//     drains every started stream, mixes the playing sounds on top,
//     additively, clamps, and counts what it rendered
//
// Until the person taps there is nothing to hear and the app still runs: a
// started stream is drained at the speed a speaker would play it (so
// `queued` and a writer that waits on it behave the same before and after),
// and a sound played then is dropped rather than heard late.

/// The rate and layout every stream is converted to, the context's own.
export const RATE = 48000;
/// Frames a stream's ring holds: about 10.9 seconds, the desktop's ten
/// seconds rounded up to a power of two so the counters can wrap.
export const FRAMES = 1 << 19;
/// Command ring records (play, stop), four Int32 each.
export const SLOTS = 256;
/// The most streams one app may hold open at once.
export const MAX_STREAMS = 8;

// A stream's control words.
export const WRITE = 0;
export const READ = 1;
export const STARTED = 2;
// The shared stats the audio thread keeps.
export const QUANTA = 0; // 128-frame blocks rendered
export const LOUD = 1; // ... of which carried sound
export const VOICES = 2; // sounds playing now
export const MAX_VOICES_SEEN = 3; // most sounds playing at once
export const MAX_COPIES = 4; // most copies of one sound playing at once
export const PEAK = 5; // loudest sample before the clamp, x1000
export const CLAMPED = 6; // samples the clamp held at 1
export const PLAYED = 7; // play-sound records taken
export const STAT_WORDS = 8;

const MIN_RATE = 8000;
const MAX_RATE = 192000;
const MAX_CHANNELS = 8;
/// A rendered block more recent than this means the audio is running.
const ALIVE_MS = 100;

export const PLAY = 1;
export const STOP = 2;

/// The shared memory one run's audio uses: the stats and the command ring.
export function makeAudioMemory() {
  return {
    stats: new SharedArrayBuffer(STAT_WORDS * 4),
    commands: new SharedArrayBuffer((2 + SLOTS * 4) * 4),
  };
}

const sampleOf = (dv, at, float) => {
  const v = float ? dv.getFloat32(at, true) : dv.getInt16(at, true) / 32767;
  return Number.isFinite(v) ? v : 0;
};

/// The worker's half: the three interfaces' functions in jco's shapes.
///
/// ctx: { allows(cap), fail(payload) -> Error, post(msg, transfer),
///        memory: makeAudioMemory()'s buffers }
export function workerAudio({ allows, fail, post, memory }) {
  const stats = new Int32Array(memory.stats);
  const cmd = new Int32Array(memory.commands);
  const bits = new Float32Array(1);
  const bitsInt = new Int32Array(bits.buffer);
  const streams = new Map();
  let nextId = 1;
  // No block rendered yet is not running: the count must move first.
  let seen = 0;
  let seenAt = -Infinity;

  const allowed = () => {
    if (!allows('audio.playback')) throw fail({ tag: 'permission-denied' });
  };
  const get = (id) => {
    const s = streams.get(String(id));
    if (!s) throw fail({ tag: 'invalid-stream' });
    return s;
  };
  /// Whether the audio thread rendered a block in the last ALIVE_MS.
  const alive = () => {
    const q = Atomics.load(stats, QUANTA);
    const t = performance.now();
    if (q !== seen) { seen = q; seenAt = t; }
    return t - seenAt < ALIVE_MS;
  };
  /// Before the first tap (or while the browser holds the sound back), a
  /// started stream drains at the speaker's pace with nobody to hear it.
  const idle = (s) => {
    const t = performance.now();
    if (alive() || !Atomics.load(s.ctrl, STARTED)) { s.idleAt = t; return; }
    const frames = Math.floor(((t - s.idleAt) * RATE) / 1000);
    if (frames <= 0) return;
    s.idleAt += (frames * 1000) / RATE;
    const w = Atomics.load(s.ctrl, WRITE);
    const r = Atomics.load(s.ctrl, READ);
    Atomics.compareExchange(s.ctrl, READ, r, (r + Math.min(frames, (w - r) | 0)) | 0);
  };
  const command = (op, stream, sound, gain) => {
    const head = Atomics.load(cmd, 0);
    if (((head - Atomics.load(cmd, 1)) | 0) >= SLOTS) return;
    const o = 2 + (head & (SLOTS - 1)) * 4;
    bits[0] = gain;
    cmd[o] = op;
    cmd[o + 1] = stream;
    cmd[o + 2] = sound;
    cmd[o + 3] = bitsInt[0];
    Atomics.store(cmd, 0, (head + 1) | 0);
  };
  /// The app's whole frames as 48 kHz stereo, for a stored sound.
  const decode = (s, bytes) => {
    const fb = s.width * s.channels;
    const whole = Math.floor(bytes.length / fb);
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const out = new Float32Array(Math.ceil((whole * RATE) / s.rate + 1) * 2);
    let n = 0;
    let acc = 0;
    for (let i = 0; i < whole; i++) {
      const at = i * fb;
      const left = sampleOf(dv, at, s.float);
      const right = s.channels > 1 ? sampleOf(dv, at + s.width, s.float) : left;
      acc += RATE;
      while (acc >= s.rate) {
        acc -= s.rate;
        out[n++] = left;
        out[n++] = right;
      }
    }
    return out.slice(0, n);
  };

  const playback = {
    open(config) {
      allowed();
      const { sampleRate, channels, format, bufferFrames } = config;
      if (!(sampleRate >= MIN_RATE && sampleRate <= MAX_RATE)) {
        throw fail({ tag: 'unsupported', val: `sample rate must be between ${MIN_RATE} and ${MAX_RATE} Hz` });
      }
      if (!(channels >= 1 && channels <= MAX_CHANNELS)) {
        throw fail({ tag: 'unsupported', val: `channels must be between 1 and ${MAX_CHANNELS}` });
      }
      if (!(bufferFrames > 0)) throw fail({ tag: 'unsupported', val: 'buffer-frames must be greater than zero' });
      if (streams.size >= MAX_STREAMS) {
        throw fail({ tag: 'platform', val: `a browser tab plays at most ${MAX_STREAMS} streams at once` });
      }
      const id = nextId++;
      const ring = new SharedArrayBuffer(FRAMES * 2 * 4);
      const ctrl = new SharedArrayBuffer(16);
      const float = format === 'float32';
      streams.set(String(id), {
        id, rate: sampleRate, channels, float, width: float ? 4 : 2,
        ring: new Float32Array(ring), ctrl: new Int32Array(ctrl),
        acc: 0, sounds: 0, idleAt: performance.now(),
      });
      post({ op: 'stream', id, ring, ctrl });
      return BigInt(id);
    },
    start(id) {
      allowed();
      const s = get(id);
      s.idleAt = performance.now();
      Atomics.store(s.ctrl, STARTED, 1);
    },
    stop(id) {
      allowed();
      const s = get(id);
      idle(s);
      Atomics.store(s.ctrl, STARTED, 0);
    },
    write(id, bytes) {
      allowed();
      const s = get(id);
      idle(s);
      const fb = s.width * s.channels;
      const whole = Math.floor(bytes.length / fb);
      if (!whole) return 0;
      const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      let w = Atomics.load(s.ctrl, WRITE);
      let room = FRAMES - ((w - Atomics.load(s.ctrl, READ)) | 0);
      let taken = 0;
      for (let i = 0; i < whole; i++) {
        const at = i * fb;
        // How many 48 kHz frames this one becomes; if they do not fit, stop
        // before it so the app writes it again, exactly as on desktop.
        let acc = s.acc + RATE;
        let n = 0;
        while (acc >= s.rate) { acc -= s.rate; n++; }
        if (n > room) break;
        s.acc = acc;
        const left = sampleOf(dv, at, s.float);
        const right = s.channels > 1 ? sampleOf(dv, at + s.width, s.float) : left;
        for (let k = 0; k < n; k++) {
          const o = (w & (FRAMES - 1)) * 2;
          s.ring[o] = left;
          s.ring[o + 1] = right;
          w = (w + 1) | 0;
        }
        room -= n;
        taken++;
      }
      // Published after the samples, so the audio thread never reads a
      // frame before it is written.
      Atomics.store(s.ctrl, WRITE, w);
      return taken * fb;
    },
    loadSound(id, bytes) {
      allowed();
      const s = get(id);
      const samples = decode(s, bytes);
      if (!samples.length) throw fail({ tag: 'unsupported', val: 'a sound needs at least one whole frame of samples' });
      s.sounds += 1;
      post({ op: 'sound', stream: s.id, sound: s.sounds, samples }, [samples.buffer]);
      return BigInt(s.sounds);
    },
    playSound(id, sound, gain) {
      allowed();
      const s = get(id);
      const n = Number(sound);
      if (!(n >= 1 && n <= s.sounds)) throw fail({ tag: 'invalid-stream' });
      // Nobody can hear it yet: dropped, not heard late after the tap.
      if (!alive()) return;
      command(PLAY, s.id, n, Number.isFinite(gain) ? Math.min(4, Math.max(0, gain)) : 1);
    },
    stopSound(id, sound) {
      allowed();
      const s = get(id);
      command(STOP, s.id, Number(sound), 0);
    },
  };
  const queue = {
    queued(id) {
      allowed();
      const s = get(id);
      idle(s);
      const frames = (Atomics.load(s.ctrl, WRITE) - Atomics.load(s.ctrl, READ)) | 0;
      return Math.floor((frames * s.rate) / RATE);
    },
  };
  return {
    'krate:audio/types': {},
    'krate:audio/playback': playback,
    'krate:audio/playback-queue': queue,
  };
}

/// The page's half: the streams and sounds the worker made, and the
/// AudioContext, made inside the person's first tap or key once the app
/// has opened a stream. Anything that fails here leaves the app running,
/// silent.
export function pageAudio({ memory, probe, report }) {
  const state = (report.audio = { state: 'none', streams: 0, sounds: 0 });
  const streams = [];
  const waiting = [];
  let ctx = null;
  let node = null;
  let analyser = null;

  const build = async () => {
    try {
      ctx = new AudioContext({ sampleRate: RATE, latencyHint: 'interactive' });
    } catch (e) {
      state.error = `no AudioContext: ${(e && e.message) || e}`;
      return;
    }
    // Inside the tap: a context a browser lets start.
    ctx.resume().catch(() => {});
    state.state = ctx.state;
    ctx.onstatechange = () => { state.state = ctx.state; };
    try {
      await ctx.audioWorklet.addModule('/player/audio-worklet.js');
      node = new AudioWorkletNode(ctx, 'krate-mix', {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [2],
        processorOptions: { stats: memory.stats, commands: memory.commands, frames: FRAMES, slots: SLOTS },
      });
    } catch (e) {
      state.error = `no audio thread: ${(e && e.message) || e}`;
      return;
    }
    for (const s of streams) node.port.postMessage(s);
    for (const s of waiting.splice(0)) node.port.postMessage(s, [s.samples.buffer]);
    if (probe) {
      analyser = ctx.createAnalyser();
      analyser.fftSize = 2048;
      node.connect(analyser);
      analyser.connect(ctx.destination);
    } else {
      node.connect(ctx.destination);
    }
    state.worklet = true;
  };
  const unlock = () => {
    if (!streams.length) return;
    if (!ctx) { build(); return; }
    if (ctx.state !== 'running' && ctx.state !== 'closed') ctx.resume().catch(() => {});
  };
  // Where a browser says sound may start on its own (Firefox asks this
  // way), there is no need to wait for a tap.
  const allowedNow = () => {
    try { return navigator.getAutoplayPolicy && navigator.getAutoplayPolicy('audiocontext') === 'allowed'; } catch { return false; }
  };
  for (const type of ['pointerdown', 'pointerup', 'touchend', 'keydown', 'click']) {
    document.addEventListener(type, unlock, { capture: true, passive: true });
  }

  if (probe) {
    // For the proofs: what the audio thread rendered, and what reached the
    // graph's output, read off the page.
    window.__krateAudio = {
      stats: () => Array.from(new Int32Array(memory.stats)),
      // Each stream's [written, played] frame counters.
      streams: () => streams.map((s) => { const c = new Int32Array(s.ctrl); return [Atomics.load(c, WRITE), Atomics.load(c, READ)]; }),
      level() {
        if (!analyser) return null;
        const buf = new Float32Array(analyser.fftSize);
        analyser.getFloatTimeDomainData(buf);
        let sum = 0;
        for (const v of buf) sum += v * v;
        return Math.sqrt(sum / buf.length);
      },
    };
  }

  return {
    onMessage(msg) {
      if (msg.op === 'stream') {
        streams.push(msg);
        state.streams = streams.length;
        if (node) node.port.postMessage(msg);
        // A browser that already counts a tap on this page (the consent
        // card's Allow) lets the sound start now; otherwise the next tap.
        else if ((navigator.userActivation && navigator.userActivation.hasBeenActive) || allowedNow()) unlock();
      } else if (msg.op === 'sound') {
        state.sounds += 1;
        if (node) node.port.postMessage(msg, [msg.samples.buffer]);
        else waiting.push(msg);
      }
    },
    close() {
      if (ctx && ctx.state !== 'closed') ctx.close().catch(() => {});
    },
  };
}
