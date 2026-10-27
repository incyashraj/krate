/* Sound in a browser tab (K-1134), the parts that decide something, in Node:
 *
 * - the worker's krate:audio (crates/adapter-web/player/web/audio.js): the
 *   app's PCM converted as the desktop converts it, backpressure on a full
 *   ring, `queued` in the app's own frames, the desktop's config refusals,
 *   permission and stream errors; before anyone has tapped, a started
 *   stream drains at a speaker's pace and a played sound is dropped;
 * - the audio thread (audio-worklet.js) fed by it: streams drained, two
 *   plays of one sound heard together, additive and clamped, stop-sound
 *   silencing every copy, the 64-voice bound;
 * - the hub serves both files with the Player's isolation headers.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/player-audio.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { workerAudio, makeAudioMemory, RATE, FRAMES, QUANTA, MAX_COPIES, MAX_VOICES_SEEN, CLAMPED, PLAYED, VOICES } from "../../../crates/adapter-web/player/web/audio.js";
import { playerAsset } from "../src/web-player.js";

class Fail extends Error {
  constructor(payload) { super(JSON.stringify(payload)); this.payload = payload; }
}

function setup({ allowed = true } = {}) {
  const memory = makeAudioMemory();
  const posts = [];
  const api = workerAudio({
    allows: (cap) => allowed && cap === "audio.playback",
    fail: (payload) => new Fail(payload),
    post: (msg) => posts.push(msg),
    memory,
  });
  return { memory, posts, play: api["krate:audio/playback"], queue: api["krate:audio/playback-queue"] };
}

const s16 = (values) => {
  const out = new Uint8Array(values.length * 2);
  const dv = new DataView(out.buffer);
  values.forEach((v, i) => dv.setInt16(i * 2, Math.round(v * 32767), true));
  return out;
};
const f32 = (values) => new Uint8Array(new Float32Array(values).buffer);
const config = (over = {}) => ({ sampleRate: 48000, channels: 1, format: "pcm-s16", bufferFrames: 1024, ...over });
const thrown = (fn) => { try { fn(); } catch (e) { return e.payload; } return null; };

// The audio thread, loaded as a browser loads it: registerProcessor and the
// processor base class are the AudioWorkletGlobalScope's.
let Mix;
globalThis.AudioWorkletProcessor = class { constructor() { this.port = { onmessage: null }; } };
globalThis.registerProcessor = (name, cls) => { if (name === "krate-mix") Mix = cls; };
await import("../../../crates/adapter-web/player/web/audio-worklet.js");

function thread(memory, posts) {
  const mix = new Mix({ processorOptions: { stats: memory.stats, commands: memory.commands, frames: FRAMES, slots: 256 } });
  // What the page forwards from the worker.
  const deliver = () => { for (const m of posts.splice(0)) mix.port.onmessage({ data: m }); };
  const block = () => {
    const out = [[new Float32Array(128), new Float32Array(128)]];
    mix.process([], out);
    return out[0];
  };
  return { mix, deliver, block };
}

test("write converts the app's PCM to 48 kHz stereo as the desktop does, and queued counts the app's frames", () => {
  const { play, queue, posts } = setup();
  const id = play.open(config({ sampleRate: 24000 }));
  assert.strictEqual(typeof id, "bigint");
  // Not started, so nothing drains while this looks.
  // 24 kHz mono: each frame is two 48 kHz frames, the same value both sides.
  const taken = play.write(id, s16([0.5, -0.5, 0.25]));
  assert.strictEqual(taken, 6);
  const { ring, ctrl } = posts.find((p) => p.op === "stream");
  const r = new Float32Array(ring);
  const near = (a, b) => Math.abs(a - b) < 1e-4;
  assert.ok([0.5, 0.5, 0.5, 0.5, -0.5, -0.5, -0.5, -0.5, 0.25, 0.25].every((v, i) => near(r[i], v)), Array.from(r.slice(0, 12)).join());
  assert.strictEqual(new Int32Array(ctrl)[0], 6);
  // 6 device frames at 48 kHz are 3 of the app's at 24 kHz.
  assert.strictEqual(queue.queued(id), 3);
  // A partial frame is left for the app to send again.
  assert.strictEqual(play.write(id, new Uint8Array(1)), 0);
  // float32 stereo, NaN heard as silence rather than poisoning the mix.
  const st = play.open(config({ channels: 2, format: "float32" }));
  assert.strictEqual(play.write(st, f32([0.1, 0.2, NaN, 0.4])), 16);
  const r2 = new Float32Array(posts.filter((p) => p.op === "stream")[1].ring);
  assert.deepStrictEqual(Array.from(r2.slice(0, 4)).map((v) => Math.round(v * 10) / 10), [0.1, 0.2, 0, 0.4]);
});

test("a full ring takes fewer bytes instead of dropping sound", () => {
  const { play } = setup();
  const id = play.open(config());
  // Not started: nothing drains, so the ring fills.
  const chunk = s16(new Array(48000).fill(0.1));
  let total = 0;
  for (let i = 0; i < 12; i++) total += play.write(id, chunk);
  assert.strictEqual(total, FRAMES * 2, "exactly the ring's frames were taken");
  assert.strictEqual(play.write(id, chunk), 0);
});

test("the desktop's refusals: config, permission, unknown stream, unknown sound", () => {
  const { play, queue } = setup();
  assert.deepStrictEqual(thrown(() => play.open(config({ sampleRate: 4000 }))), { tag: "unsupported", val: "sample rate must be between 8000 and 192000 Hz" });
  assert.deepStrictEqual(thrown(() => play.open(config({ channels: 0 }))), { tag: "unsupported", val: "channels must be between 1 and 8" });
  assert.deepStrictEqual(thrown(() => play.open(config({ bufferFrames: 0 }))), { tag: "unsupported", val: "buffer-frames must be greater than zero" });
  assert.deepStrictEqual(thrown(() => play.start(99n)), { tag: "invalid-stream" });
  assert.deepStrictEqual(thrown(() => queue.queued(99n)), { tag: "invalid-stream" });
  const id = play.open(config());
  assert.deepStrictEqual(thrown(() => play.loadSound(id, new Uint8Array(1))), { tag: "unsupported", val: "a sound needs at least one whole frame of samples" });
  assert.deepStrictEqual(thrown(() => play.playSound(id, 1n, 1)), { tag: "invalid-stream" });
  const denied = setup({ allowed: false });
  assert.deepStrictEqual(thrown(() => denied.play.open(config())), { tag: "permission-denied" });
});

test("before anyone taps, a started stream drains at a speaker's pace and a played sound is dropped", async () => {
  const { play, queue, memory } = setup();
  const id = play.open(config());
  play.start(id);
  play.write(id, s16(new Array(24000).fill(0.2)));
  assert.ok(queue.queued(id) > 23500);
  const sound = play.loadSound(id, s16([0.5, 0.5]));
  play.playSound(id, sound, 1);
  assert.strictEqual(Atomics.load(new Int32Array(memory.commands), 0), 0, "no play reaches the audio thread before it runs");
  await new Promise((r) => setTimeout(r, 120));
  const left = queue.queued(id);
  // ~120 ms at 48 kHz is ~5760 frames gone; allow for the timer's slack.
  assert.ok(left < 24000 - 4000 && left > 24000 - 12000, `queued ${left}`);
  // A stopped stream does not drain.
  play.stop(id);
  const held = queue.queued(id);
  await new Promise((r) => setTimeout(r, 60));
  assert.strictEqual(queue.queued(id), held);
});

test("the audio thread drains streams, mixes two copies of one sound, clamps, and stops every copy", () => {
  const { play, queue, memory, posts } = setup();
  const t = thread(memory, posts);
  const id = play.open(config());
  play.start(id);
  const shot = play.loadSound(id, s16(new Array(1000).fill(0.4)));
  play.write(id, s16(new Array(256).fill(0.1)));
  t.deliver();
  // The first block is what tells the worker the sound is running.
  let out = t.block();
  assert.ok(Math.abs(out[0][0] - 0.1) < 1e-3 && Math.abs(out[1][127] - 0.1) < 1e-3, `stream ${out[0][0]}`);
  assert.strictEqual(queue.queued(id), 128);

  // The same sound twice: two copies at once, added.
  play.playSound(id, shot, 1);
  play.playSound(id, shot, 1);
  out = t.block();
  assert.ok(Math.abs(out[0][0] - 0.9) < 1e-3, `0.1 + 0.4 + 0.4 = ${out[0][0]}`);
  const stats = new Int32Array(memory.stats);
  assert.strictEqual(stats[MAX_COPIES], 2);
  assert.strictEqual(stats[PLAYED], 2);

  // A third loud copy goes past 1: held at 1, not wrapped into noise.
  play.playSound(id, shot, 2);
  out = t.block();
  assert.strictEqual(out[0][0], 1);
  assert.ok(stats[CLAMPED] > 0);
  assert.strictEqual(stats[MAX_VOICES_SEEN], 3);

  // stop-sound silences every copy.
  play.stopSound(id, shot);
  out = t.block();
  assert.strictEqual(out[0][0], 0);
  assert.strictEqual(stats[VOICES], 0);
  assert.ok(stats[QUANTA] >= 4);
});

test("a sound played the moment it is loaded is heard once it arrives; at most 64 sound at once", () => {
  const { play, memory, posts } = setup();
  const t = thread(memory, posts);
  const id = play.open(config());
  play.start(id);
  t.deliver();
  t.block();
  const beep = play.loadSound(id, s16(new Array(4800).fill(0.001)));
  // Played before the page has forwarded the samples.
  play.playSound(id, beep, 1);
  let out = t.block();
  assert.strictEqual(out[0][0], 0);
  t.deliver();
  out = t.block();
  assert.ok(Math.abs(out[0][0] - 0.001) < 1e-4, `${out[0][0]}`);
  for (let i = 0; i < 100; i++) play.playSound(id, beep, 1);
  t.block();
  assert.strictEqual(new Int32Array(memory.stats)[VOICES], 64);
});

test("the hub serves the audio files with the Player's isolation", async () => {
  const env = { ASSETS: { fetch: async (req) => new Response(`// ${new URL(req.url).pathname}`) } };
  for (const name of ["audio.js", "audio-worklet.js"]) {
    const url = new URL(`https://hub.example/player/${name}`);
    const res = await playerAsset(new Request(url), url, env);
    assert.strictEqual(res.status, 200, name);
    assert.match(res.headers.get("content-type"), /text\/javascript/);
    assert.strictEqual(res.headers.get("cross-origin-embedder-policy"), "require-corp");
  }
  assert.strictEqual(RATE, 48000);
});
