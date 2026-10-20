// Krate's host interfaces for an app in a browser tab (IC-956 Phase 1).
//
// Runs in the app's Web Worker, beside the app. Every rule that decides
// something -- the widget tree, layout, focus, scroll, the consent words,
// the grants, the net.connect judgement, the store's bounds -- is the
// desktop's own Rust, compiled into the Player module (crates/adapter-web);
// this file only moves values between jco's JavaScript shapes and it.
//
// jco's shapes: a u64 is a BigInt, an option is the value or undefined, a
// variant is {tag, val}, an enum is its case name, a record's fields are
// camelCase, and a failed `result` is a thrown object carrying `payload`.
//
// Anything the tab has no equivalent for is answered by the refusal table
// the hub made at publish (web.json `refusals`): the function's own error
// case, in words that say to open the app in Krate, or a neutral answer
// where the function has no error channel. The page shows the person a
// line saying so, once per thing.

export class HostError extends Error {
  constructor(payload, words) {
    super(words || (typeof payload === 'object' ? JSON.stringify(payload) : String(payload)));
    this.payload = payload;
  }
}

/// Thrown to end the app with words (a trap), e.g. a denied clock read,
/// which traps on desktop too.
export class AppStopped extends Error {}

const camel = (name) => name.replace(/-([a-z0-9])/g, (_, c) => c.toUpperCase());
const bare = (iface) => iface.replace(/@[^@/]*$/, '');

/// u64 fields cross into the Player module as strings.
export const toJson = (value) =>
  JSON.stringify(value, (_, v) => (typeof v === 'bigint' ? v.toString() : v));

/// One answer from the Player module: the value, or throw the WIT error.
function unwrap(text) {
  const answer = JSON.parse(text);
  if (Object.prototype.hasOwnProperty.call(answer, 'err')) throw new HostError(answer.err);
  return answer.ok;
}

/// u64 fields of the events the Player module emits, by event case.
function liftEvent(e) {
  const big = (v) => (v === null || v === undefined ? undefined : BigInt(v));
  switch (e.tag) {
    case 'close-requested':
    case 'redraw-requested':
    case 'action':
      return { tag: e.tag, val: BigInt(e.val) };
    case 'focus-changed':
      return { tag: e.tag, val: big(e.val) };
    case 'pointer':
    case 'wheel':
    case 'key':
      return { tag: e.tag, val: { ...e.val, window: BigInt(e.val.window), widget: big(e.val.widget),
        button: e.val.button === null ? undefined : e.val.button } };
    case 'text-changed':
      return { tag: e.tag, val: { ...e.val, window: BigInt(e.val.window), widget: BigInt(e.val.widget) } };
    default:
      return e;
  }
}

const OPEN_IN_KRATE = 'This works in Krate on a computer, not in a browser tab yet. Open the app in Krate to use it.';

/// What the hub's table says to answer for a function the tab lacks.
function refusalStub(iface, fname, shape, notice) {
  return (...args) => {
    notice(iface, fname);
    if (!shape || shape.trap) {
      throw new AppStopped(`${bare(iface)}.${fname}: ${OPEN_IN_KRATE}`);
    }
    if (shape.throw === 'variant') {
      throw new HostError(shape.words ? { tag: shape.case, val: OPEN_IN_KRATE } : { tag: shape.case }, OPEN_IN_KRATE);
    }
    if (shape.throw === 'enum') throw new HostError(shape.case, OPEN_IN_KRATE);
    if (shape.throw === 'string') throw new HostError(OPEN_IN_KRATE, OPEN_IN_KRATE);
    if (shape.throw === 'unit') throw new HostError(undefined, OPEN_IN_KRATE);
    if (shape.bigint) return 0n;
    return shape.value === null ? undefined : shape.value;
  };
}

/// Build the import object jco's `instantiate` takes.
///
/// ctx: { meta, ui, grants, kv, ring, bridge, args, output, notice,
///        present, onWindow, onTitle, screen, sleepCell }
export function buildImports(ctx) {
  const { ui, grants, kv, ring, bridge } = ctx;
  const allows = (cap) => grants.allows(cap);
  const need = (cap, words) => {
    if (!allows(cap)) throw new AppStopped(`${words} (this app was not allowed ${cap})`);
  };

  // ---------------------------------------------------------------- io
  const decoders = new Map();
  const write = (fd, bytes) => {
    if (!decoders.has(fd)) decoders.set(fd, new TextDecoder());
    ctx.output(fd, decoders.get(fd).decode(bytes, { stream: true }));
  };
  class OutputStream {
    constructor(fd) { this.fd = fd; }
    write(bytes) { write(this.fd, bytes); return bytes.length; }
    writeAll(bytes) { write(this.fd, bytes); }
    flush() {}
  }
  // A tab has no terminal to type into: stdin is closed, as when a desktop
  // app is run with nothing piped in.
  class InputStream {
    read() { throw new HostError({ tag: 'closed' }); }
    readToString() { return ''; }
  }

  // --------------------------------------------------------------- input
  // Apply one record from the page's ring to the ui host.
  const apply = (r) => {
    const w = r.window;
    switch (r.type) {
      case 'down': ui.pointer(w, r.x, r.y, r.button, true, r.mods); ctx.inputAt(r.at); break;
      case 'up': ui.pointer(w, r.x, r.y, r.button, false, r.mods); break;
      case 'move': ui.pointer_moved(w, r.x, r.y); break;
      case 'leave': ui.pointer_left(w); break;
      case 'wheel': ui.wheel(w, r.x, r.y, r.dx, r.dy, r.mods); ctx.inputAt(r.at); break;
      case 'touch': ui.touch(w, r.phase, r.finger, r.x, r.y); if (r.phase !== 'move') ctx.inputAt(r.at); break;
      case 'key': ui.key(w, r.key, r.pressed, r.mods, r.text); if (r.pressed) ctx.inputAt(r.at); break;
      case 'text': ui.text(w, r.text); ctx.inputAt(r.at); break;
      case 'close': ui.close_requested(w); break;
      case 'resize': ctx.resize(w, r.width, r.height); break;
      case 'theme': ui.theme(r.dark); break;
      case 'blur': ui.blurred(); break;
      default: break;
    }
  };
  const drain = () => {
    for (let r = ring.take(0); r; r = ring.take(0)) apply(r);
  };
  const nextEvent = () => {
    const e = ui.next_event();
    return e === undefined ? undefined : liftEvent(JSON.parse(e));
  };

  // ---------------------------------------------------------------- http
  const methods = { get: 'GET', post: 'POST', put: 'PUT', delete: 'DELETE', patch: 'PATCH', head: 'HEAD', options: 'OPTIONS' };
  const judged = (url, headers, body) => {
    const answer = JSON.parse(grants.check_http(url, JSON.stringify(headers || []), body ? body.length : 0));
    if (answer.err) throw new HostError(answer.err);
  };
  const toResponse = ({ json, body }) => {
    if (json.err) throw new HostError(json.err);
    return { status: json.ok.status, headers: json.ok.headers, body: new Uint8Array(body) };
  };
  const request = (req) => ({
    method: methods[req.method] || 'GET',
    url: req.url,
    headers: req.headers,
    timeout: req.timeoutMillis,
  });

  // ----------------------------------------------------------------- org
  // An organisation's team data (store.org, G1). The page holds the copy,
  // the merge and the sealing (org-data.js over crates/orgdata in the
  // validator module); this only moves values between jco's shapes and the
  // copy's wire shapes: null, {t}, {n}, {b}, {x: base64}.
  const toB64 = (bytes) => {
    let s = '';
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const fromB64 = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  const toWire = (v) => {
    switch (v.tag) {
      case 'text': return { t: v.val };
      case 'number': return { n: v.val };
      case 'boolean': return { b: v.val };
      case 'bytes': return { x: toB64(v.val) };
      default: return null;
    }
  };
  const fromWire = (w) => {
    if (w === null || w === undefined) return { tag: 'null' };
    if ('t' in w) return { tag: 'text', val: w.t };
    if ('n' in w) return { tag: 'number', val: w.n };
    if ('b' in w) return { tag: 'boolean', val: w.b };
    return { tag: 'bytes', val: fromB64(w.x) };
  };
  const entry = (e) => ({
    id: e.id,
    fields: e.fields.map(([name, w]) => [name, fromWire(w)]),
    changedBy: e.changed_by,
    changedAt: BigInt(e.changed_at),
  });
  const big = (n) => (n === null || n === undefined ? undefined : BigInt(n));
  const orgCall = (op, args) => {
    if (!allows('store.org')) throw new HostError({ tag: 'denied' });
    // Not an organisation's app opened as its release: no team data, as on
    // a computer. `unsupported` when this browser cannot be a device.
    if (ctx.org !== 'on') throw new HostError({ tag: ctx.org === 'unsupported' ? 'unsupported' : 'not-an-org-app' });
    const { json } = bridge.call(op, args);
    if (json.err) throw new HostError(json.err);
    return json.ok;
  };

  // ------------------------------------------------------------------ kv
  const kvCall = (op, args, value) => {
    const { json } = bridge.call(op, { store: ctx.storeKey, ...args }, value ? [] : undefined);
    if (json.err) throw new HostError(json.err);
  };

  const impl = {
    'krate:io/types': {},
    'krate:io/streams': { OutputStream, InputStream },
    'krate:io/stdio': {
      stdout: () => new OutputStream(1),
      stderr: () => new OutputStream(2),
      stdin: () => new InputStream(),
    },
    'krate:io/args': {
      raw: () => { need('io.args', 'reading what the app was started with'); return ctx.args; },
    },
    'krate:io/log': {
      emit(level, message) { if (allows('io.log')) ctx.output(2, `[${level}] ${message}\n`); },
    },
    'krate:random/bytes': {
      get(count) {
        if (!allows('random.bytes')) throw new HostError({ tag: 'denied' });
        if (count > 64 * 1024) throw new HostError({ tag: 'too-large' });
        const out = new Uint8Array(count);
        for (let at = 0; at < count; at += 65536) crypto.getRandomValues(out.subarray(at, Math.min(count, at + 65536)));
        return out;
      },
      nextU64() {
        if (!allows('random.bytes')) throw new HostError({ tag: 'denied' });
        return new DataView(crypto.getRandomValues(new Uint8Array(8)).buffer).getBigUint64(0, true);
      },
      below(bound) {
        if (!allows('random.bytes')) throw new HostError({ tag: 'denied' });
        if (bound === 0n) throw new HostError({ tag: 'empty-range' });
        // Rejection sampling, as the desktop does: `%` alone would favour
        // the low end whenever `bound` does not divide 2^64.
        const max = (1n << 64n) - 1n;
        const limit = max - (max % bound) - 1n;
        for (;;) {
          const v = new DataView(crypto.getRandomValues(new Uint8Array(8)).buffer).getBigUint64(0, true);
          if (v <= limit) return v % bound;
        }
      },
    },
    'krate:time/clock': {
      nowMillis: () => { need('time.clock', 'reading the current time'); return BigInt(Date.now()); },
      monotonicNanos: () => { need('time.monotonic', 'measuring time'); return BigInt(Math.floor(performance.now() * 1e6)); },
    },
    'krate:time/sleep': {
      sleepMillis(ms) { need('time.sleep', 'waiting'); Atomics.wait(ctx.sleepCell, 0, 0, ms); },
    },
    'krate:locale/types': {},
    'krate:locale/info': {
      current: () => { need('locale.info', 'reading your language'); return { bcp47: navigator.language || 'en-US' }; },
      timezone: () => { need('locale.info', 'reading your time zone'); return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'; },
    },
    'krate:locale/format': {
      formatDate(millis, tz, style, loc) {
        need('locale.format', 'formatting a date');
        try { return new Intl.DateTimeFormat(loc.bcp47, { dateStyle: style, timeZone: tz || undefined }).format(Number(millis)); }
        catch { return new Intl.DateTimeFormat('en-US', { dateStyle: style, timeZone: 'UTC' }).format(Number(millis)); }
      },
      formatNumber(value, style, loc) {
        need('locale.format', 'formatting a number');
        const opts = style === 'percent' ? { style: 'percent' } : {};
        try { return new Intl.NumberFormat(loc.bcp47, opts).format(value); }
        catch { return new Intl.NumberFormat('en-US', opts).format(value); }
      },
    },
    'krate:net/types': {},
    'krate:net/http-client': {
      get(url) {
        judged(url, [], null);
        return toResponse(bridge.call('http.fetch', { method: 'GET', url, headers: [] })).body;
      },
      fetch(req) {
        judged(req.url, req.headers, req.body);
        const body = req.body && req.body.length ? req.body.slice() : undefined;
        return toResponse(bridge.call('http.fetch', { ...request(req), body }, body ? [body.buffer] : []));
      },
      begin(req) {
        // The grant is judged here, before anything leaves the tab, as on
        // desktop: a handle is only issued for a host the person allowed.
        judged(req.url, req.headers, req.body);
        const body = req.body && req.body.length ? req.body.slice() : undefined;
        const { json } = bridge.call('http.begin', { ...request(req), body }, body ? [body.buffer] : []);
        return BigInt(json.ok);
      },
      poll(handle) {
        const { json, body } = bridge.call('http.poll', { handle: handle.toString() });
        if (json.state === 'pending') return { tag: 'pending' };
        if (json.state === 'unknown') return { tag: 'unknown-handle' };
        if (json.err) return { tag: 'failed', val: json.err };
        return { tag: 'ready', val: { status: json.ok.status, headers: json.ok.headers, body: new Uint8Array(body) } };
      },
      cancel(handle) { bridge.call('http.cancel', { handle: handle.toString() }); },
    },
    'krate:store/kv': {
      get(key) {
        if (unwrap(kv.get(key))) return kv.value(key);
        return undefined;
      },
      set(key, value) {
        unwrap(kv.check_set(key, value.length));
        const copy = value.slice();
        kvCall('kv.set', { key, value: copy });
        kv.commit_set(key, copy);
      },
      delete(key) {
        if (unwrap(kv.check_delete(key))) {
          kvCall('kv.delete', { key });
          kv.commit_delete(key);
        }
      },
      keys: () => unwrap(kv.keys()),
      clear() {
        unwrap(kv.check_clear());
        kvCall('kv.clear', {});
        kv.commit_clear();
      },
    },
    'krate:store/org': {
      put(collection, id, fields) {
        orgCall('org.put', { collection, id, fields: fields.map(([name, v]) => [name, toWire(v)]) });
      },
      get(collection, id) {
        const e = orgCall('org.get', { collection, id });
        return e ? entry(e) : undefined;
      },
      delete(collection, id) { orgCall('org.delete', { collection, id }); },
      records(collection, since) {
        const p = orgCall('org.records', { collection, since: since === undefined ? null : since });
        return { entries: p.entries.map(entry), removed: p.removed, next: p.next, more: p.more };
      },
      sync() {
        const s = orgCall('org.sync', {});
        return { online: s.online, changed: s.changed, pending: s.pending, lastSynced: big(s.last_synced) };
      },
      who() {
        const m = orgCall('org.who', {});
        return m ? { name: m.name, role: m.role, org: m.org } : undefined;
      },
      // Where this tab stands, never waiting (K-1108's status).
      status() {
        const s = orgCall('org.status', {});
        return { link: s.link, pending: s.pending, reachedAt: big(s.reached_at), org: s.org };
      },
    },
    'krate:ui/types': {},
    'krate:ui/window': {
      create(title, size) {
        if (!allows('ui.window:create')) throw new HostError({ tag: 'permission-denied' });
        const fit = ctx.fit(size.width, size.height);
        const id = unwrap(ui.create_window(title, fit.width, fit.height, size.width, size.height));
        ctx.onWindow(id, title, size);
        return BigInt(id);
      },
      show: (w) => { unwrap(ui.show_window(w.toString())); },
      close: (w) => { unwrap(ui.close_window(w.toString())); ctx.onClose(w.toString()); },
      setTitle: (w, title) => { unwrap(ui.set_title(w.toString(), title)); ctx.onTitle(title); },
      setSize: (w, size) => {
        const fit = ctx.fit(size.width, size.height);
        unwrap(ui.set_size(w.toString(), fit.width, fit.height, false));
        ctx.onResized(w.toString(), size);
      },
      setState: () => { throw new HostError({ tag: 'unsupported', val: 'window state changes are not implemented yet' }); },
      requestRedraw: (w) => { unwrap(ui.request_redraw(w.toString())); },
      setFullBleed: () => {},
    },
    'krate:ui/tree': {
      setRoot: (w, node) => { unwrap(ui.set_root(w.toString(), toJson(node))); },
      upsertNode: (w, node) => { unwrap(ui.upsert(w.toString(), toJson(node))); },
      removeNode: (w, id) => { unwrap(ui.remove(w.toString(), id.toString())); },
      focusNode: (w, id) => { unwrap(ui.focus(w.toString(), id.toString())); },
      setEnabled: () => { throw new HostError({ tag: 'unsupported', val: 'widget enable state is not implemented yet' }); },
    },
    'krate:ui/image': {
      setPixels: (w, id, px) => { unwrap(ui.set_image(w.toString(), id.toString(), px.width, px.height, px.rgba)); },
      clear: (w, id) => { unwrap(ui.clear_image(w.toString(), id.toString())); },
    },
    'krate:ui/events': {
      poll() {
        drain();
        ctx.present();
        return nextEvent();
      },
      wait(timeout) {
        // The app asking for its next event is the frame boundary: whatever
        // it changed since the last wait is painted now, once.
        drain();
        const until = timeout === undefined ? Infinity : performance.now() + timeout;
        for (;;) {
          const e = nextEvent();
          if (e) return e;
          ctx.present();
          const left = until === Infinity ? undefined : Math.max(0, until - performance.now());
          if (left === 0) return undefined;
          const r = ring.take(left);
          if (!r) {
            if (until !== Infinity && performance.now() >= until) return undefined;
            continue;
          }
          apply(r);
          drain();
        }
      },
      keyHeld: (name) => { drain(); return ui.key_held(name); },
      gamepadConnected: () => false,
      gamepadHeld: () => false,
      gamepadAxis: () => 0,
    },
    'krate:ui/pointer': {
      position: (w) => { drain(); const p = ui.position(w.toString()); return p ? [p[0], p[1]] : undefined; },
      buttonHeld: (b) => { drain(); return ui.button_held({ primary: 0, middle: 1, secondary: 2 }[b] ?? 3); },
      takeMotion: () => [0, 0],
      capture: () => { throw new HostError({ tag: 'unsupported', val: OPEN_IN_KRATE }); },
      release: () => {},
      captured: () => false,
    },
    'krate:ui/text-entry': {
      start: (w) => { ctx.textEntry(true); return true; },
      stop: () => { ctx.textEntry(false); },
    },
    'krate:ui/screen': {
      facts(w) {
        const s = ctx.screen(w.toString());
        const cls = (side, compact, large) => (side < compact ? 'compact' : side >= large ? 'large' : 'medium');
        return {
          width: s.width, height: s.height,
          widthClass: cls(s.width, 600, 1000), heightClass: cls(s.height, 500, 900),
          input: s.touch ? 'touch' : 'pointer',
          safe: { top: 0, right: 0, bottom: 0, left: 0 },
        };
      },
    },
    'krate:ui/appearance': {
      current: () => ({ dark: ui.dark(), highContrast: false, textScale: 1 }),
    },
    'krate:ui/accessibility': {
      describe: () => {},
      screenReaderOn: () => false,
    },
    'krate:ui/clipboard': {
      readText() {
        if (!allows('ui.clipboard:read')) throw new HostError({ tag: 'permission-denied' });
        const { json } = bridge.call('clipboard.read', {});
        if (json.err) throw new HostError(json.err);
        return json.ok;
      },
      writeText(text) {
        if (!allows('ui.clipboard:write')) throw new HostError({ tag: 'permission-denied' });
        const { json } = bridge.call('clipboard.write', { text });
        if (json.err) throw new HostError(json.err);
      },
    },
    'krate:ui/launcher': {
      openUrl(url) {
        if (!allows('ui.open-url')) throw new HostError({ tag: 'denied' });
        if (!/^https?:\/\/[^\s]+$/i.test(url)) throw new HostError({ tag: 'invalid-url', val: 'only http and https links open' });
        const { json } = bridge.call('open-url', { url });
        if (json.err) throw new HostError(json.err);
      },
    },
  };

  // ------------------------------------------------------------ canvas2d
  // Every op goes to the desktop's own rasterizer in the Player module;
  // present() puts the frame on screen and paces a loop that never waits
  // to the display, as the desktop does (16.7 ms).
  const g = (err) => { if (err !== undefined) throw new HostError(JSON.parse(err)); };
  const col = (c) => [c.r, c.g, c.b, c.a];
  const geo = (a, r) => (r ? [a.x, a.y, a.width, a.height, r.topLeft, r.topRight, r.bottomRight, r.bottomLeft] : [a.x, a.y, a.width, a.height]);
  const sty = (s) => [s.weight, s.italic ? 1 : 0, s.letterSpacing, { sans: 0, serif: 1, mono: 2 }[s.family] ?? 0];
  let lastPresent = 0;
  impl['krate:gfx/types'] = {};
  impl['krate:gfx/canvas2d'] = {
    bind: (w, widget) => BigInt(unwrap(ui.c_bind(w.toString(), widget.toString()))),
    canvasSize: (c) => { const [width, height] = unwrap(ui.c_size(c)); return { width, height }; },
    setDesignSize: (c, s) => g(ui.c_design(c, s.width, s.height)),
    setClip: (c, x, y, w, h) => g(ui.c_clip(c, x, y, w, h)),
    clearClip: (c) => g(ui.c_unclip(c)),
    clear: (c, f) => g(ui.c_clear(c, f.r, f.g, f.b, f.a)),
    fillRect: (c, a, f) => g(ui.c_fill_rect(c, a.x, a.y, a.width, a.height, f.r, f.g, f.b, f.a)),
    strokeRect: (c, a, s, width) => g(ui.c_stroke_rect(c, a.x, a.y, a.width, a.height, s.r, s.g, s.b, s.a, width)),
    fillRoundRect: (c, a, r, f) => g(ui.c_fill_round_rect(c, geo(a, r), col(f))),
    strokeRoundRect: (c, a, r, width, s) => g(ui.c_stroke_round_rect(c, geo(a, r), width, col(s))),
    dropShadowRoundRect: (c, a, r, blur, s) => g(ui.c_drop_shadow(c, geo(a, r), blur, col(s))),
    linearGradientStops: (c, a, angle, stops) => g(ui.c_gradient_stops(c, geo(a), angle, stops.flatMap((s) => [s.offset, ...col(s.color)]))),
    drawTextStyled: (c, text, o, size, ink, s) => g(ui.c_text(c, text, o.x, o.y, size, col(ink), sty(s))),
    measureTextStyled: (c, text, size, s) => { const [width, height, ascent, descent] = unwrap(ui.c_measure(c, text, size, sty(s))); return { width, height, ascent, descent }; },
    strokeArc: (c, ctr, radius, start, sweep, width, s) => g(ui.c_stroke_arc(c, [ctr.x, ctr.y, radius, start, sweep, width], col(s))),
    drawPixelsRound: (c, a, r, w, h, rgba) => g(ui.c_pixels(c, geo(a, r), w, h, rgba)),
    fillCircle: (c, ctr, radius, f) => g(ui.c_fill_circle(c, ctr.x, ctr.y, radius, col(f))),
    strokeCircle: (c, ctr, radius, width, s) => g(ui.c_stroke_circle(c, ctr.x, ctr.y, radius, width, col(s))),
    radialGradient: (c, ctr, radius, inner, outer) => g(ui.c_radial_gradient(c, ctr.x, ctr.y, radius, col(inner), col(outer))),
    linearGradient: (c, a, top, bottom) => g(ui.c_linear_gradient(c, geo(a), col(top), col(bottom))),
    drawText: (c, text, o, size, ink) => g(ui.c_text(c, text, o.x, o.y, size, col(ink), [])),
    measureText: (c, text, size) => { const [width, height, ascent, descent] = unwrap(ui.c_measure(c, text, size, [])); return { width, height, ascent, descent }; },
    drawPixels: (c, a, w, h, rgba) => g(ui.c_pixels(c, geo(a), w, h, rgba)),
    drawSprite: (c, ctr, dst, angle, w, h, rgba) => g(ui.c_sprite(c, [ctr.x, ctr.y, dst.width, dst.height, angle], w, h, rgba)),
    present(c) {
      g(ui.c_present(c));
      drain();
      ctx.present();
      const left = 16.7 - (performance.now() - lastPresent);
      if (left > 1) Atomics.wait(ctx.sleepCell, 0, 0, left);
      lastPresent = performance.now();
    },
  };
  impl['krate:gfx/canvas-images'] = {
    keep: (c, w, h, rgba) => BigInt(unwrap(ui.c_keep(c, w, h, rgba))),
    draw: (c, image, a) => g(ui.c_draw_kept(c, image, geo(a))),
    drawRound: (c, image, a, r) => g(ui.c_draw_kept(c, image, geo(a, r))),
    forget: (c, image) => ui.c_forget(c, image),
  };

  // Every interface the component imports: what this file implements,
  // and the hub's refusal for anything else -- including whole interfaces.
  const imports = {};
  const refusals = ctx.meta.refusals || {};
  const names = new Set([...Object.keys(refusals).map(bare), ...Object.keys(impl), ...(ctx.meta.imports || []).map(bare)]);
  for (const name of names) {
    const own = impl[name] || {};
    const table = Object.entries(refusals).find(([k]) => bare(k) === name)?.[1] || {};
    const byCamel = Object.fromEntries(Object.entries(table).map(([f, shape]) => [camel(f), [f, shape]]));
    const iface = new Proxy(own, {
      get(target, prop) {
        if (prop in target) return target[prop];
        if (typeof prop !== 'string') return undefined;
        const [fname, shape] = byCamel[prop] || [prop, null];
        return refusalStub(name, fname, shape, ctx.notice);
      },
      has() { return true; },
    });
    imports[name] = iface;
  }
  return imports;
}
