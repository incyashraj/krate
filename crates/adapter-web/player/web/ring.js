// The page's input to a Krate app blocked in a Web Worker (IC-956).
//
// A ring of fixed slots in a SharedArrayBuffer: the page writes a record
// and bumps `head`, then Atomics.notify; the worker, blocked in
// events.wait, wakes on `head` changing, reads, and bumps `tail`. A record
// is one JSON object (a press, a key, text, a resize). The ring holds
// SLOTS records; when it is full the page keeps the rest in order and
// writes them as the worker drains, so nothing typed is lost.

export const SLOTS = 256;
export const SLOT = 1024;
const HEAD = 64;

/* Whether the browser keeps its own meaning for a key the app was sent.
 * Every key goes to the app. Plain keys and the shortcuts an app draws
 * its own answer for (Save, Open, Print, Find, Select All, Undo) are the
 * app's, so the browser's dialog does not open over it (K-1196). Copy,
 * cut, paste and the tab's own keys (new tab, close, reload, address
 * bar, zoom) stay the browser's: a copy or paste event needs the
 * browser's default, and a tab you cannot close is worse. */
const APP_CHORDS = /^[sopfgdbjekazy]$/i;
export function browserKeepsKey(e) {
  if (!e.ctrlKey && !e.metaKey) return false;
  if (e.altKey) return true;
  return !APP_CHORDS.test(e.key);
}

export function makeRing() {
  return new SharedArrayBuffer(HEAD + SLOTS * SLOT);
}

const enc = new TextEncoder();
const dec = new TextDecoder();

export function pageRing(buffer) {
  const ctrl = new Int32Array(buffer, 0, 2);
  const bytes = new Uint8Array(buffer);
  const dv = new DataView(buffer);
  const backlog = [];
  const flush = () => {
    while (backlog.length) {
      const head = Atomics.load(ctrl, 0);
      const tail = Atomics.load(ctrl, 1);
      if (head - tail >= SLOTS) return false;
      const b = enc.encode(JSON.stringify(backlog[0]));
      backlog.shift();
      const off = HEAD + (head % SLOTS) * SLOT;
      dv.setUint32(off, b.length, true);
      bytes.set(b, off + 4);
      Atomics.store(ctrl, 0, head + 1);
      Atomics.notify(ctrl, 0);
    }
    return true;
  };
  return {
    push(record) {
      // A record must fit a slot: long text is split into pieces.
      if (record.type === 'text' && enc.encode(record.text).length > SLOT - 128) {
        for (const piece of record.text.match(/[\s\S]{1,200}/gu) || []) {
          backlog.push({ ...record, text: piece });
        }
      } else {
        backlog.push(record);
      }
      if (!flush()) setTimeout(function retry() { if (!flush()) setTimeout(retry, 4); }, 4);
    },
  };
}

export function workerRing(buffer) {
  const ctrl = new Int32Array(buffer, 0, 2);
  const bytes = new Uint8Array(buffer);
  const dv = new DataView(buffer);
  return {
    // One record, waiting up to `timeout` ms (undefined: forever, 0: not at
    // all). Returns undefined when nothing came.
    take(timeout) {
      let head = Atomics.load(ctrl, 0);
      const tail = Atomics.load(ctrl, 1);
      if (head === tail) {
        if (timeout === 0) return undefined;
        Atomics.wait(ctrl, 0, head, timeout === undefined ? Infinity : timeout);
        head = Atomics.load(ctrl, 0);
        if (head === tail) return undefined;
      }
      const off = HEAD + (tail % SLOTS) * SLOT;
      const len = dv.getUint32(off, true);
      // TextDecoder refuses a view of shared memory, so copy the slot out.
      const record = JSON.parse(dec.decode(bytes.slice(off + 4, off + 4 + len)));
      Atomics.store(ctrl, 1, tail + 1);
      return record;
    },
  };
}
