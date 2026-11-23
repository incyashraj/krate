/* Video and sound in a browser tab (krate:gfx/video, K-1348, K-1351): the
 * web Player's two halves, hosts.js in the app's worker and media.js in the
 * page.
 *
 *   node --test cloud/worker/test/player-video.test.mjs
 *
 * Pins what is judged before the page is asked (the asset path, the
 * net.connect grant, a picked folder, a volume that is not a number), that
 * a picture is kept once per canvas and drawn by handle until a newer one
 * comes, and that a clip is read out of the app's own bundle.
 */
import { test } from "node:test";
import assert from "node:assert";
import { buildImports } from "../../../crates/adapter-web/player/web/hosts.js";
import { bundleFile } from "../../../crates/adapter-web/player/web/media.js";

/// The imports for an app that may reach `hosts`, with a bridge that
/// records every call and answers `answer(op, args)`, and a canvas surface
/// that records what is kept, drawn and forgotten.
const worker = ({ hosts = [], answer = () => ({ json: { ok: null } }) } = {}) => {
  const calls = [];
  const canvas = [];
  let nextImage = 1;
  const imports = buildImports({
    meta: { refusals: {}, imports: [] },
    grants: {
      allows: () => false,
      check_http: (url) =>
        JSON.stringify(hosts.some((h) => url.startsWith(`https://${h}/`)) ? { ok: null } : { err: { tag: "permission-denied" } }),
    },
    bridge: { call: (op, args) => { calls.push({ op, args }); return answer(op, args); } },
    ui: {
      c_keep: (c, w, h, rgba) => { canvas.push(["keep", c, w, h, rgba.length]); return JSON.stringify({ ok: nextImage++ }); },
      c_draw_kept: (c, image) => { canvas.push(["draw", c, image]); },
      c_forget: (c, image) => { canvas.push(["forget", c, image]); },
    },
    kv: {}, ring: {}, output() {}, notice() {}, present() {},
  });
  return { video: imports["krate:gfx/video"], calls, canvas };
};

const refused = (f, tag) => {
  try {
    f();
  } catch (e) {
    assert.strictEqual(e.payload.tag, tag, JSON.stringify(e.payload));
    return e.payload;
  }
  assert.fail(`expected ${tag}`);
};

const AREA = { x: 0, y: 0, width: 100, height: 50 };

test("a source is judged as on a computer before the page is asked", () => {
  const w = worker({ hosts: ["media.example.com"], answer: () => ({ json: { ok: "1" } }) });
  for (const bad of ["../secret.mp4", "/etc/clip.mp4", "clips//a.mp4", "clips/./a.mp4", ""]) {
    refused(() => w.video.open(bad), "unsupported");
  }
  const picked = refused(() => w.video.open("picked/folder-1/talk.mp4"), "unsupported");
  assert.match(picked.val, /Krate on a computer/);
  refused(() => w.video.open("https://elsewhere.example.org/clip.mp4"), "permission-denied");
  assert.deepStrictEqual(w.calls, [], "nothing reached the page");
  assert.strictEqual(w.video.open("clips/intro.mp4"), 1n);
  assert.strictEqual(w.video.open("https://media.example.com/clip.mp4"), 1n);
  assert.deepStrictEqual(w.calls.map((c) => c.args.source), ["clips/intro.mp4", "https://media.example.com/clip.mp4"]);
});

test("a video the app did not open is not its to control, and a volume must be a number", () => {
  const w = worker({ answer: (op) => (op === "video.open" ? { json: { ok: "7" } } : { json: { ok: null } }) });
  refused(() => w.video.play(7n), "invalid-target");
  const v = w.video.open("clips/a.mp4");
  w.video.setVolume(v, 0.25);
  w.video.setVolume(v, 9);
  refused(() => w.video.setVolume(v, NaN), "unsupported");
  w.video.setMuted(v, true);
  w.video.seek(v, 1500n);
  w.video.setRate(v, 2);
  w.video.setRate(v, 0.1);
  refused(() => w.video.setRate(v, Infinity), "unsupported");
  const controls = w.calls.filter((c) => c.op === "video.control").map((c) => [c.args.kind, c.args.value]);
  assert.deepStrictEqual(controls, [
    ["volume", 0.25], ["volume", 1], ["muted", true], ["seek", 1500], ["rate", 2], ["rate", 0.25],
  ]);
});

test("a picture is kept once and drawn by handle until a newer one comes", () => {
  let fresh = true;
  const w = worker({
    answer: (op) => {
      if (op === "video.open") return { json: { ok: "3" } };
      if (op === "video.frame") {
        if (!fresh) return { json: { ok: null } };
        fresh = false;
        return { json: { ok: { width: 2, height: 1 } }, body: new Uint8Array(8) };
      }
      if (op === "video.state") {
        return { json: { ok: { positionMillis: 40, durationMillis: 1000, width: 2, height: 1, ready: true, playing: true, ended: false, problem: "" } } };
      }
      return { json: { ok: null } };
    },
  });
  const v = w.video.open("clips/a.mp4");
  w.video.draw(9n, v, AREA);
  w.video.draw(9n, v, AREA);
  fresh = true;
  w.video.drawRound(9n, v, AREA, { topLeft: 4, topRight: 4, bottomRight: 4, bottomLeft: 4 });
  assert.deepStrictEqual(w.canvas, [
    ["keep", 9n, 2, 1, 8], ["draw", 9n, 1n],
    ["draw", 9n, 1n],
    ["forget", 9n, 1n], ["keep", 9n, 2, 1, 8], ["draw", 9n, 2n],
  ]);
  const s = w.video.state(v);
  assert.strictEqual(s.positionMillis, 40n);
  assert.strictEqual(s.durationMillis, 1000n);
  w.video.close(v);
  assert.deepStrictEqual(w.canvas.at(-1), ["forget", 9n, 2n]);
  refused(() => w.video.draw(9n, v, AREA), "invalid-target");
});

test("floating asks the page, and a video the app did not open never floats", () => {
  const w = worker({
    answer: (op) => {
      if (op === "video.open") return { json: { ok: "4" } };
      if (op === "video.floating") return { json: { ok: true } };
      return { json: { ok: null } };
    },
  });
  assert.strictEqual(w.video.floating(4n), false);
  refused(() => w.video.startFloating(4n), "invalid-target");
  w.video.stopFloating(4n);
  assert.deepStrictEqual(w.calls, [], "nothing reached the page for a video not opened");
  const v = w.video.open("clips/a.mp4");
  w.video.startFloating(v);
  assert.strictEqual(w.video.floating(v), true);
  w.video.stopFloating(v);
  assert.deepStrictEqual(
    w.calls.filter((c) => c.op === "video.float").map((c) => c.args.on),
    [true, false],
  );
});

test("before its first picture a video draws nothing", () => {
  const w = worker({ answer: (op) => ({ json: { ok: op === "video.open" ? "1" : null } }) });
  const v = w.video.open("clips/a.mp4");
  w.video.draw(1n, v, AREA);
  assert.deepStrictEqual(w.canvas, []);
});

// --------------------------------------------------------- the page half

/// A zip with each of `files` stored (`deflate: false`) or deflated, as
/// `krate pack` writes a bundle.
async function zip(files) {
  const enc = new TextEncoder();
  const locals = [];
  const central = [];
  let offset = 0;
  for (const { name, data, deflate } of files) {
    const body = deflate
      ? new Uint8Array(await new Response(new Blob([data]).stream().pipeThrough(new CompressionStream("deflate-raw"))).arrayBuffer())
      : data;
    const n = enc.encode(name);
    const local = new Uint8Array(30 + n.length + body.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(8, deflate ? 8 : 0, true);
    lv.setUint32(18, body.length, true);
    lv.setUint32(22, data.length, true);
    lv.setUint16(26, n.length, true);
    local.set(n, 30);
    local.set(body, 30 + n.length);
    const entry = new Uint8Array(46 + n.length);
    const cv = new DataView(entry.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(10, deflate ? 8 : 0, true);
    cv.setUint32(20, body.length, true);
    cv.setUint32(24, data.length, true);
    cv.setUint16(28, n.length, true);
    cv.setUint32(42, offset, true);
    entry.set(n, 46);
    locals.push(local);
    central.push(entry);
    offset += local.length;
  }
  const dirSize = central.reduce((a, c) => a + c.length, 0);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, files.length, true);
  ev.setUint16(10, files.length, true);
  ev.setUint32(12, dirSize, true);
  ev.setUint32(16, offset, true);
  const out = new Uint8Array(offset + dirSize + 22);
  let at = 0;
  for (const part of [...locals, ...central, end]) { out.set(part, at); at += part.length; }
  return out;
}

test("a bundled clip is read out of the app's own bundle, stored or deflated", async () => {
  const clip = new Uint8Array(5000).map((_, i) => (i * 7) % 251);
  const bundle = await zip([
    { name: "manifest.toml", data: new TextEncoder().encode("[app]"), deflate: true },
    { name: "assets/clips/intro.mp4", data: clip, deflate: true },
    { name: "assets/tone.wav", data: new Uint8Array([1, 2, 3]), deflate: false },
  ]);
  assert.deepStrictEqual(await bundleFile(bundle, "assets/clips/intro.mp4"), clip);
  assert.deepStrictEqual(await bundleFile(bundle, "assets/tone.wav"), new Uint8Array([1, 2, 3]));
  assert.strictEqual(await bundleFile(bundle, "assets/clips/none.mp4"), null);
  assert.strictEqual(await bundleFile(new Uint8Array(40), "assets/a.mp4"), null, "not a zip");
});
