// Serial and USB devices for a Krate app in a browser tab, the page's half
// (IC-992): krate:serial through the browser's Web Serial, krate:usb through
// WebUSB, both in Chrome and Edge.
//
// hosts.js, in the app's worker, has already checked the app's grant and
// what it handed over -- a baud rate, a wait, a size -- in the desktop's
// words; nothing reaches here for an app without the capability. What is
// here is what the desktop's serial_host.rs and usb_host.rs hold: the
// handles, the picked device behind each, the one claimed interface, and
// every rule that needs them, word for word.
//
// The pick is the browser's own device list: what the person chooses there
// is the grant, for that one device, for this run. For USB the list is
// filtered to exactly the models the manifest names (device.usb:<vendor>:
// <product>, as the person allowed them), and a device the browser hands
// back anyway is checked again here before anything opens it. A browser
// shows its list only inside the person's own tap: from a tap at once,
// otherwise one tap on the Player's sheet away (player.js withTap), as the
// file picker does.
//
// A browser has no wait on a transfer: each wait here is a timer beside it.
// A read that runs out answers empty and leaves the browser's read going,
// and what it brings later is the next read's, so no byte is lost.

import { NO_SERIAL, NO_USB } from './hosts.js';

const ok = (value) => ({ json: { ok: value === undefined ? null : value } });
const no = (tag, val) => ({ json: { err: val === undefined ? { tag } : { tag, val } } });
const bytes = (b) => ({ json: { ok: null }, body: b });

/// crates/runtime/src/serial_host.rs and usb_host.rs.
const MAX_HELD = 8;
const MAX_IO_BYTES = 65536;
const MAX_TITLE_CHARS = 200;
const MIN_WRITE_WAIT_MS = 1000;
/// What a serial port keeps of what the device sent while nobody read it.
const MAX_QUEUED = 1 << 20;


const hex4 = (n) => Number(n).toString(16).padStart(4, '0');
/// Rust's `{:#04x}`: 0x01, 0x81.
const addr = (n) => `0x${Number(n).toString(16).padStart(2, '0')}`;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const message = (e) => String((e && e.message) || e);
const titleOf = (title) => Array.from(String(title || '').trim()).slice(0, MAX_TITLE_CHARS).join('');

/// What the computer's own drivers serve, by USB class code
/// (usb_host.rs kept_by_the_computer).
export function keptByTheComputer(cls) {
  switch (cls) {
    case 0x01: return 'an audio device';
    case 0x03: return 'a keyboard, mouse or other input device';
    case 0x08: return 'a storage device';
    case 0x09: return 'a USB hub';
    case 0x0e: return 'a camera or other video device';
    case 0x10: return 'an audio and video device';
    default: return null;
  }
}

/// The interfaces of the device's active configuration, or its first.
const interfacesOf = (device) => {
  const config = device.configuration || (device.configurations && device.configurations[0]);
  return (config && config.interfaces) || [];
};
const classOf = (iface) => (iface.alternate || (iface.alternates && iface.alternates[0]) || {}).interfaceClass;

/// usb_host.rs kept_device: its own class, or every interface it has.
function keptDevice(device) {
  const own = keptByTheComputer(device.deviceClass);
  if (own) return own;
  const kept = interfacesOf(device).map((i) => keptByTheComputer(classOf(i)));
  return kept.length && kept.every(Boolean) ? kept[0] : null;
}

/// The device models the person allowed, from the grants the page holds:
/// `device.usb:1915:7777` -> "1915:7777".
export function usbModels(granted) {
  return (granted || []).map((cap) => /^device\.usb:([0-9a-f]{4}):([0-9a-f]{4})$/.exec(cap))
    .filter(Boolean).map((m) => `${m[1]}:${m[2]}`);
}

/// The browser's list, filtered to exactly those models.
export function usbFilters(models) {
  return models.map((ids) => {
    const [vendor, product] = ids.split(':');
    return { vendorId: parseInt(vendor, 16), productId: parseInt(product, 16) };
  });
}

/// ctx: { name, report, granted, sheet(title, body, yes, no) -> Promise<bool>,
///        withTap(go(how), ask()) }
export function pageDevices({ name, report, granted, sheet, withTap }) {
  const state = (report.devices = { asked: [], serial: [], usb: [] });
  const serial = typeof navigator !== 'undefined' ? navigator.serial : undefined;
  const usb = typeof navigator !== 'undefined' ? navigator.usb : undefined;
  const models = usbModels(granted);
  let next = 1;

  // The browser's own list, inside the person's tap. Resolves with what
  // they chose, or null when they cancelled (on the sheet or in the list).
  const choose = (what, title, show) => new Promise((resolve) => {
    const go = (how) => {
      state.asked.push({ what, how });
      let p;
      try { p = show(); } catch (e) { p = Promise.reject(e); }
      p.then(resolve, (e) => resolve(e && e.name === 'NotFoundError' ? null : { error: e }));
    };
    const heading = title || (what === 'serial' ? 'Choose a serial device' : 'Choose a USB device');
    const body = `${name} asked to talk to ${what === 'serial' ? 'a serial device' : 'a USB device'}. It reaches only the one you choose, until it closes.`;
    withTap(go, () => sheet(heading, body, 'Choose a device', 'Cancel').then((yes) => {
      if (yes) go('sheet');
      else { state.asked.push({ what, how: 'declined' }); resolve(null); }
    }));
  });

  // ------------------------------------------------------------ serial
  const ports = new Map();
  const portName = (port) => {
    let info = {};
    try { info = port.getInfo() || {}; } catch { /* no ids */ }
    return info.usbVendorId !== undefined ? `USB serial device (${hex4(info.usbVendorId)}:${hex4(info.usbProductId)})` : 'Serial device';
  };
  const wake = (h) => { const w = h.waiters; h.waiters = []; for (const f of w) f(); };
  const gone = (h) => no('disconnected', `${h.name} is not connected any more`);
  // Everything the device sends, kept until the app reads it, as the
  // computer's own serial driver keeps it.
  const pump = async (h) => {
    while (!h.stopping && h.port.readable) {
      let reader;
      try { reader = h.port.readable.getReader(); } catch { break; }
      h.reader = reader;
      let ended = false;
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (done) { ended = true; break; }
          if (value && value.length) {
            h.queue.push(value);
            h.queued += value.length;
            while (h.queued > MAX_QUEUED && h.queue.length > 1) h.queued -= h.queue.shift().length;
            wake(h);
          }
        }
      } catch {
        // A break, framing, parity or overrun error is the line's, not the
        // end: the browser gives a fresh stream, read on. A lost device
        // leaves none, which ends it below.
      } finally {
        try { reader.releaseLock(); } catch { /* already let go */ }
        h.reader = null;
      }
      if (ended) break;
    }
    if (!h.stopping) { h.gone = true; wake(h); }
  };
  const stop = async (h) => {
    if (!h.open) return;
    h.stopping = true;
    if (h.reader) await h.reader.cancel().catch(() => {});
    await h.pumping;
    if (h.writer) { try { h.writer.releaseLock(); } catch { /* pending write */ } h.writer = null; }
    await h.port.close().catch(() => {});
    h.open = false;
  };
  const start = async (h, s) => {
    await h.port.open({
      baudRate: s.baudRate,
      dataBits: s.dataBits,
      stopBits: s.stopBits === 'two' ? 2 : 1,
      parity: s.parity,
      flowControl: 'none',
      bufferSize: MAX_IO_BYTES,
    });
    h.open = true;
    h.stopping = false;
    h.gone = false;
    h.writer = h.port.writable ? h.port.writable.getWriter() : null;
    h.pumping = pump(h);
  };
  const serialHandlers = {
    'serial.pick': async ({ title }) => {
      if (!serial) return no('unavailable', NO_SERIAL);
      // Krate's limit is said before the list is shown, not after a pick.
      if (ports.size >= MAX_HELD) return no('too-many', `this app already holds ${MAX_HELD} serial devices; close one first`);
      const port = await choose('serial', titleOf(title), () => serial.requestPort());
      if (!port) return ok(null);
      if (port.error) return no('unavailable', `the browser did not show its list of serial devices (${message(port.error)})`);
      const handle = next++;
      const h = { port, name: portName(port), open: false, queue: [], queued: 0, waiters: [], readTimeout: 0, baud: 4000000 };
      ports.set(handle, h);
      state.serial.push({ handle, name: h.name });
      return ok({ handle, name: h.name });
    },
    'serial.open': async ({ handle, settings }) => {
      const h = ports.get(handle);
      if (!h) return no('unknown-handle');
      // A browser sets a port's speed only as it opens it: changing an open
      // port's settings closes it and opens it again.
      if (h.open) await stop(h);
      try {
        await start(h, settings);
      } catch (e) {
        if (e && e.name === 'TypeError') return no('bad-settings', `${h.name} does not take these settings: ${message(e)}`);
        if (e && (e.name === 'NetworkError' || e.name === 'InvalidStateError')) return no('busy', `${h.name} is in use by another program, or was unplugged`);
        if (e && e.name === 'NotFoundError') return gone(h);
        return no('failed', `${h.name}: ${message(e)}`);
      }
      h.readTimeout = settings.readTimeoutMs;
      h.baud = settings.baudRate;
      return ok();
    },
    'serial.read': async ({ handle, max, timeout }) => {
      const h = ports.get(handle);
      if (!h) return no('unknown-handle');
      if (!h.open) return no('not-open');
      const want = Math.min(max, MAX_IO_BYTES);
      if (want === 0) return bytes(new Uint8Array(0));
      const wait = timeout === null || timeout === undefined ? h.readTimeout : timeout;
      if (!h.queued && !h.gone && wait > 0) {
        await Promise.race([new Promise((resolve) => h.waiters.push(resolve)), sleep(wait)]);
      }
      if (!h.queued) return h.gone ? gone(h) : bytes(new Uint8Array(0));
      const out = new Uint8Array(Math.min(want, h.queued));
      let at = 0;
      while (at < out.length) {
        const first = h.queue[0];
        const take = Math.min(first.length, out.length - at);
        out.set(first.subarray(0, take), at);
        at += take;
        if (take === first.length) h.queue.shift();
        else h.queue[0] = first.subarray(take);
      }
      h.queued -= out.length;
      return bytes(out);
    },
    'serial.write': async ({ handle, bytes: data }) => {
      const h = ports.get(handle);
      if (!h) return no('unknown-handle');
      if (!h.open) return no('not-open');
      if (h.gone || !h.writer) return gone(h);
      // The desktop's deadline: the port's own wait (at least a second)
      // plus the bytes' time on the wire, ten bits a character.
      const onTheWire = Math.ceil((data.length * 10 * 1000) / Math.max(1, h.baud));
      const deadline = Math.max(h.readTimeout, MIN_WRITE_WAIT_MS) + onTheWire;
      let late = false;
      try {
        await Promise.race([h.writer.write(data), sleep(deadline).then(() => { late = true; })]);
      } catch (e) {
        if (e && e.name === 'NetworkError') { h.gone = true; return gone(h); }
        return no('failed', `${h.name}: ${message(e)}`);
      }
      if (late) return no('failed', `${h.name} did not take the bytes in time`);
      return ok();
    },
    'serial.close': async ({ handle }) => {
      const h = ports.get(handle);
      if (!h) return ok();
      ports.delete(handle);
      await stop(h).catch(() => {});
      return ok();
    },
  };

  // --------------------------------------------------------------- usb
  const devices = new Map();
  const usbName = (device) => {
    const ids = `${hex4(device.vendorId)}:${hex4(device.productId)}`;
    const label = String(device.productName || device.manufacturerName || '').trim();
    return label ? `${label} (${ids})` : `USB device ${ids}`;
  };
  const usbGone = (h) => no('disconnected', `${h.name} is not connected any more`);
  // A browser's answer to a transfer, as usb_host.rs says it.
  const usbFault = (h, e) => {
    if (e && e.name === 'NotFoundError') return usbGone(h);
    if (e && e.name === 'NetworkError' && /disconnect/i.test(message(e))) return usbGone(h);
    return no('failed', `${h.name}: ${message(e)}`);
  };
  const KINDS = { bulk: 'bulk', interrupt: 'interrupt' };
  const kindName = (k) => KINDS[k] || 'isochronous or control';
  // usb_host.rs own_endpoint.
  const ownEndpoint = (h, kind, address, isIn) => {
    if (!h.claimed) return { refused: no('not-claimed') };
    const ep = h.claimed.endpoints.find((e) => e.address === address);
    if (!ep) return { refused: no('bad-request', `endpoint ${addr(address)} is not one of interface ${h.claimed.number}'s`) };
    if (ep.kind !== kind) return { refused: no('bad-request', `endpoint ${addr(address)} is a ${kindName(ep.kind)} endpoint, and this call is for ${kindName(kind)} endpoints`) };
    const epIn = (ep.address & 0x80) !== 0;
    if (epIn !== isIn) {
      return { refused: no('bad-request', epIn ? `endpoint ${addr(address)} is an IN endpoint: read from it` : `endpoint ${addr(address)} is an OUT endpoint: write to it`) };
    }
    return { ep };
  };
  // usb_host.rs check_setup: the claimed interface, one of its endpoints,
  // or (class and vendor) the whole device; of the standard requests only
  // GET_STATUS and GET_DESCRIPTOR, and only as reads.
  const checkSetup = (h, setup, isIn) => {
    if (!h.claimed) return no('not-claimed');
    const target = setup.index & 0xff;
    if (setup.recipient === 'interface' && target !== h.claimed.number) {
      return no('bad-request', `a control request for interface ${target}: this pick claimed interface ${h.claimed.number}, and a request reaches only that one`);
    }
    if (setup.recipient === 'endpoint' && !h.claimed.endpoints.some((e) => e.address === target)) {
      return no('bad-request', `a control request for endpoint ${addr(target)}, which is not one of interface ${h.claimed.number}'s`);
    }
    if (setup.requestType === 'standard' && !(isIn && (setup.request === 0x00 || setup.request === 0x06))) {
      return no('bad-request', `standard request ${addr(setup.request)} is the computer's to send; an app sends class and vendor requests, and of the standard ones only GET_STATUS and GET_DESCRIPTOR`);
    }
    return null;
  };
  const webSetup = (s) => ({ requestType: s.requestType, recipient: s.recipient, request: s.request, value: s.value, index: s.index });
  // A transfer and its wait: the result, or `late` when the wait ran out.
  const within = (p, ms) => Promise.race([p.then((r) => ({ r }), (e) => ({ e })), sleep(ms).then(() => ({ late: true }))]);
  const clear = (h, direction, number) => {
    try { return Promise.resolve(h.device.clearHalt(direction, number)).catch(() => {}); } catch { return Promise.resolve(); }
  };
  const take = (h, address, len) => {
    const kept = h.stash.get(address);
    if (!kept || !kept.length) return null;
    const out = kept.slice(0, len);
    h.stash.set(address, kept.slice(out.length));
    return out;
  };
  const usbHandlers = {
    'usb.pick': async ({ title }) => {
      if (!usb) return no('unavailable', NO_USB);
      if (!models.length) return no('not-allowed', 'this app did not ask for device.usb, so it cannot talk to a USB device');
      if (devices.size >= MAX_HELD) return no('too-many', `this app already holds ${MAX_HELD} USB devices; close one first`);
      const filters = usbFilters(models);
      state.usb.push({ filters });
      const device = await choose('usb', titleOf(title), () => usb.requestDevice({ filters }));
      if (!device) return ok(null);
      if (device.error) return no('unavailable', `the browser did not show its list of USB devices (${message(device.error)})`);
      const ids = `${hex4(device.vendorId)}:${hex4(device.productId)}`;
      const named = usbName(device);
      // The browser's list was filtered to the named models; a device it
      // hands back anyway is still not this app's.
      if (!models.includes(ids)) {
        state.usb.push({ refused: ids });
        if (typeof device.forget === 'function') device.forget().catch(() => {});
        return no('not-allowed', `this app did not ask for device.usb:${ids}, so it cannot talk to that USB device`);
      }
      const kept = keptDevice(device);
      if (kept) return no('not-for-apps', `${named} is ${kept}, which the computer's own drivers serve; Krate does not hand it to an app`);
      try {
        await device.open();
        if (!device.configuration && device.configurations && device.configurations.length) {
          await device.selectConfiguration(device.configurations[0].configurationValue);
        }
      } catch (e) {
        if (e && e.name === 'NotFoundError') return no('disconnected', `${named} is not connected any more`);
        if (e && (e.name === 'NetworkError' || e.name === 'InvalidStateError')) return no('busy', `${named} is in use by another program`);
        return no('failed', `${named}: ${message(e)}`);
      }
      const handle = next++;
      devices.set(handle, { device, name: named, ids, claimed: null, inflight: new Map(), stash: new Map() });
      state.usb.push({ handle, ids });
      return ok({ handle, name: named, vendorId: device.vendorId, productId: device.productId });
    },
    'usb.claim': async ({ handle, number }) => {
      const h = devices.get(handle);
      if (!h) return no('unknown-handle');
      if (h.claimed) {
        if (h.claimed.number === number) return ok();
        return no('bad-request', `this pick already claimed interface ${h.claimed.number}; one interface a pick, so pick the device again to reach interface ${number}`);
      }
      const iface = interfacesOf(h.device).find((i) => i.interfaceNumber === number);
      if (!iface) return no('bad-request', `${h.name} has no interface ${number}`);
      const what = keptByTheComputer(classOf(iface));
      const notForApps = (w) => no('not-for-apps', `interface ${number} of ${h.name} is ${w}, which the computer's own drivers serve; Krate does not hand it to an app`);
      if (what) return notForApps(what);
      try {
        await h.device.claimInterface(number);
      } catch (e) {
        // The browser keeps some kinds for itself too (WebUSB's protected
        // classes): the same refusal.
        if (e && e.name === 'SecurityError') return notForApps('a kind of device the browser keeps for itself');
        if (e && e.name === 'NotFoundError') return usbGone(h);
        if (e && (e.name === 'NetworkError' || e.name === 'InvalidStateError')) {
          return no('busy', `interface ${number} of ${h.name} is in use by another program or a driver of the computer's own`);
        }
        return no('failed', `${h.name}: ${message(e)}`);
      }
      const alt = iface.alternate || (iface.alternates && iface.alternates[0]) || { endpoints: [] };
      h.claimed = {
        number,
        endpoints: (alt.endpoints || []).map((e) => ({
          address: (e.endpointNumber & 0x0f) | (e.direction === 'in' ? 0x80 : 0),
          kind: e.type === 'bulk' || e.type === 'interrupt' ? e.type : 'other',
          packet: e.packetSize || 0,
        })),
      };
      return ok();
    },
    'usb.in': async ({ handle, kind, endpoint, max, timeout }) => {
      const h = devices.get(handle);
      if (!h) return no('unknown-handle');
      const { ep, refused } = ownEndpoint(h, kind, endpoint, true);
      if (refused) return refused;
      if (max === 0) return bytes(new Uint8Array(0));
      // USB moves whole packets: ask for whole ones, never past the limit.
      const packet = Math.max(1, ep.packet);
      const len = Math.max(packet, Math.min(Math.ceil(Math.min(max, MAX_IO_BYTES) / packet) * packet, Math.floor(MAX_IO_BYTES / packet) * packet));
      const kept = take(h, endpoint, len);
      if (kept) return bytes(kept);
      let pending = h.inflight.get(endpoint);
      if (!pending) {
        let p;
        try { p = Promise.resolve(h.device.transferIn(endpoint & 0x0f, len)); } catch (e) { p = Promise.reject(e); }
        pending = p;
        h.inflight.set(endpoint, pending);
      }
      const got = await within(pending, timeout);
      // Nothing yet: empty, and the browser's read stays the next one's.
      if (got.late) return bytes(new Uint8Array(0));
      h.inflight.delete(endpoint);
      if (got.e) return usbFault(h, got.e);
      if (got.r.status === 'stall') {
        await clear(h, 'in', endpoint & 0x0f);
        return no('stalled', `${h.name} refused the transfer on endpoint ${addr(endpoint)}; Krate cleared it, so the next transfer can go`);
      }
      if (got.r.status && got.r.status !== 'ok') return no('failed', `${h.name}: the device sent more than it was asked for`);
      const data = got.r.data ? new Uint8Array(got.r.data.buffer, got.r.data.byteOffset, got.r.data.byteLength).slice() : new Uint8Array(0);
      if (data.length > len) {
        h.stash.set(endpoint, data.slice(len));
        return bytes(data.slice(0, len));
      }
      return bytes(data);
    },
    'usb.out': async ({ handle, kind, endpoint, bytes: data, timeout }) => {
      const h = devices.get(handle);
      if (!h) return no('unknown-handle');
      const { refused } = ownEndpoint(h, kind, endpoint, false);
      if (refused) return refused;
      let p;
      try { p = Promise.resolve(h.device.transferOut(endpoint & 0x0f, data)); } catch (e) { p = Promise.reject(e); }
      const got = await within(p, timeout);
      if (got.late) return no('timed-out', `${h.name} did not answer in time`);
      if (got.e) return usbFault(h, got.e);
      if (got.r.status === 'stall') {
        await clear(h, 'out', endpoint & 0x0f);
        return no('stalled', `${h.name} refused the transfer on endpoint ${addr(endpoint)}; Krate cleared it, so the next transfer can go`);
      }
      return ok();
    },
    'usb.control-in': async ({ handle, setup, length, timeout }) => {
      const h = devices.get(handle);
      if (!h) return no('unknown-handle');
      const refused = checkSetup(h, setup, true);
      if (refused) return refused;
      let p;
      try { p = Promise.resolve(h.device.controlTransferIn(webSetup(setup), length)); } catch (e) { p = Promise.reject(e); }
      const got = await within(p, timeout);
      if (got.late) return no('timed-out', `${h.name} did not answer the request in time`);
      if (got.e) return usbFault(h, got.e);
      if (got.r.status === 'stall') return no('stalled', `${h.name} refused the request`);
      const data = got.r.data ? new Uint8Array(got.r.data.buffer, got.r.data.byteOffset, got.r.data.byteLength).slice(0, length) : new Uint8Array(0);
      return bytes(data);
    },
    'usb.control-out': async ({ handle, setup, data, timeout }) => {
      const h = devices.get(handle);
      if (!h) return no('unknown-handle');
      const refused = checkSetup(h, setup, false);
      if (refused) return refused;
      let p;
      try { p = Promise.resolve(h.device.controlTransferOut(webSetup(setup), data)); } catch (e) { p = Promise.reject(e); }
      const got = await within(p, timeout);
      if (got.late) return no('timed-out', `${h.name} did not answer the request in time`);
      if (got.e) return usbFault(h, got.e);
      if (got.r.status === 'stall') return no('stalled', `${h.name} refused the request`);
      return ok();
    },
    'usb.close': async ({ handle }) => {
      const h = devices.get(handle);
      if (!h) return ok();
      devices.delete(handle);
      await letGo(h);
      return ok();
    },
  };
  const letGo = async (h) => {
    if (h.claimed) await Promise.resolve().then(() => h.device.releaseInterface(h.claimed.number)).catch(() => {});
    await Promise.resolve().then(() => h.device.close()).catch(() => {});
  };

  return {
    /// What this browser has, for the app's worker and the consent card.
    facts: { serial: Boolean(serial), usb: Boolean(usb), usbModels: models },
    handlers: { ...serialHandlers, ...usbHandlers },
    /// The app ended: every port closed and every device let go, as the
    /// desktop does when a run ends.
    close() {
      for (const h of ports.values()) stop(h).catch(() => {});
      ports.clear();
      for (const h of devices.values()) letGo(h);
      devices.clear();
    },
  };
}

/// Printing what the window shows (krate:ui/print, IC-992), the page's
/// half: each page, a picture of the window, put in the page's own print
/// sheet (player.css hides everything else when printing) and handed to the
/// browser's print dialog, where the person picks the printer. The job is
/// named after the app's title for it.
export function pagePrint({ name, report }) {
  const state = (report.printed = []);
  const toPicture = (page) => new Promise((resolve) => {
    const canvas = document.createElement('canvas');
    canvas.width = page.width;
    canvas.height = page.height;
    canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(page.rgba.buffer, 0, page.width * page.height * 4), page.width, page.height), 0, 0);
    canvas.toBlob((blob) => resolve(URL.createObjectURL(blob)), 'image/png');
  });
  return {
    'print.pages': async ({ title, pages }) => {
      const sheet = document.getElementById('printPages');
      const urls = await Promise.all(pages.map(toPicture));
      sheet.textContent = '';
      for (const [i, url] of urls.entries()) {
        const img = document.createElement('img');
        img.src = url;
        img.alt = `Page ${i + 1}`;
        sheet.appendChild(img);
      }
      await Promise.all(Array.from(sheet.querySelectorAll('img'), (img) => img.decode().catch(() => {})));
      const was = document.title;
      document.title = String(title || '').trim() || name;
      document.body.classList.add('printing');
      state.push({ title: document.title, pages: pages.length });
      let done = false;
      const tidy = () => {
        if (done) return;
        done = true;
        document.body.classList.remove('printing');
        document.title = was;
        sheet.textContent = '';
        for (const url of urls) URL.revokeObjectURL(url);
      };
      // A browser whose dialog does not hold the page tidies up after it.
      addEventListener('afterprint', () => setTimeout(tidy, 0), { once: true });
      try {
        window.print();
      } catch (e) {
        tidy();
        return { json: { err: { tag: 'platform', val: `the browser did not print (${message(e)})` } } };
      }
      setTimeout(tidy, 60000);
      return { json: { ok: 'printed' } };
    },
  };
}

/// Whether this browser has the API a consent row needs (`needs` from the
/// Player module: "serial" or "usb"), so the card can say where it works.
export function browserHas(needs) {
  if (!needs) return true;
  if (typeof navigator === 'undefined') return false;
  return Boolean(navigator[needs]);
}
