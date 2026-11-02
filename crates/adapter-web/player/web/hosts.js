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

import { workerAudio, makeAudioMemory } from './audio.js';

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
///        present, onWindow, onTitle, screen, sleepCell, audio, audioPost }
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
  // Events the page makes rather than the ui host: a dropped file, a hover.
  const fileEvents = [];
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
      // A file dropped on the app, or a drag over it: the page already
      // judged the drop by the app's ui.dropzone grant and made the token.
      case 'drop': fileEvents.push({ tag: 'file-dropped', val: { name: r.name, token: r.token } }); ctx.inputAt(r.at); break;
      case 'hover': fileEvents.push({ tag: 'file-hovering', val: Boolean(r.over) }); break;
      default: break;
    }
  };
  const drain = () => {
    for (let r = ring.take(0); r; r = ring.take(0)) apply(r);
  };
  const nextEvent = () => {
    if (fileEvents.length) return fileEvents.shift();
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

  // ------------------------------------------- store.shared, store.secret
  // The shared store and the secrets live on the page (shared-store.js,
  // secret-store.js): the shared copy syncs with the hub in the background,
  // which a blocked worker cannot, and the secrets' key is the page's. A
  // call without the grant is denied here, as on desktop, before anything
  // reaches the page.
  const storeCall = (cap, op, args, transfer) => {
    if (!allows(cap)) throw new HostError({ tag: 'denied' });
    const answer = bridge.call(op, args || {}, transfer);
    if (answer.json.err) throw new HostError(answer.json.err);
    return answer;
  };
  const bytesOf = ({ json, body }) => (json.ok ? new Uint8Array(body) : undefined);

  // ---------------------------------------------------------- store.sql
  // The app's database (sqldb.js, crates/sql-web): SQLite and the desktop's
  // rules, in this worker. Every write is kept before the call returns, so
  // an app that was told its change went through can rely on it.
  const sql = ctx.sql;
  const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
  const unhex = (t) => { const o = new Uint8Array(t.length / 2); for (let i = 0; i < o.length; i++) o[i] = parseInt(t.substr(i * 2, 2), 16); return o; };
  const toSql = (v) => {
    switch (v.tag) {
      case 'integer': return { t: 'integer', v: v.val.toString() };
      case 'real': return { t: 'real', v: Number.isFinite(v.val) ? v.val : String(v.val) };
      case 'text': return { t: 'text', v: v.val };
      case 'blob': return { t: 'blob', v: hex(v.val) };
      default: return { t: 'null' };
    }
  };
  const fromSql = (v) => {
    switch (v.t) {
      case 'integer': return { tag: 'integer', val: BigInt(v.v) };
      case 'real': return { tag: 'real', val: Number(v.v) };
      case 'text': return { tag: 'text', val: v.v };
      case 'blob': return { tag: 'blob', val: unhex(v.v) };
      default: return { tag: 'null' };
    }
  };
  const sqlCall = (request) => {
    // The database module itself could not start in this tab.
    if (sql.startFailed) throw new HostError({ tag: 'io', val: sql.startFailed });
    const answer = sql.call(request);
    if (sql.dirty()) {
      const image = sql.image();
      const { json } = bridge.call('sql.save', { image }, [image.buffer]);
      if (json.err) throw new HostError({ tag: 'io', val: json.err.val || 'the browser did not keep the change' });
    }
    if (answer.err) throw new HostError(answer.err);
    return answer.ok;
  };

  // ------------------------------------------------------- picked files
  // A file the person chose in the browser's picker, named in a save, or
  // dropped on the app (IC-974). As on desktop the app holds a name and a
  // token, never a path (crates/runtime/src/chosen_files.rs). The page keeps
  // the file itself -- the browser's File, which is not read until asked --
  // and its list of tokens dies with the tab, so a token is good for this
  // run only. Reads come from the page a window at a time; what the app
  // writes is kept here until it lets go of the file, and then goes to the
  // page, which hands it to the person as a download: the phone's save
  // (K-1032), since a tab cannot write into a folder of theirs.
  const DISPOSE = Symbol.dispose || Symbol.for('dispose');
  // The desktop's limit for one read or one write (MAX_PHASE2_READ_BYTES).
  const MAX_IO = 8 * 1024 * 1024;
  const READ_WINDOW = 1 << 20;
  const writers = new Set();
  const fsFail = (payload) => new HostError(payload);
  class File {
    constructor(token, mode, info, bytes) {
      this.token = token;
      this.mode = mode;
      this.kind = info.kind;
      this.pos = 0;
      this.size = info.size;
      this.modified = info.modified;
      this.cache = null;
      this.closed = false;
      this.writes = mode !== 'read';
      if (this.writes) {
        // write truncates; read-write and append start from what is there.
        this.buf = bytes && bytes.length ? new Uint8Array(bytes) : new Uint8Array(0);
        this.len = this.buf.length;
        this.dirty = mode === 'write';
        writers.add(this);
      }
    }
    read(n) {
      if (this.mode !== 'read' && this.mode !== 'read-write') throw fsFail({ tag: 'permission-denied' });
      if (n > MAX_IO) throw fsFail({ tag: 'io', val: `read request exceeds limit (${MAX_IO} bytes)` });
      if (this.writes) {
        const out = this.buf.slice(this.pos, Math.min(this.len, this.pos + n));
        this.pos += out.length;
        return out;
      }
      const want = Math.min(n, Math.max(0, this.size - this.pos));
      if (want === 0) return new Uint8Array(0);
      const c = this.cache;
      if (!c || this.pos < c.at || this.pos + want > c.at + c.bytes.length) {
        const { json, body } = bridge.call('file.read', { token: this.token, at: this.pos, len: Math.max(want, READ_WINDOW) });
        if (json.err) throw fsFail(json.err);
        this.cache = { at: this.pos, bytes: body };
      }
      const from = this.pos - this.cache.at;
      const out = this.cache.bytes.slice(from, from + want);
      this.pos += out.length;
      return out;
    }
    write(bytes) {
      if (!this.writes) throw fsFail({ tag: 'permission-denied' });
      if (bytes.length > MAX_IO) throw fsFail({ tag: 'io', val: `write request exceeds limit (${MAX_IO} bytes)` });
      const at = this.mode === 'append' ? this.len : this.pos;
      const end = at + bytes.length;
      if (end > this.buf.length) {
        const grown = new Uint8Array(Math.max(end, this.buf.length * 2, 4096));
        grown.set(this.buf.subarray(0, this.len));
        this.buf = grown;
      }
      this.buf.set(bytes, at);
      this.len = Math.max(this.len, end);
      this.pos = end;
      this.dirty = true;
      this.modified = Date.now();
      return bytes.length;
    }
    seekSet(pos) { this.pos = Number(pos); return pos; }
    seekEnd() { this.pos = this.writes ? this.len : this.size; return BigInt(this.pos); }
    stat() {
      return { size: BigInt(this.writes ? this.len : this.size), modifiedMillis: BigInt(Math.floor(this.modified || 0)), isDir: false };
    }
    // The app let go of the file: what it wrote is the save.
    [DISPOSE]() {
      if (this.closed) return;
      this.closed = true;
      if (!this.writes) return;
      writers.delete(this);
      const data = this.buf.slice(0, this.len);
      this.buf = null;
      // A save always reaches the person; a picked file only when written.
      bridge.call('file.close', { token: this.token, data, offer: this.dirty || this.kind === 'save' }, [data.buffer]);
    }
  }
  // An app that ends while still holding a file it wrote has let go of it.
  ctx.closeFiles = () => { for (const f of [...writers]) f[DISPOSE](); };
  const dialogCall = (cap, op, args) => {
    if (!allows(cap)) throw new HostError({ tag: 'permission-denied' });
    // What the app drew last is on screen while the person answers.
    ctx.present();
    const { json } = bridge.call(op, args);
    if (json.err) throw new HostError(json.err);
    return json.ok;
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
    // Live connections (IC-974): the socket lives on the page, which has an
    // event loop while this worker runs the app, and its events wait there
    // until the app polls, as on desktop. The grant is net.connect's,
    // judged here before anything leaves the tab: ws:// is judged as
    // http://, wss:// as https://, for the same host and port.
    'krate:net/ws': {
      open(url) {
        const asked = String(url);
        const m = /^(wss?):\/\/(.*)$/i.exec(asked);
        if (!m) throw new HostError({ tag: 'invalid-url' });
        judged(`${m[1].toLowerCase() === 'wss' ? 'https' : 'http'}://${m[2]}`, [], null);
        const { json } = bridge.call('ws.open', { url: asked });
        if (json.err) throw new HostError(json.err);
        return BigInt(json.ok);
      },
      send(handle, message) {
        const text = message.tag === 'text';
        const data = text ? undefined : message.val.slice();
        const { json } = bridge.call('ws.send', { handle: handle.toString(), text: text ? message.val : undefined, data }, data ? [data.buffer] : []);
        if (json.err) throw new HostError(json.err);
      },
      poll(handle) {
        const { json, body } = bridge.call('ws.poll', { handle: handle.toString() });
        switch (json.state) {
          case 'pending': return { tag: 'pending' };
          case 'opened': return { tag: 'opened' };
          case 'text': return { tag: 'message', val: { tag: 'text', val: json.text } };
          case 'binary': return { tag: 'message', val: { tag: 'binary', val: new Uint8Array(body) } };
          case 'closed': return { tag: 'closed' };
          case 'failed': return { tag: 'failed', val: json.why || 'the connection failed' };
          default: return { tag: 'unknown-handle' };
        }
      },
      close(handle) { bridge.call('ws.close', { handle: handle.toString() }); },
    },
    ...(sql ? {
      'krate:store/sql': {
        query(statement, params) {
          const ok = sqlCall({ op: 'query', statement, params: params.map(toSql) });
          return { columns: ok.columns, rows: ok.rows.map((values) => ({ values: values.map(fromSql) })) };
        },
        execute: (statement, params) => BigInt(sqlCall({ op: 'execute', statement, params: params.map(toSql) })),
        transaction(statements) { sqlCall({ op: 'transaction', statements }); },
      },
    } : {}),
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
    'krate:store/shared': {
      code: () => storeCall('store.shared', 'shared.code').json.ok ?? undefined,
      create: () => storeCall('store.shared', 'shared.create').json.ok,
      join(code) { storeCall('store.shared', 'shared.join', { code: String(code) }); },
      leave() { storeCall('store.shared', 'shared.leave'); },
      get: (key) => bytesOf(storeCall('store.shared', 'shared.get', { key })),
      set(key, value) {
        const copy = value.slice();
        storeCall('store.shared', 'shared.set', { key, value: copy }, [copy.buffer]);
      },
      delete(key) { storeCall('store.shared', 'shared.delete', { key }); },
      keys: () => storeCall('store.shared', 'shared.keys').json.ok,
      sync: () => storeCall('store.shared', 'shared.sync').json.ok === true,
    },
    'krate:store/secret': {
      get: (name) => bytesOf(storeCall('store.secret', 'secret.get', { name })),
      set(name, secret) {
        const copy = secret.slice();
        storeCall('store.secret', 'secret.set', { name, value: copy }, [copy.buffer]);
      },
      delete(name) { storeCall('store.secret', 'secret.delete', { name }); },
      names: () => storeCall('store.secret', 'secret.names').json.ok,
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
    'krate:fs/files': {
      File,
      // Only a token opens a file here; a path is answered by the hub's
      // refusal, as before, because a tab has no folder of the app's.
      openChosen(token, mode) {
        const { json, body } = bridge.call('file.open', { token: String(token), mode: mode.tag });
        if (json.err) throw fsFail(json.err);
        return new File(String(token), mode.tag, json.ok, mode.tag === 'read-write' || mode.tag === 'append' ? body : null);
      },
    },
    // The folder dialog is not here: a tab answers it with the hub's
    // refusal ("works in Krate on a computer").
    'krate:ui/dialog': {
      message(w, title, body) { dialogCall('ui.dialog:message', 'dialog.ask', { kind: 'message', title, body }); },
      // Dismissed counts as no, as on desktop.
      confirm: (w, title, body) => dialogCall('ui.dialog:confirm', 'dialog.ask', { kind: 'confirm', title, body }) === true,
      openFile(w, title, filter) {
        const chosen = dialogCall('ui.dialog:file-open', 'dialog.open', { title, filter });
        return chosen ? { name: chosen.name, token: chosen.token } : undefined;
      },
      saveFile(w, title, suggested, filter) {
        const chosen = dialogCall('ui.dialog:file-save', 'dialog.save', { title, suggested, filter });
        return chosen ? { name: chosen.name, token: chosen.token } : undefined;
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
  // --------------------------------------------------------------- audio
  // Sound out (K-1134): audio.js converts and queues, the page's
  // AudioWorklet plays, from the person's first tap or key.
  Object.assign(impl, workerAudio({
    allows,
    fail: (payload) => new HostError(payload),
    post: ctx.audioPost || (() => {}),
    memory: ctx.audio || makeAudioMemory(),
  }));
  impl['krate:gfx/canvas-images'] = {
    keep: (c, w, h, rgba) => BigInt(unwrap(ui.c_keep(c, w, h, rgba))),
    draw: (c, image, a) => g(ui.c_draw_kept(c, image, geo(a))),
    drawRound: (c, image, a, r) => g(ui.c_draw_kept(c, image, geo(a, r))),
    forget: (c, image) => ui.c_forget(c, image),
  };
  Object.assign(impl, senses({ allows, bridge, present: () => ctx.present(), facts: ctx.senses || {} }));

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

// ------------------------------------------------------------- senses
// The device's senses in a tab (IC-974): notifications, where the device
// is, the share sheet, speaking aloud, the camera and the microphone. Each
// is the desktop's interface with the desktop's grant, checked here before
// anything reaches the page, so an app without the capability is refused
// without the browser ever asking the person. With the grant, the page
// asks the browser (player.js), and the browser's own "no" comes back as
// the refusal the desktop gives when the system says no. The checks on
// what an app hands over -- a notification's length, a file's name, a
// line's length, a camera's size -- are the desktop's, word for word, from
// the files named beside each.

/// What crates/runtime/src/desktop_host.rs check_notification refuses.
const MAX_TITLE_BYTES = 256;
const MAX_BODY_BYTES = 2048;
// char::is_control: the C0 and C1 control characters.
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;
const utf8 = (s) => new TextEncoder().encode(s).length;
/// crates/runtime/src/speech_synthesis.rs.
const MAX_TEXT_CHARS = 4000;
const MIN_RATE = 0.5;
const MAX_RATE = 2.0;
/// crates/runtime/src/camera_capture.rs.
const MAX_FRAME_PIXELS = 3840 * 2160;
/// crates/runtime/src/audio_capture.rs.
const MIN_SAMPLE_RATE = 8000;
const MAX_SAMPLE_RATE = 192000;
const MAX_CHANNELS = 8;
const MAX_READ_BYTES = 1024 * 1024;
const MAX_CAPTURE_BUFFER_BYTES = 8 * 1024 * 1024;
/// The microphone's ring between the page's audio thread and this worker:
/// samples as the browser hears them, newest kept, 8 MB as on desktop.
export const MIC_SAMPLES = 1 << 21;
// Its control words.
export const MIC_WRITTEN = 0;
export const MIC_CHANNELS = 1;
export const MIC_RATE = 2;
export const MIC_FAILED = 3;
export const MIC_WORDS = 8;
/// What the page puts in MIC_FAILED, and what each says to the app.
export const MIC_FAILURES = {
  1: { tag: 'platform', val: 'the microphone was not allowed: the browser said no. Allow it in the browser\'s settings for this site, then open the app again' },
  2: { tag: 'device-unavailable' },
  3: { tag: 'platform', val: 'the microphone could not start: another app may be using it' },
  4: { tag: 'unsupported', val: 'this browser cannot use a microphone' },
};
/// An f32 as Rust's Display writes it ("2.5", "3", "0.1").
const f32 = (v) => String(Number.isFinite(v) ? Number(Math.fround(v).toPrecision(7)) : v);

function senses({ allows, bridge, present, facts }) {
  const fail = (payload) => new HostError(payload);
  const ask = (op, args, transfer) => {
    const answer = bridge.call(op, args || {}, transfer);
    if (answer.json.err) throw fail(answer.json.err);
    return answer;
  };

  // ------------------------------------------------------- notifications
  // As desktop_host::notify: the grant, then the text, then the service.
  const notify = {
    show(title, body) {
      if (!allows('ui.notify')) throw fail({ tag: 'denied' });
      if (!String(title).trim()) throw fail({ tag: 'invalid-content', val: 'a notification needs a title' });
      if (utf8(title) > MAX_TITLE_BYTES || utf8(body) > MAX_BODY_BYTES) throw fail({ tag: 'invalid-content', val: 'the notification text was too long' });
      if (CONTROL.test(title) || CONTROL.test(body)) throw fail({ tag: 'invalid-content', val: 'the notification text contained control characters' });
      // What the app drew last stays on screen if the person is asked.
      present();
      ask('notify.show', { title, body });
    },
  };

  // ------------------------------------------------------------ location
  // As Phase3GuiHost::location: without location.read the answer is
  // not-allowed and the browser is never asked. The first ask starts the
  // browser looking and answers finding.
  const location = {
    available: () => Boolean(facts.geolocation),
    current() {
      if (!allows('location.read')) throw fail({ tag: 'not-allowed' });
      const p = ask('location.current').json.ok;
      const opt = (v) => (v === null || v === undefined || !Number.isFinite(v) ? undefined : v);
      return {
        latitude: p.latitude,
        longitude: p.longitude,
        accuracy: p.accuracy,
        altitude: opt(p.altitude),
        speed: opt(p.speed),
        course: opt(p.course),
        ageMs: BigInt(Math.max(0, Math.round(p.age_ms))),
      };
    },
  };

  // --------------------------------------------------------------- share
  // As Phase3GuiHost::share: no capability (the person's pick in the sheet
  // is the grant), and the same checks on what is offered.
  const share = {
    share(w, text, url, files) {
      const empty = (s) => s === undefined || s === null || !String(s).trim();
      if (empty(text) && empty(url) && !files.length) throw fail({ tag: 'invalid', val: 'nothing to share' });
      for (const f of files) {
        const n = String(f.name);
        // A plain name: no folder in it, not hidden, not empty. A tab
        // refuses \ as Windows does, wherever it runs.
        if (!n.trim() || /[\/\\]/.test(n) || n === '.' || n === '..' || n.startsWith('.')) {
          throw fail({ tag: 'invalid', val: `a file name must be a plain name, not ${JSON.stringify(n)}` });
        }
      }
      const total = files.reduce((t, f) => t + f.data.length, 0) + (text ? utf8(text) : 0) + (url ? utf8(url) : 0);
      if (total > 64 * 1024 * 1024) throw fail({ tag: 'invalid', val: 'more than 64 MB to share' });
      present();
      const copies = files.map((f) => ({ name: String(f.name), data: f.data.slice() }));
      return ask('share', { text: text ?? null, url: url ?? null, files: copies }, copies.map((c) => c.data.buffer)).json.ok === true;
    },
  };

  // ------------------------------------------------------ speaking aloud
  // As the desktop's speech::synthesis host: speech is sound, so it is
  // audio.playback's to allow, and stop is always allowed.
  const sound = () => { if (!allows('audio.playback')) throw fail({ tag: 'permission-denied' }); };
  const synthesis = {
    voices() {
      sound();
      return ask('speech.voices').json.ok.map((v) => ({ id: v.id, name: v.name, language: v.language }));
    },
    say(text, voice, rate) {
      sound();
      if (!String(text).trim()) throw fail({ tag: 'invalid-request', val: 'there is no text to say' });
      const chars = [...String(text)].length;
      if (chars > MAX_TEXT_CHARS) throw fail({ tag: 'invalid-request', val: `${chars} characters is more than one say takes (${MAX_TEXT_CHARS}); split it` });
      const r = rate === undefined ? 1 : rate;
      if (!(r >= MIN_RATE && r <= MAX_RATE)) throw fail({ tag: 'invalid-request', val: `rate ${f32(r)} is outside ${f32(MIN_RATE)} to ${f32(MAX_RATE)}` });
      ask('speech.say', { text: String(text), voice: voice ?? null, rate: r });
    },
    stop() { bridge.call('speech.stop', {}); },
    speaking() {
      sound();
      return ask('speech.speaking').json.ok === true;
    },
  };

  // -------------------------------------------------------------- camera
  // As camera_capture.rs: the grant on every call but stop and close, the
  // config checked before any device is touched, at most four streams,
  // nothing captured (and the browser's camera light off) until start, the
  // newest frame only. The stream lives on the page, which has the camera.
  const camDenied = () => !allows('camera.capture');
  const camera = {
    devices() {
      if (camDenied()) throw fail({ tag: 'permission-denied' });
      return ask('camera.devices').json.ok;
    },
    open(device, config) {
      if (camDenied()) throw fail({ tag: 'permission-denied' });
      const { width, height, fps } = config;
      if (width === 0 || height === 0) throw fail({ tag: 'unsupported', val: 'width and height must both be greater than zero' });
      if (width * height > MAX_FRAME_PIXELS) throw fail({ tag: 'unsupported', val: `${width}x${height} is larger than the ${MAX_FRAME_PIXELS}-pixel limit` });
      if (fps === 0 || fps > 240) throw fail({ tag: 'unsupported', val: 'fps must be between 1 and 240' });
      return BigInt(ask('camera.open', { device: String(device), width, height, fps }).json.ok);
    },
    info(id) {
      if (camDenied()) throw fail({ tag: 'permission-denied' });
      const i = ask('camera.info', { id: id.toString() }).json.ok;
      return { width: i.width, height: i.height, fps: i.fps, format: 'rgba8' };
    },
    start(id) {
      if (camDenied()) throw fail({ tag: 'permission-denied' });
      ask('camera.start', { id: id.toString() });
    },
    // Not behind the grant: stopping and closing are how an app turns the
    // camera off.
    stop(id) { ask('camera.stop', { id: id.toString() }); },
    read(id) {
      if (camDenied()) throw fail({ tag: 'permission-denied' });
      const { json, body } = ask('camera.read', { id: id.toString() });
      if (!json.ok) return undefined;
      return { bytes: new Uint8Array(body), width: json.ok.width, height: json.ok.height, elapsedMillis: BigInt(json.ok.elapsed) };
    },
    close(id) { ask('camera.close', { id: id.toString() }); },
  };

  // ---------------------------------------------------------- microphone
  // As audio_capture.rs: the grant on every call, the config checked first,
  // nothing heard until start, the default microphone converted to exactly
  // the rate, channels and format the app asked for -- the desktop's own
  // converter (CaptureConverter), here -- and at most 8 MB kept unread,
  // the oldest dropped first. The page's audio thread writes what the
  // browser hears into shared memory; read takes it from there, with no
  // call to the page.
  const mics = new Map();
  const micDenied = () => { if (!allows('audio.capture')) throw fail({ tag: 'permission-denied' }); };
  const mic = (id) => {
    const m = mics.get(id.toString());
    if (!m) throw fail({ tag: 'invalid-stream' });
    return m;
  };
  const pushSample = (m, v) => {
    const s = Math.max(-1, Math.min(1, v));
    if (m.float) { m.dv.setFloat32(m.len, s, true); m.len += 4; } else { m.dv.setInt16(m.len, Math.trunc(s * 32767), true); m.len += 2; }
  };
  // Everything the browser heard since the last read, converted.
  const drain = (m) => {
    const written = Atomics.load(m.ctrl, MIC_WRITTEN);
    let avail = (written - m.at) | 0;
    if (avail > MIC_SAMPLES) { m.at = (written - MIC_SAMPLES) | 0; avail = MIC_SAMPLES; }
    const ch = Atomics.load(m.ctrl, MIC_CHANNELS) || 1;
    const inRate = Atomics.load(m.ctrl, MIC_RATE);
    const frames = Math.floor(avail / ch);
    if (!frames || !inRate) return;
    const most = Math.ceil((frames * m.rate) / inRate + 1) * m.channels * m.width;
    if (m.len + most > m.out.length) {
      const grown = new Uint8Array(Math.max(m.out.length * 2, m.len + most));
      grown.set(m.out.subarray(0, m.len));
      m.out = grown;
      m.dv = new DataView(grown.buffer);
    }
    const mask = MIC_SAMPLES - 1;
    const frame = new Float32Array(ch);
    let at = m.at;
    for (let i = 0; i < frames; i++) {
      for (let c = 0; c < ch; c++) { frame[c] = m.ring[at & mask]; at = (at + 1) | 0; }
      m.acc += m.rate;
      while (m.acc >= inRate) {
        m.acc -= inRate;
        if (m.channels === 1) {
          let sum = 0;
          for (let c = 0; c < ch; c++) sum += frame[c];
          pushSample(m, sum / ch);
        } else {
          for (let c = 0; c < m.channels; c++) pushSample(m, frame[ch === 1 ? 0 : Math.min(c, ch - 1)]);
        }
      }
    }
    m.at = at;
    // Bounded as on desktop: the newest 8 MB, whole samples.
    if (m.len > MAX_CAPTURE_BUFFER_BYTES) {
      const drop = m.len - MAX_CAPTURE_BUFFER_BYTES;
      m.out.copyWithin(0, drop, m.len);
      m.len -= drop;
    }
  };
  const capture = {
    open(config) {
      micDenied();
      const { sampleRate, channels, format, bufferFrames } = config;
      if (!(sampleRate >= MIN_SAMPLE_RATE && sampleRate <= MAX_SAMPLE_RATE)) throw fail({ tag: 'unsupported', val: `sample rate must be between ${MIN_SAMPLE_RATE} and ${MAX_SAMPLE_RATE} Hz` });
      if (!(channels >= 1 && channels <= MAX_CHANNELS)) throw fail({ tag: 'unsupported', val: `channels must be between 1 and ${MAX_CHANNELS}` });
      if (!(bufferFrames > 0)) throw fail({ tag: 'unsupported', val: 'buffer-frames must be greater than zero' });
      const ring = new SharedArrayBuffer(MIC_SAMPLES * 4);
      const ctrl = new SharedArrayBuffer(MIC_WORDS * 4);
      const id = ask('mic.open', { ring, ctrl }).json.ok;
      const float = format === 'float32';
      const out = new Uint8Array(1 << 16);
      mics.set(String(id), {
        ring: new Float32Array(ring), ctrl: new Int32Array(ctrl), at: 0, acc: 0, started: false,
        rate: sampleRate, channels, float, width: float ? 4 : 2, out, dv: new DataView(out.buffer), len: 0,
      });
      return BigInt(id);
    },
    start(id) {
      micDenied();
      const m = mic(id);
      ask('mic.start', { id: id.toString() });
      m.started = true;
    },
    stop(id) {
      micDenied();
      const m = mic(id);
      ask('mic.stop', { id: id.toString() });
      m.started = false;
    },
    read(id, maxBytes) {
      micDenied();
      const m = mic(id);
      const failed = MIC_FAILURES[Atomics.load(m.ctrl, MIC_FAILED)];
      if (failed) throw fail(failed);
      if (!m.started) return new Uint8Array(0);
      drain(m);
      const n = Math.min(maxBytes, MAX_READ_BYTES, m.len);
      const out = m.out.slice(0, n);
      m.out.copyWithin(0, n, m.len);
      m.len -= n;
      return out;
    },
  };

  return {
    'krate:ui/notify': notify,
    'krate:ui/location': location,
    'krate:ui/share': share,
    'krate:speech/synthesis': synthesis,
    'krate:camera/types': {},
    'krate:camera/capture': camera,
    'krate:audio/capture': capture,
  };
}
