// An organisation's team data for an app in this tab (store.org, G1).
//
// The same thing Krate on a computer does (crates/runtime org_store.rs), in
// the page: every rule -- the device's copy, which write wins, the limits,
// the sealing and the opening -- is crates/orgdata compiled into the hub's
// validator module (krate_org_data). This file only keeps the copy in
// IndexedDB, talks to the hub with the person's session, and keeps the
// state an app and the page show: synced, syncing, offline, or waiting for
// the key.
//
// The key: this tab is one of the organisation's devices, a "browser" in
// its device list. Its X25519 key is made by WebCrypto and kept sealed in
// IndexedDB (kit/org-device.js, the owner's browser's own code). It holds
// the organisation key once another member's device passes it on -- a
// computer running Krate does that in the background, so does any tab that
// holds it -- and until then writes wait here, as they do offline.
//
// The app's worker is blocked while it waits for an answer, so everything
// async happens here and the worker asks over the bridge (bridge.js).

/// How often a sync runs while the app is open.
export const BACKGROUND_EVERY = 5000;
/// An app asking faster than this, with nothing to send, is answered from here.
export const SYNC_FLOOR = 2000;
/// Rounds of push and pull in one sync before it stops for now.
const MAX_ROUNDS = 64;
/// The most writes one round seals (crates/orgdata PUSH_WRITES).
const PUSH_WRITES = 1000;

const hex = (bytes) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');

/// The error an app sees, in the WIT org-error's shape.
const io = (words) => ({ tag: 'io', val: words });

/// Refusals that end access for the run (as on a computer), and the ones
/// that also mean the person is no longer in the organisation: its data
/// leaves this browser.
const ENDS = new Set([401, 403, 404, 410]);
const ends_membership = (reason) => reason === 'removed' || reason === 'not-a-member';

export class TeamData {
  /// o: { mod, signing, hub, token, slug, appId, digest, me, store, device,
  ///      now?, fetch?, onState?, windowed? }
  ///   mod      KrateSigning.loadModule(validator module): keys() and data()
  ///   signing  window.KrateSigning (refresh)
  ///   store    {get, put, del}: where the copy is kept (IndexedDB)
  ///   device   KrateSigning.openDevice(...): this browser's key
  ///   windowed a windowed app's sync never waits (K-1107); a command-line
  ///            app's does
  constructor(o) {
    Object.assign(this, {
      now: () => Date.now(),
      fetch: (...a) => globalThis.fetch(...a),
      onState: () => {},
      windowed: false,
    }, o);
    this.appId = String(o.appId || '').toLowerCase();
    this.key = `data/${o.slug}/${this.appId}/${o.me || ''}`;
    this.name = `${o.slug}/${this.appId}`;
    this.refused = null;
    this.lastOnline = false;
    this.lastSync = 0;
    this.unseen = false;
    this.tried = false;
    this.syncing = false;
    this.keyState = 'unknown';
    this.reachedAt = undefined;
    this.orgName = o.orgName || o.slug;
    this.chain = Promise.resolve();
    this.timer = null;
    this.writes = Promise.resolve();
  }

  /// Load this browser's copy (or start one).
  async open() {
    const kept = await this.store.get(this.key).catch(() => undefined);
    const device = hex(crypto.getRandomValues(new Uint8Array(8)));
    const a = this.mod.data({ op: 'open', store: this.name, mirror: kept || null, device });
    if (!a.ok) throw new Error(a.error && a.error.val ? a.error.val : 'the team data did not open');
    if (a.mirror) await this.keep(a.mirror);
    if (a.envelope) this.keyState = 'holds';
    this.emit();
    return this;
  }

  /// Keep the copy, in the order the changes were made.
  keep(text) {
    this.writes = this.writes.then(() => this.store.put(this.key, text));
    return this.writes;
  }

  emit() {
    try { this.onState(this.state()); } catch (_) { /* the page's own problem */ }
  }

  status() {
    const s = this.mod.data({ op: 'status', store: this.name });
    return s.ok ? s : { pending: 0, last_synced: null };
  }

  /// Where this browser stands: the WIT `link` (K-1108), plus `refused` and
  /// `no-key` for the page's own words.
  state() {
    const s = this.status();
    let link;
    if (this.refused) link = 'refused';
    else if (this.tried && !this.lastOnline) link = 'offline';
    else if (this.keyState === 'no-key') link = 'no-key';
    else if (this.keyState === 'waiting') link = 'waiting-for-key';
    else if (!this.tried || this.syncing || s.pending > 0) link = 'syncing';
    else link = 'synced';
    return { link, pending: s.pending, lastSynced: s.last_synced ?? undefined, reachedAt: this.reachedAt, refused: this.refused, org: this.orgName };
  }

  check() {
    return this.refused ? { tag: 'refused', val: { reason: this.refused.reason, message: this.refused.message } } : null;
  }

  // ------------------------------------------------------------ the app's calls

  async put(collection, id, fields) {
    if (!fields.length) { const r = this.check(); return r ? { err: r } : { ok: null }; }
    if (fields.some(([name]) => name === '')) return { err: { tag: 'invalid', val: 'a field name cannot be empty' } };
    const r = this.check();
    if (r) return { err: r };
    return this.write({ op: 'put', collection, id, fields });
  }

  async delete(collection, id) {
    const r = this.check();
    if (r) return { err: r };
    return this.write({ op: 'delete', collection, id });
  }

  async write(op) {
    const a = this.mod.data({ ...op, store: this.name, now: this.now(), me: this.me });
    if (!a.ok) return { err: a.error };
    if (a.mirror) {
      try { await this.keep(a.mirror); } catch (e) { return { err: io(`this browser did not save it (${(e && e.message) || e})`) }; }
    }
    this.emit();
    return { ok: null };
  }

  get(collection, id) {
    const r = this.check();
    if (r) return { err: r };
    const a = this.mod.data({ op: 'get', store: this.name, collection, id });
    return a.ok ? { ok: a.entry } : { err: a.error };
  }

  records(collection, since) {
    const r = this.check();
    if (r) return { err: r };
    const a = this.mod.data({ op: 'records', store: this.name, collection, since: since ?? null });
    return a.ok ? { ok: a.page } : { err: a.error };
  }

  who() {
    const r = this.check();
    if (r) return { err: r };
    const a = this.mod.data({ op: 'who', store: this.name });
    return a.ok ? { ok: a.member } : { err: a.error };
  }

  /// `store.org.status` (K-1108): never waits, never reaches the network.
  standing() {
    const r = this.check();
    if (r) return { err: r };
    const s = this.state();
    const link = ['synced', 'syncing', 'offline', 'waiting-for-key'].includes(s.link) ? s.link : 'waiting-for-key';
    return { ok: { link, pending: s.pending, reached_at: s.reachedAt, org: s.org } };
  }

  /// `store.org.sync`. A windowed app's never waits: it asks for a sync now
  /// and answers with where this browser stands, and `changed` comes on a
  /// later call (K-1107). A command-line app's waits for the exchange.
  /// Faster than every two seconds with nothing to send is answered from
  /// here.
  async sync() {
    const r = this.check();
    if (r) return { err: r };
    const answer = (state) => {
      const changed = this.unseen;
      this.unseen = false;
      return { ok: { online: state ? state.online : this.lastOnline, changed, pending: this.status().pending, last_synced: this.status().last_synced ?? null } };
    };
    if (this.windowed) {
      this.kick();
      return answer();
    }
    await this.chain.catch(() => {});
    if (this.lastSync && this.now() - this.lastSync < SYNC_FLOOR && this.status().pending === 0) return answer();
    try {
      const state = await this.run();
      return answer(state);
    } catch (e) {
      return { err: e.org || io(String((e && e.message) || e)) };
    }
  }

  // ------------------------------------------------------------- the sync

  /// One sync, after any under way.
  run() {
    const next = this.chain.catch(() => {}).then(() => this.rounds());
    this.chain = next.catch(() => {});
    return next;
  }

  kick() {
    if (this.refused) return;
    this.run().catch(() => {});
  }

  start() {
    if (this.timer || this.refused) return;
    this.kick();
    this.timer = setInterval(() => { if (this.refused) this.stop(); else this.kick(); }, BACKGROUND_EVERY);
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  fail(org) {
    const e = new Error(org.val && org.val.message ? org.val.message : org.val || org.tag);
    e.org = org;
    return e;
  }

  /// The hub refused this person or this version: nothing is served for
  /// the rest of the run, and someone no longer in the organisation keeps
  /// none of its data.
  async refuse(reason, message) {
    reason = reason || 'not-a-member';
    if (ends_membership(reason)) {
      const device = hex(crypto.getRandomValues(new Uint8Array(8)));
      this.mod.data({ op: 'reset', store: this.name, device });
      await this.store.del(this.key).catch(() => {});
    }
    this.refused = { reason, message: message || '' };
    this.lastOnline = true;
    this.stop();
    return this.fail({ tag: 'refused', val: { reason, message: message || '' } });
  }

  /// This browser's hold of the organisation key, brought up to date.
  /// The envelope, or null while waiting, with no key yet, or offline.
  async refreshKey() {
    const held = this.status().envelope || null;
    let got;
    try {
      got = await this.signing.refresh({ hub: this.hub, token: this.token, slug: this.slug, device: this.device, mod: this.mod, held });
    } catch (e) {
      if (e.reason === 'offline') {
        this.lastOnline = false;
        return null;
      }
      if (ENDS.has(e.status)) throw await this.refuse(e.reason === 'refused' ? '' : e.reason, e.message);
      if (e.status === 429 || (e.status && e.status >= 500)) {
        this.lastOnline = false;
        return null;
      }
      throw this.fail(io(`the organisation's key: ${(e && e.message) || e}`));
    }
    this.lastOnline = true;
    if (got.status !== 'holds') {
      this.keyState = got.status === 'no-key' ? 'no-key' : 'waiting';
      return null;
    }
    this.keyState = 'holds';
    if (got.envelope !== held) {
      const a = this.mod.data({ op: 'set_envelope', store: this.name, envelope: got.envelope });
      if (a.ok && a.changed) await this.keep(a.mirror);
    }
    return got.envelope;
  }

  async rounds() {
    if (this.refused) throw this.fail(this.check());
    this.syncing = true;
    this.emit();
    try {
      return await this.roundsInner();
    } finally {
      this.syncing = false;
      this.tried = true;
      this.lastSync = this.now();
      this.emit();
    }
  }

  async roundsInner() {
    const state = (online, changed = false) => ({ online, changed, pending: this.status().pending });
    let envelope = await this.refreshKey();
    if (!envelope) return state(this.lastOnline);
    const secret = await this.device.secret();
    const keys = { store: this.name, org: this.slug, app_id: this.appId, envelope, device_secret: secret };
    const url = `${this.hub.replace(/\/$/, '')}/orgs/${this.slug}/data/${this.digest}/sync`;
    let changed = false;
    let online = false;
    let keyTries = 0;
    for (let round = 0; round < MAX_ROUNDS; round += 1) {
      if (this.refused) throw this.fail(this.check());
      const pending = Math.min(this.status().pending, PUSH_WRITES);
      const begin = this.mod.data({ ...keys, envelope, op: 'sync_begin', now: this.now() }, 24 * pending + 64);
      if (!begin.ok) throw this.fail(begin.error);
      let res;
      try {
        res = await this.fetch(url, {
          method: 'POST',
          headers: { 'content-type': 'application/json', accept: 'application/json', authorization: `Bearer ${this.token}` },
          body: JSON.stringify(begin.body),
        });
      } catch (_) {
        this.mod.data({ op: 'sync_abandon', store: this.name });
        this.lastOnline = false;
        return state(false, changed);
      }
      if (!res.ok) {
        this.mod.data({ op: 'sync_abandon', store: this.name });
        const body = await res.json().catch(() => ({}));
        const reason = body.error || '';
        if (ENDS.has(res.status)) throw await this.refuse(reason, body.message);
        // The key moved on (someone left): take the new version, then send
        // again under it.
        if (res.status === 409 && reason === 'key-rotated' && keyTries < 2) {
          keyTries += 1;
          envelope = await this.refreshKey();
          if (!envelope) return state(true, changed);
          continue;
        }
        if (res.status === 409) return state(true, changed);
        if (res.status === 413) throw this.fail(reason === 'over-quota' ? { tag: 'over-quota' } : { tag: 'too-large' });
        if (res.status === 400) throw this.fail(io(body.message || 'the hub did not take this sync'));
        // Busy, rate limited, down: offline for now.
        this.lastOnline = false;
        return state(false, changed);
      }
      const answer = await res.json().catch(() => null);
      if (!answer) {
        this.mod.data({ op: 'sync_abandon', store: this.name });
        this.lastOnline = false;
        return state(false, changed);
      }
      online = true;
      this.lastOnline = true;
      this.reachedAt = answer.now;
      if (answer.org && answer.org.name) this.orgName = answer.org.name;
      const end = this.mod.data({ ...keys, envelope, op: 'sync_end', answer });
      if (!end.ok) throw this.fail(end.error);
      await this.keep(end.mirror);
      if (end.changed) {
        changed = true;
        this.unseen = true;
      }
      if (!end.again) break;
    }
    return state(online, changed);
  }
}
