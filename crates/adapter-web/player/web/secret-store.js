// An app's secrets in this tab (krate:store/secret, IC-974).
//
// The same contract Krate on a computer keeps (crates/runtime/src/
// secret_host.rs): one app's secrets -- a sign-in token, an API key --
// encrypted at rest, names listed but never values in bulk, the same
// bounds and the same refusals.
//
// The key is a WebCrypto AES-GCM key made in this browser, one per app,
// created non-extractable: page script can ask the browser to seal and
// open with it, and can never read it out. It is kept in IndexedDB beside
// what it seals, under the app's store key and the number 3; the sealed
// secrets are one value under the number 2, names included, so nothing in
// the browser's storage says what the app keeps. Neither number is a
// string, so store.kv's clear never reaches them.
//
// What this protects, as on desktop: what the browser keeps on disk is
// never the secret in plain text, and one app's secrets do not open with
// another's key. What it does not, also as on desktop: code already
// running on this site in this browser, which can ask the browser to use
// the key exactly as the Player does. And unlike a computer, the browser
// decides how long its storage lives: clearing this site's data, or
// Safari after seven days of use without opening the link, removes the
// secrets with everything else, and the app asks the person again.

export const MAX_NAME_BYTES = 256;
export const MAX_SECRET_BYTES = 64 * 1024;

const enc = new TextEncoder();
const dec = new TextDecoder();

/// A failed call, carrying the WIT secret-error it answers with.
export class SecretFail extends Error {
  constructor(payload) {
    super(payload.val || payload.tag);
    this.payload = payload;
  }
}
const io = (words) => new SecretFail({ tag: 'io', val: words });

function validateName(name) {
  if (typeof name !== 'string' || name.length === 0 || enc.encode(name).length > MAX_NAME_BYTES) {
    throw new SecretFail({ tag: 'invalid-name' });
  }
  if (name.includes('/') || name.includes('\\') || name.includes('..') || /[\u0000-\u001f\u007f-\u009f]/.test(name)) {
    throw new SecretFail({ tag: 'invalid-name' });
  }
}

const toB64 = (bytes) => {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return btoa(s);
};
const fromB64 = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

/// Names in the desktop's order (a BTreeMap: by code point).
const byCodePoint = (a, b) => {
  const x = [...a];
  const y = [...b];
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const d = x[i].codePointAt(0) - y[i].codePointAt(0);
    if (d) return d;
  }
  return x.length - y.length;
};

export class SecretStore {
  /// o: { storeKey, getKey(), addKey(key), getSealed(), putSealed(value), subtle? }
  ///   getKey / addKey      the app's CryptoKey; addKey fails if one is there
  ///   getSealed / putSealed the sealed secrets, one value
  constructor(o) {
    Object.assign(this, { subtle: globalThis.crypto && globalThis.crypto.subtle }, o);
    this.entries = new Map();
    this.changed = new Set();
    this.deleted = new Set();
    this.key = null;
    this.unreadable = null;
    this.flushing = Promise.resolve();
    // Sealed for this app only: another app's sealed value placed here
    // does not open.
    this.aad = enc.encode(`krate:store/secret ${o.storeKey}`);
  }

  /// Open (or start) the app's secrets. A store that is there and cannot
  /// be opened is not an empty store: every call says so, and nothing is
  /// written over it.
  async open() {
    try {
      if (!this.subtle) throw new Error('this browser has no Web Crypto here');
      this.key = await this.getKey();
      if (!this.key) {
        const made = await this.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
        // Two tabs of one app opening at once: the first key kept wins.
        try { await this.addKey(made); this.key = made; } catch { this.key = await this.getKey(); }
        if (!this.key) throw new Error('the browser did not keep the key');
      }
      const sealed = await this.getSealed();
      if (sealed) this.entries = await this.unseal(sealed);
    } catch (e) {
      this.unreadable = String((e && e.message) || e) || 'the browser could not open them';
    }
    return this;
  }

  async unseal(sealed) {
    if (!sealed || sealed.v !== 1 || !sealed.iv || !sealed.data) throw new Error('they are not in a form this Player reads');
    let plain;
    try {
      plain = await this.subtle.decrypt({ name: 'AES-GCM', iv: sealed.iv, additionalData: this.aad }, this.key, sealed.data);
    } catch {
      throw new Error('they do not open with this app\'s key');
    }
    const map = JSON.parse(dec.decode(plain));
    return new Map(Object.entries(map).map(([name, b64]) => [name, fromB64(b64)]));
  }

  async seal(entries) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const plain = enc.encode(JSON.stringify(Object.fromEntries([...entries].map(([name, bytes]) => [name, toB64(bytes)]))));
    const data = await this.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: this.aad }, this.key, plain);
    return { v: 1, iv, data };
  }

  requireReadable() {
    if (this.unreadable) {
      throw io(`this app's saved secrets could not be read in this browser (${this.unreadable}). They have not been changed.`);
    }
  }

  get(name) {
    this.requireReadable();
    validateName(name);
    const v = this.entries.get(name);
    return v ? v.slice() : undefined;
  }

  async set(name, secret) {
    this.requireReadable();
    validateName(name);
    if (secret.length > MAX_SECRET_BYTES) throw new SecretFail({ tag: 'too-large' });
    this.entries.set(name, secret.slice());
    this.changed.add(name);
    this.deleted.delete(name);
    await this.flush();
  }

  async delete(name) {
    this.requireReadable();
    validateName(name);
    if (this.entries.delete(name)) {
      this.deleted.add(name);
      this.changed.delete(name);
      await this.flush();
    }
  }

  names() {
    this.requireReadable();
    return [...this.entries.keys()].sort(byCodePoint);
  }

  /// Save this tab's changes without erasing another tab's (K-878): what
  /// the browser holds is opened again, and only the names this tab set or
  /// deleted change.
  flush() {
    const run = this.flushing.then(async () => {
      let merged;
      try {
        const sealed = await this.getSealed();
        merged = sealed ? await this.unseal(sealed) : new Map();
      } catch {
        // As at open: what does not open under this app's key was never
        // this app's to keep.
        merged = new Map();
      }
      for (const name of this.deleted) merged.delete(name);
      for (const name of this.changed) {
        const v = this.entries.get(name);
        if (v) merged.set(name, v);
      }
      let sealed;
      try { sealed = await this.seal(merged); } catch (e) { throw io(`the browser could not seal the secret (${(e && e.message) || e}); it was not saved`); }
      try { await this.putSealed(sealed); } catch (e) { throw io(`the browser did not save it (${(e && e.message) || e})`); }
    });
    this.flushing = run.catch(() => {});
    return run;
  }
}
