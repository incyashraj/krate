// Video and sound for a Krate app in a browser tab, the page's half
// (krate:gfx/video, K-1348).
//
// The browser plays it: one <video> element per video, kept out of sight,
// decodes the picture and plays the sound, so a tab plays whatever its
// browser plays (H.264 and AAC everywhere, WebM in Chrome, Edge and
// Firefox, HEVC where the system has it). The app draws by handle, as on a
// computer: hosts.js asks for the newest picture with `video.frame` and
// draws it into the app's canvas; only a new picture crosses the bridge.
//
// hosts.js has already judged the source exactly as the desktop does (a
// path inside the app's assets, or an address the app may reach with
// net.connect); a bundled clip is read out of the app's own bundle here.

const ok = (value) => ({ json: { ok: value === undefined ? null : value } });
const no = (tag, val) => ({ json: { err: val === undefined ? { tag } : { tag, val } } });

/// The largest picture handed to the app's canvas, on its long side: a
/// 4K frame is 33 MB a frame, which no bridge carries at 30 a second, and a
/// canvas in a tab is rarely wider than this.
const MAX_SIDE = 1280;
const MAX_OPEN = 16;

const KINDS = {
  mp4: 'video/mp4', m4v: 'video/mp4', mov: 'video/quicktime', webm: 'video/webm', mkv: 'video/x-matroska',
  mp3: 'audio/mpeg', m4a: 'audio/mp4', aac: 'audio/aac', wav: 'audio/wav', ogg: 'audio/ogg', oga: 'audio/ogg',
  opus: 'audio/ogg', flac: 'audio/flac',
};
const kindOf = (name) => KINDS[(String(name).split('.').pop() || '').toLowerCase()] || 'application/octet-stream';

/// One file out of the app's bundle (a zip), by its name in the zip, or
/// null when it is not there.
export async function bundleFile(bundle, name) {
  const view = new DataView(bundle.buffer, bundle.byteOffset, bundle.byteLength);
  const u16 = (at) => view.getUint16(at, true);
  const u32 = (at) => view.getUint32(at, true);
  // The end-of-directory record sits within the last 64 KiB + 22 bytes.
  let end = -1;
  for (let at = bundle.length - 22; at >= Math.max(0, bundle.length - 65557); at -= 1) {
    if (u32(at) === 0x06054b50) { end = at; break; }
  }
  if (end < 0) return null;
  const entries = u16(end + 10);
  let at = u32(end + 16);
  const want = new TextEncoder().encode(name);
  for (let i = 0; i < entries && at + 46 <= bundle.length; i += 1) {
    if (u32(at) !== 0x02014b50) return null;
    const method = u16(at + 10);
    const size = u32(at + 20);
    const nameLen = u16(at + 28);
    const local = u32(at + 42);
    const here = bundle.subarray(at + 46, at + 46 + nameLen);
    at += 46 + nameLen + u16(at + 30) + u16(at + 32);
    if (here.length !== want.length || !here.every((b, k) => b === want[k])) continue;
    if (local + 30 > bundle.length || u32(local) !== 0x04034b50) return null;
    const start = local + 30 + u16(local + 26) + u16(local + 28);
    if (start + size > bundle.length) return null;
    const raw = bundle.subarray(start, start + size);
    if (method === 0) return raw.slice();
    if (method !== 8) return null;
    const stream = new Blob([raw]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
    return new Uint8Array(await new Response(stream).arrayBuffer());
  }
  return null;
}

export function pageVideo({ bundle, report }) {
  const videos = new Map();
  let next = 1;
  const state = (report.videos = { opened: 0, frames: 0, problems: [] });
  const video = (id) => videos.get(String(id));

  const problemOf = (v) => {
    const e = v.el.error;
    if (v.problem) return v.problem;
    if (!e) return '';
    return {
      1: 'the video stopped loading',
      2: 'the video could not be fetched',
      3: 'this browser could not decode the video',
      4: 'this browser cannot play this video; it plays in Krate on a computer',
    }[e.code] || 'this video cannot be played';
  };

  /// Play, and if the browser wants a tap first (a sound with no tap on
  /// the page yet), play on the person's next tap or key.
  const play = (v) => {
    v.wantPlay = true;
    v.el.play().catch((e) => {
      if (!e || e.name !== 'NotAllowedError' || v.waiting) return;
      v.waiting = true;
      const go = () => {
        removeEventListener('pointerdown', go, true);
        removeEventListener('keydown', go, true);
        v.waiting = false;
        if (v.wantPlay && videos.get(v.id) === v) v.el.play().catch(() => {});
      };
      addEventListener('pointerdown', go, true);
      addEventListener('keydown', go, true);
    });
  };

  const handlers = {
    'video.open': async ({ source }) => {
      if (videos.size >= MAX_OPEN) return no('platform', `${MAX_OPEN} videos are already open; close one before opening another`);
      let src;
      let blobUrl = null;
      const https = /^https?:\/\//i.test(source);
      if (https) {
        src = source;
      } else {
        const data = bundle ? await bundleFile(bundle, `assets/${source}`) : null;
        if (!data) return no('unsupported', `there is no ${source} in the app's assets`);
        blobUrl = URL.createObjectURL(new Blob([data], { type: kindOf(source) }));
        src = blobUrl;
      }
      const el = document.createElement('video');
      el.playsInline = true;
      el.preload = 'auto';
      el.className = 'feed';
      // An address from elsewhere is read with CORS, or its picture could
      // not be handed to the app at all (a tainted canvas).
      if (https) el.crossOrigin = 'anonymous';
      el.src = src;
      document.body.appendChild(el);
      const id = String(next++);
      const v = { id, el, blobUrl, looping: false, ended: false, problem: '', seq: 0, wantPlay: false, waiting: false, canvas: null, g: null, lastTime: -1 };
      el.addEventListener('ended', () => {
        if (v.looping) { el.currentTime = 0; el.play().catch(() => {}); } else { v.ended = true; v.wantPlay = false; }
      });
      if (typeof el.requestVideoFrameCallback === 'function') {
        const tick = () => { v.seq += 1; if (videos.get(id) === v) el.requestVideoFrameCallback(tick); };
        el.requestVideoFrameCallback(tick);
      } else {
        v.seq = -1;
      }
      videos.set(id, v);
      state.opened += 1;
      return ok(id);
    },
    'video.control': ({ id, kind, value }) => {
      const v = video(id);
      if (!v) return no('invalid-target');
      const el = v.el;
      switch (kind) {
        case 'play':
          if (v.ended || (el.duration && el.currentTime >= el.duration)) el.currentTime = 0;
          v.ended = false;
          play(v);
          break;
        case 'pause': v.wantPlay = false; el.pause(); break;
        case 'seek': v.ended = false; el.currentTime = Math.max(0, Number(value) / 1000); break;
        case 'looping': v.looping = !!value; break;
        case 'muted': el.muted = !!value; break;
        case 'volume': el.volume = Math.min(1, Math.max(0, Number(value))); break;
        default: return no('platform', `no ${kind}`);
      }
      return ok();
    },
    'video.state': ({ id }) => {
      const v = video(id);
      if (!v) return no('invalid-target');
      const el = v.el;
      const ms = (s) => (Number.isFinite(s) && s > 0 ? Math.round(s * 1000) : 0);
      const problem = problemOf(v);
      if (problem && !state.problems.includes(problem)) state.problems.push(problem);
      return ok({
        positionMillis: ms(el.currentTime),
        durationMillis: ms(el.duration),
        width: el.videoWidth || 0,
        height: el.videoHeight || 0,
        ready: el.readyState >= 2,
        playing: !el.paused && !el.ended,
        ended: v.ended,
        problem,
      });
    },
    'video.frame': ({ id }) => {
      const v = video(id);
      if (!v) return no('invalid-target');
      const el = v.el;
      const width = el.videoWidth;
      const height = el.videoHeight;
      if (!width || !height || el.readyState < 2) return ok(null);
      // Nothing new since the last read is nothing.
      if (v.seq > 0 ? v.seq === v.read : el.currentTime === v.lastTime) return ok(null);
      const scale = Math.min(1, MAX_SIDE / Math.max(width, height));
      const w = Math.max(1, Math.round(width * scale));
      const h = Math.max(1, Math.round(height * scale));
      if (!v.canvas) v.canvas = document.createElement('canvas');
      if (v.canvas.width !== w || v.canvas.height !== h) { v.canvas.width = w; v.canvas.height = h; v.g = null; }
      if (!v.g) v.g = v.canvas.getContext('2d', { willReadFrequently: true });
      v.g.drawImage(el, 0, 0, w, h);
      let pixels;
      try {
        pixels = v.g.getImageData(0, 0, w, h).data;
      } catch {
        v.problem = 'the server does not let a browser tab show this video (CORS); it plays in Krate on a computer';
        return ok(null);
      }
      v.read = v.seq;
      v.lastTime = el.currentTime;
      state.frames += 1;
      return { json: { ok: { width: w, height: h } }, body: new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength) };
    },
    'video.close': ({ id }) => {
      const v = video(id);
      if (!v) return ok();
      videos.delete(String(id));
      v.wantPlay = false;
      v.el.pause();
      v.el.removeAttribute('src');
      v.el.load();
      v.el.remove();
      if (v.blobUrl) URL.revokeObjectURL(v.blobUrl);
      return ok();
    },
  };
  return {
    handlers,
    /// The app ended: every video it left open stops, so it is never heard.
    close() {
      for (const id of [...videos.keys()]) handlers['video.close']({ id });
    },
  };
}
