// An app's shared store in this tab (krate:store/shared, IC-974).
//
// The same thing Krate on a computer does (crates/runtime/src/shared_host.rs),
// rule for rule: a key-value bucket shared between everyone who holds its
// ten-character invite code, kept local-first. get/set/delete/keys always
// work against this browser's copy; sync exchanges changes with the hub
// (krate.tech, the origin this page came from) when the network allows.
// Merging is last-writer-wins per key by the writer's clock, with
// tombstones for deletes. Offline is a normal state, never an error.
//
// The copy is one JSON value in IndexedDB, in the desktop file's own shape
// ({code, kv: {key: {v, t, dirty}}}), under the app's store key and the
// number 1, which no store.kv key (always a string) can be or reach.
//
// The app's worker is blocked while it waits for an answer, so the copy
// lives here on the page, and a windowed app's sync never waits for the
// hub (K-1121): it starts the exchange here and answers at once with
// whether an earlier exchange changed the copy. A command-line app's sync
// waits, as a script that syncs and exits expects.

/// Bounds matching the desktop's and the hub's: a list, not a database.
export const MAX_KEY = 128;
export const MAX_VALUE = 64 * 1024;
/// The least time between network syncs; a faster caller hears the local
/// answer instead of reaching the hub.
export const SYNC_FLOOR = 2000;
/// How long one request to the hub may take (the desktop's 8 s).
const TIMEOUT = 8000;

const enc = new TextEncoder();

/// A failed call, carrying the WIT shared-error it answers with.
export class SharedFail extends Error {
  constructor(payload) {
    super(payload.val || payload.tag);
    this.payload = payload;
  }
}
const io = (words) => new SharedFail({ tag: 'io', val: words });

/// Keys in the order the desktop lists them (a BTreeMap of Rust strings:
/// by code point, which JavaScript's own sort is not past U+FFFF).
export function byCodePoint(a, b) {
  const x = [...a];
  const y = [...b];
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const d = x[i].codePointAt(0) - y[i].codePointAt(0);
    if (d) return d;
  }
  return x.length - y.length;
}

/// Rust's char::is_control: the Unicode Cc category.
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

function checkKey(key) {
  if (typeof key !== 'string' || key.length === 0 || enc.encode(key).length > MAX_KEY || CONTROL.test(key)) {
    throw new SharedFail({ tag: 'invalid-name' });
  }
}

const toB64 = (bytes) => {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
};
/// A value that is not base64 reads as absent, as on desktop.
const fromB64 = (text) => {
  try { return Uint8Array.from(atob(text.replace(/\n/g, '')), (c) => c.charCodeAt(0)); } catch { return undefined; }
};

/// The desktop reads a clock as a u64; anything else is 0.
const clock = (t) => (typeof t === 'number' && Number.isInteger(t) && t >= 0 ? t : 0);

export class SharedStore {
  /// o: { hub, load(), save(text), windowed, fetch?, now? }
  ///   hub      where the shares live (this page's own origin)
  ///   load     the kept copy as text, or nothing
  ///   save     keep the copy; resolves once the browser has it
  ///   windowed a windowed app's sync never waits for the hub
  constructor(o) {
    Object.assign(this, {
      fetch: (...a) => globalThis.fetch(...a),
      now: () => Date.now(),
      windowed: false,
    }, o);
    this.state = { code: null, kv: {} };
    this.lastSync = null;
    this.unseenChange = false;
    this.unseenError = null;
    this.inFlight = false;
    this.saving = Promise.resolve();
  }

  /// Load this browser's copy (or start one). A copy that cannot be read
  /// starts empty, as the desktop's does.
  async open() {
    try {
      const text = await this.load();
      const kept = text ? JSON.parse(text) : null;
      if (kept && typeof kept === 'object') {
        this.state = { code: typeof kept.code === 'string' ? kept.code : null, kv: kept.kv && typeof kept.kv === 'object' ? kept.kv : {} };
      }
    } catch { /* an unreadable copy is a fresh one */ }
    return this;
  }

  /// Keep the copy, in the order the changes were made. A write the
  /// browser refused is the app's error, as a full disk is on desktop.
  keep() {
    const text = JSON.stringify(this.state);
    const done = this.saving.then(() => this.save(text));
    this.saving = done.catch(() => {});
    return done.catch((e) => { throw io(`the browser did not keep the shared copy (${(e && e.message) || e})`); });
  }

  code() { return this.state.code; }

  get(key) {
    checkKey(key);
    const entry = this.state.kv[key];
    return entry && typeof entry.v === 'string' ? fromB64(entry.v) : undefined;
  }

  async set(key, value) {
    checkKey(key);
    if (value.length > MAX_VALUE) throw new SharedFail({ tag: 'too-large' });
    this.state.kv[key] = { v: toB64(value), t: this.now(), dirty: true };
    await this.keep();
  }

  async delete(key) {
    checkKey(key);
    // A tombstone, not an absence: without one, the other machine's next
    // push would resurrect every item ever removed.
    this.state.kv[key] = { v: null, t: this.now(), dirty: true };
    await this.keep();
  }

  keys() {
    return Object.keys(this.state.kv).filter((k) => typeof this.state.kv[k].v === 'string').sort(byCodePoint);
  }

  /// One request to the hub, the desktop's 8 s at most. What comes back is
  /// the bucket, a 404 (no such share), or an io error in plain words.
  async ask(path, method, body) {
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), TIMEOUT);
    let res;
    try {
      res = await this.fetch(`${this.hub}${path}`, {
        method,
        headers: body ? { 'content-type': 'application/json' } : undefined,
        body,
        credentials: 'omit',
        cache: 'no-store',
        signal: abort.signal,
      });
    } catch (e) {
      throw io(abort.signal.aborted ? 'krate.tech did not answer in 8 seconds' : 'could not reach krate.tech; check the connection');
    } finally {
      clearTimeout(timer);
    }
    if (res.status === 404) throw new SharedFail({ tag: 'no-such-share' });
    if (res.status === 429) throw io('krate.tech was asked too often from here; try again in a minute');
    if (!res.ok) throw io(`krate.tech answered ${res.status}`);
    try { return await res.json(); } catch { throw io('krate.tech answered with something that is not a share'); }
  }

  /// Start a fresh share; the code is minted by krate.tech so it is unique.
  async create() {
    const body = await this.ask('/share/new', 'POST');
    if (!body || typeof body.code !== 'string') throw io('the hub returned no code');
    this.state.code = body.code;
    await this.keep();
    // Whatever was written before the share existed rides up with the
    // first sync -- create-then-fill and fill-then-create both work.
    await this.kick();
    return body.code;
  }

  /// Join a share by its code. Proved to exist before it is remembered, so
  /// a typo answers now rather than as eternal silent sync failure.
  async join(asked) {
    const code = String(asked).trim().toLowerCase();
    if (code.length !== 10 || !/^[a-z0-9]+$/.test(code)) throw new SharedFail({ tag: 'no-such-share' });
    const remote = await this.ask(`/share/${code}`, 'GET');
    this.state.code = code;
    this.mergeRemote(remote);
    await this.keep();
    await this.kick();
  }

  /// Leave: keeps the local copy, stops syncing, forgets the code.
  async leave() {
    this.state.code = null;
    await this.keep();
  }

  /// Exchange changes with the hub. `true` when the LOCAL copy changed;
  /// offline answers `false` and the queue keeps waiting.
  async sync() {
    if (this.state.code === null) throw new SharedFail({ tag: 'not-joined' });
    const floor = this.lastSync !== null && this.now() - this.lastSync < SYNC_FLOOR;
    if (this.windowed) {
      if (this.unseenError) {
        const e = this.unseenError;
        this.unseenError = null;
        throw e;
      }
      const changed = this.unseenChange;
      this.unseenChange = false;
      if (!floor) this.startExchange();
      return changed;
    }
    if (floor) return false;
    try {
      return await this.exchange();
    } catch (e) {
      // The network being away is a normal state for a laptop, and a phone.
      if (e instanceof SharedFail && e.payload.tag === 'io') return false;
      throw e;
    }
  }

  /// One exchange now: in the background for a windowed app, waited for
  /// otherwise. Its answer reaches the app through the next sync.
  async kick() {
    if (!this.windowed) {
      try { if (await this.exchange()) this.unseenChange = true; } catch { /* heard at the next sync */ }
    } else {
      this.startExchange();
    }
  }

  /// A background exchange, unless one is already under way.
  startExchange() {
    if (this.inFlight) return;
    this.inFlight = true;
    this.exchange().then(
      (changed) => { if (changed) this.unseenChange = true; },
      (e) => {
        const tag = e instanceof SharedFail ? e.payload.tag : 'io';
        if (tag !== 'io') this.unseenError = e;
      },
    ).finally(() => { this.inFlight = false; });
  }

  /// One push-and-pull with the hub: what to send is read as the copy
  /// stands now, and the answer folded in when it comes.
  async exchange() {
    const sent = this.outgoing();
    const remote = sent.writes.length
      ? await this.ask(sent.path, 'PUT', JSON.stringify({ writes: sent.writes.map(([key, v, t]) => ({ key, v, t })) }))
      : await this.ask(sent.path, 'GET');
    return this.finish(sent, remote);
  }

  outgoing() {
    const code = this.state.code;
    if (code === null) throw new SharedFail({ tag: 'not-joined' });
    this.lastSync = this.now();
    const writes = Object.keys(this.state.kv).sort(byCodePoint)
      .filter((k) => this.state.kv[k].dirty)
      .map((k) => [k, this.state.kv[k].v, this.state.kv[k].t]);
    return { path: `/share/${code}`, writes };
  }

  /// Only the writes that went up are clean now: one the app made while
  /// the exchange was on the network is newer, and waits for the next push.
  async finish(sent, remote) {
    for (const [key, , t] of sent.writes) {
      const entry = this.state.kv[key];
      if (entry && entry.t === t) entry.dirty = false;
    }
    const changed = this.mergeRemote(remote);
    await this.keep();
    return changed;
  }

  /// Fold the hub's bucket into the copy. Newest write per key wins; a
  /// local write with a newer clock survives to be pushed next time.
  mergeRemote(remote) {
    const kv = remote && remote.kv;
    if (!kv || typeof kv !== 'object' || Array.isArray(kv)) return false;
    let changed = false;
    for (const [key, value] of Object.entries(kv)) {
      const t = clock(value && value.t);
      const v = value && typeof value.v === 'string' ? value.v : null;
      const existing = this.state.kv[key];
      if (existing && existing.t >= t) continue;
      this.state.kv[key] = { v, t, dirty: false };
      changed = true;
    }
    return changed;
  }
}
