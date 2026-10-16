// The app's own thread in a tab (IC-956 Phase 1).
//
// The page judged the bundle, checked that every core module it was handed
// is one of the judged component's own, and asked the person. This worker
// then runs the app: the hub-made jco glue, the verified core modules, the
// Player module (crates/adapter-web) for every rule, and hosts.js between
// them. run() blocks here -- in events.wait on the input ring, in a bridge
// call on the page's answer -- exactly as a desktop app blocks.

import init, { Ui, Grants, Kv } from './krate_web.js';
import { buildImports, AppStopped } from './hosts.js';
import { workerBridge } from './bridge.js';
import { workerRing } from './ring.js';

const post = (msg, transfer) => self.postMessage(msg, transfer || []);
const now = () => performance.timeOrigin + performance.now();

self.onmessage = async ({ data }) => {
  if (data.t !== 'start') return;
  const marks = { workerStart: now() };
  try {
    const player = await init({ module_or_path: data.playerModule });
    marks.playerReady = now();
    const ui = new Ui(data.scale, data.dark);
    const grants = new Grants(JSON.stringify(data.granted));
    const kv = new Kv(grants.allows('store.kv'));
    for (const [key, value] of data.kv || []) kv.load(key, value);

    const ring = workerRing(data.ring);
    const bridge = workerBridge(data.bridge, post);
    const windows = new Map();
    const signal = data.signal ? new Int32Array(data.signal) : null;
    let pendingInputAt;
    const noticed = new Set();

    // A window's frame goes to the page through shared memory: a blocked
    // worker's OffscreenCanvas never reaches the screen (measured in Phase 0).
    const attachFrame = (id) => {
      const size = ui.frame_size(id);
      if (!size.length) return;
      const [pw, ph, lw, lh] = size;
      const frame = new SharedArrayBuffer(pw * ph * 4);
      windows.set(id, { ...(windows.get(id) || {}), frame, pw, ph });
      post({ t: 'window', id, frame, pw, ph, lw, lh });
    };
    const present = () => {
      for (const [id, w] of windows) {
        if (!ui.render(id)) continue;
        const len = ui.frame_len(id);
        const view = new Uint8Array(player.memory.buffer, ui.frame_ptr(id), len);
        new Uint8Array(w.frame).set(view);
        // The page wakes on this counter (Atomics.waitAsync) rather than on
        // the message below, which Chromium hands the page only at its next
        // rendering opportunity: ~9 ms of every click (measured).
        let seq = 0;
        if (signal) {
          Atomics.store(signal, 1, Number(id));
          seq = Atomics.add(signal, 0, 1) + 1;
          Atomics.notify(signal, 0);
        }
        // ?probe=1 (the proofs): what is on screen rides with the frame.
        const screen = data.probe ? ui.on_screen(id) : undefined;
        // On a touch screen, where the text fields are, when that changed:
        // the page raises the keyboard inside the tap that lands on one.
        let fields;
        if (data.touch) {
          const r = Array.from(ui.text_rects(id));
          const key = r.join(',');
          if (key !== w.fieldsKey) { w.fieldsKey = key; fields = r; }
        }
        post({ t: 'frame', id, seq, inputAt: pendingInputAt, paintedAt: now(), screen, fields });
        pendingInputAt = undefined;
      }
    };

    const imports = buildImports({
      meta: data.meta,
      ui, grants, kv, ring, bridge,
      args: data.args,
      storeKey: data.storeKey,
      // store.org: on, not-org or unsupported, as the page found it.
      org: data.org,
      sleepCell: new Int32Array(new SharedArrayBuffer(4)),
      output: (fd, text) => post({ t: 'out', fd, text }),
      notice: (iface, fname) => {
        const key = `${iface}#${fname}`;
        if (noticed.has(key)) return;
        noticed.add(key);
        post({ t: 'notice', iface, fname });
      },
      inputAt: (at) => { if (at !== undefined && pendingInputAt === undefined) pendingInputAt = at; },
      present,
      // The window the app asked for, unless the tab is smaller: then the
      // tab's size, as a phone does (the app hears it as its size).
      fit: (width, height) => ({
        width: Math.max(1, Math.min(width, data.avail.width)),
        height: Math.max(1, Math.min(height, data.avail.height)),
      }),
      onWindow: (id, title, asked) => {
        windows.set(id, { asked });
        post({ t: 'title', title });
        attachFrame(id);
        if (marks.firstWindow === undefined) marks.firstWindow = now();
      },
      onResized: (id) => attachFrame(id),
      onClose: (id) => { windows.delete(id); post({ t: 'closed', id }); },
      onTitle: (title) => post({ t: 'title', title }),
      resize: (id, width, height) => {
        data.avail = { width, height };
        const w = windows.get(id);
        const asked = (w && w.asked) || { width, height };
        const fit = { width: Math.min(asked.width, width), height: Math.min(asked.height, height) };
        const [, , lw, lh] = ui.frame_size(id);
        if (fit.width === lw && fit.height === lh) return;
        ui.set_size(id, fit.width, fit.height, true);
        attachFrame(id);
      },
      screen: (id) => {
        const size = ui.frame_size(id);
        return { width: size[2] || data.avail.width, height: size[3] || data.avail.height, touch: data.touch };
      },
      textEntry: (on) => post({ t: 'text-entry', on }),
    });

    // The glue the hub made at publish, and the core modules the page
    // checked against the judgement; nothing else is compiled.
    const glueUrl = URL.createObjectURL(new Blob([data.glue], { type: 'text/javascript' }));
    const glue = await import(glueUrl);
    marks.glueLoaded = now();
    const compiled = new Map();
    for (const [name, bytes] of data.modules) compiled.set(name, WebAssembly.compile(bytes));
    const getCoreModule = (name) => {
      const m = compiled.get(name);
      if (!m) throw new Error(`the app asked for ${name}, which is not one of its judged modules`);
      return m;
    };
    const app = await glue.instantiate(getCoreModule, imports);
    marks.instantiated = now();
    post({ t: 'marks', marks });
    const run = app.run || (app['krate:app/run'] && app['krate:app/run'].run);
    const code = run();
    present();
    post({ t: 'exit', code });
  } catch (e) {
    const stopped = e instanceof AppStopped;
    post({ t: 'trap', message: String((e && e.message) || e), stopped, stack: e && e.stack });
  }
};
