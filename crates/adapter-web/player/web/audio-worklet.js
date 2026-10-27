// The audio thread of a Krate app in a browser tab (K-1134).
//
// Every 128 frames: take the play and stop records the app's worker left on
// the command ring, drain each started stream's ring, mix the playing sounds
// on top -- additively, then clamped, so a dozen at once distort rather than
// wrap into noise, as krate:audio documents -- and count what was rendered.
// The layout (ring sizes, control words, stats) is audio.js's; it arrives
// in processorOptions so the two cannot disagree.

const WRITE = 0;
const READ = 1;
const STARTED = 2;
const QUANTA = 0;
const LOUD = 1;
const VOICES = 2;
const MAX_VOICES_SEEN = 3;
const MAX_COPIES = 4;
const PEAK = 5;
const CLAMPED = 6;
const PLAYED = 7;
const PLAY = 1;
const STOP = 2;
/// Most sounds at once, the desktop's bound: past it a mix is noise, and an
/// app calling play-sound in a loop must not grow the list without end.
const MAX_VOICES = 64;
/// A play whose sound is still on its way from the page waits this many
/// blocks (about a quarter of a second), then is dropped.
const WAIT_BLOCKS = 94;

class KrateMix extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = options.processorOptions;
    this.stats = new Int32Array(o.stats);
    this.cmd = new Int32Array(o.commands);
    this.mask = o.frames - 1;
    this.slots = o.slots;
    this.bits = new Int32Array(1);
    this.gainOf = new Float32Array(this.bits.buffer);
    this.streams = new Map();
    this.sounds = new Map();
    this.voices = [];
    this.pending = [];
    this.port.onmessage = ({ data }) => {
      if (data.op === 'stream') {
        this.streams.set(data.id, { ring: new Float32Array(data.ring), ctrl: new Int32Array(data.ctrl) });
      } else if (data.op === 'sound') {
        this.sounds.set(`${data.stream}:${data.sound}`, data.samples);
      }
    };
  }

  start(stream, sound, gain) {
    const samples = this.sounds.get(`${stream}:${sound}`);
    if (!samples) return false;
    if (this.voices.length >= MAX_VOICES) return true;
    let copies = 1;
    for (const v of this.voices) if (v.stream === stream && v.sound === sound) copies += 1;
    this.voices.push({ stream, sound, samples, at: 0, gain });
    if (copies > this.stats[MAX_COPIES]) this.stats[MAX_COPIES] = copies;
    return true;
  }

  commands() {
    const c = this.cmd;
    for (;;) {
      const tail = Atomics.load(c, 1);
      if (tail === Atomics.load(c, 0)) break;
      const o = 2 + (tail & (this.slots - 1)) * 4;
      const op = c[o];
      const stream = c[o + 1];
      const sound = c[o + 2];
      this.bits[0] = c[o + 3];
      const gain = this.gainOf[0];
      Atomics.store(c, 1, (tail + 1) | 0);
      if (op === PLAY) {
        this.stats[PLAYED] += 1;
        if (!this.start(stream, sound, gain)) this.pending.push({ stream, sound, gain, left: WAIT_BLOCKS });
      } else if (op === STOP) {
        this.voices = this.voices.filter((v) => !(v.stream === stream && v.sound === sound));
        this.pending = this.pending.filter((p) => !(p.stream === stream && p.sound === sound));
      }
    }
    if (this.pending.length) {
      this.pending = this.pending.filter((p) => !this.start(p.stream, p.sound, p.gain) && --p.left > 0);
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const left = out[0];
    const right = out[1] || out[0];
    const n = left.length;
    this.commands();

    for (const s of this.streams.values()) {
      if (!Atomics.load(s.ctrl, STARTED)) continue;
      let r = Atomics.load(s.ctrl, READ);
      const take = Math.min(n, (Atomics.load(s.ctrl, WRITE) - r) | 0);
      for (let i = 0; i < take; i++) {
        const o = (r & this.mask) * 2;
        left[i] += s.ring[o];
        right[i] += s.ring[o + 1];
        r = (r + 1) | 0;
      }
      Atomics.store(s.ctrl, READ, r);
    }

    if (this.voices.length) {
      for (const v of this.voices) {
        const s = this.streams.get(v.stream);
        if (!s || !Atomics.load(s.ctrl, STARTED)) continue;
        const take = Math.min(n, (v.samples.length - v.at) / 2);
        for (let i = 0; i < take; i++) {
          left[i] += v.samples[v.at++] * v.gain;
          right[i] += v.samples[v.at++] * v.gain;
        }
      }
      this.voices = this.voices.filter((v) => v.at < v.samples.length);
    }

    let peak = 0;
    let clamped = 0;
    for (const ch of left === right ? [left] : [left, right]) {
      for (let i = 0; i < n; i++) {
        const v = ch[i];
        const a = v < 0 ? -v : v;
        if (a > peak) peak = a;
        if (a > 1) { ch[i] = v < 0 ? -1 : 1; clamped += 1; }
      }
    }
    const st = this.stats;
    if (peak > 1e-4) st[LOUD] += 1;
    const milli = Math.min(2147483647, Math.round(peak * 1000));
    if (milli > st[PEAK]) st[PEAK] = milli;
    st[CLAMPED] += clamped;
    st[VOICES] = this.voices.length;
    if (this.voices.length > st[MAX_VOICES_SEEN]) st[MAX_VOICES_SEEN] = this.voices.length;
    // Last: the worker reads a new count as "the sound is running".
    Atomics.add(st, QUANTA, 1);
    return true;
  }
}

registerProcessor('krate-mix', KrateMix);
