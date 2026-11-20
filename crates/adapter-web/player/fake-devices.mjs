// A scripted serial device and scripted USB devices for the browser-tab
// proofs (IC-992), put in a page before the Player loads
// (context.addInitScript(fakeDevices)): navigator.serial and navigator.usb
// as Chrome gives them, behaving as the desktop's fakes do
// (crates/runtime/src/usb_host.rs fake::FakeUsb::standard, and a serial
// device that answers PING with PONG).
//
// What the person does in the browser's own device list is the proof's to
// say, before the list opens, in window.__fake:
//   serialPick  'usb' (the device), or 'cancel'
//   usbPick     '1915:7777' and the like, or 'cancel'
//   usbRogue    true: the browser hands back usbPick whatever the filters
//               said (a browser that does not keep to them)
// and everything the devices saw is in window.__fake.log, a line an event,
// with each list the browser was asked to show in window.__fake.asked.
//
// Like Chrome, each list opens only inside the person's own tap (transient
// user activation), a port closes only once its streams are let go, a USB
// interface is claimed only on an open device, and a protected class (HID,
// storage, audio, video) cannot be claimed.

export function fakeDevices() {
  const fake = (window.__fake = { serialPick: 'usb', usbPick: '1915:7777', usbRogue: false, log: [], asked: [] });
  const note = (line) => fake.log.push(line);
  const active = () => Boolean(navigator.userActivation && navigator.userActivation.isActive);
  const hex = (bytes) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  const ids = (v, p) => `${v.toString(16).padStart(4, '0')}:${p.toString(16).padStart(4, '0')}`;
  const noGesture = () => new DOMException('Must be handling a user gesture to show a permission request.', 'SecurityError');

  // ------------------------------------------------------------ serial
  class FakePort {
    constructor(info) {
      this.info = info;
      this.readable = null;
      this.writable = null;
      this.control = null;
    }
    getInfo() { return { ...this.info }; }
    async open(opts) {
      if (this.readable) throw new DOMException('The port is already open.', 'InvalidStateError');
      const dataBits = opts.dataBits === undefined ? 8 : opts.dataBits;
      if (dataBits !== 7 && dataBits !== 8) throw new TypeError('Requested number of data bits must be 7 or 8.');
      note(`serial open ${opts.baudRate} ${dataBits}${opts.parity === 'none' ? 'N' : opts.parity === 'odd' ? 'O' : 'E'}${opts.stopBits || 1}`);
      this.readable = new ReadableStream({ start: (c) => { this.control = c; } });
      this.writable = new WritableStream({
        write: (chunk) => {
          const text = new TextDecoder().decode(chunk);
          note(`serial write ${JSON.stringify(text)}`);
          if (text === 'PING\n') setTimeout(() => { try { this.control.enqueue(new TextEncoder().encode('PONG\n')); } catch { /* closed */ } }, 30);
        },
      });
    }
    async close() {
      if (!this.readable) throw new DOMException('The port is already closed.', 'InvalidStateError');
      if (this.readable.locked || this.writable.locked) throw new TypeError('Cannot close a port whose streams are locked.');
      try { this.control.close(); } catch { /* cancelled */ }
      this.readable = null;
      this.writable = null;
      note('serial close');
    }
  }
  const port = new FakePort({ usbVendorId: 0x0403, usbProductId: 0x6001 });
  const serial = {
    requestPort(options) {
      fake.asked.push({ what: 'serial', active: active(), options: options || null });
      if (!active()) return Promise.reject(noGesture());
      if (fake.serialPick !== 'usb') return Promise.reject(new DOMException('No port selected by the user.', 'NotFoundError'));
      return Promise.resolve(port);
    },
    getPorts: () => Promise.resolve([]),
    addEventListener() {},
    removeEventListener() {},
  };
  Object.defineProperty(Navigator.prototype, 'serial', { configurable: true, get: () => serial });

  // --------------------------------------------------------------- usb
  const endpoint = (endpointNumber, direction, type, packetSize) => ({ endpointNumber, direction, type, packetSize });
  const iface = (interfaceNumber, interfaceClass, endpoints) => {
    const alternate = { alternateSetting: 0, interfaceClass, interfaceSubclass: 0, interfaceProtocol: 0, endpoints };
    return { interfaceNumber, alternate, alternates: [alternate], claimed: false };
  };
  const echoIface = () => iface(0, 0xff, [
    endpoint(1, 'out', 'bulk', 64), endpoint(1, 'in', 'bulk', 64),
    endpoint(2, 'out', 'interrupt', 8), endpoint(2, 'in', 'interrupt', 8),
  ]);
  const PROTECTED = new Set([0x01, 0x03, 0x08, 0x0b, 0x0e, 0x10]);
  class FakeUsbDevice {
    constructor(vendorId, productId, deviceClass, productName, interfaces) {
      Object.assign(this, { vendorId, productId, deviceClass, productName, manufacturerName: 'Proof Labs', serialNumber: '' });
      this.configurations = [{ configurationValue: 1, configurationName: '', interfaces }];
      this.configuration = this.configurations[0];
      this.opened = false;
      this.queues = new Map();
      this.waiting = new Map();
      this.stored = new Map();
    }
    get ids() { return ids(this.vendorId, this.productId); }
    async open() { if (this.opened) return; this.opened = true; note(`open ${this.ids}`); }
    async close() {
      if (!this.opened) return;
      for (const i of this.configuration.interfaces) if (i.claimed) { i.claimed = false; note(`release ${this.ids} ${i.interfaceNumber}`); }
      for (const [, list] of this.waiting) for (const w of list) w.reject(new DOMException('The transfer was cancelled.', 'AbortError'));
      this.waiting.clear();
      this.opened = false;
      note(`close ${this.ids}`);
    }
    async forget() { note(`forget ${this.ids}`); }
    async selectConfiguration() {}
    async claimInterface(n) {
      if (!this.opened) throw new DOMException('The device must be opened first.', 'InvalidStateError');
      const i = this.configuration.interfaces.find((x) => x.interfaceNumber === n);
      if (!i) throw new DOMException('The interface number provided is not supported by the device in its current configuration.', 'NotFoundError');
      if (PROTECTED.has(i.alternate.interfaceClass)) throw new DOMException('The requested interface implements a protected class.', 'SecurityError');
      i.claimed = true;
      note(`claim ${this.ids} ${n}`);
    }
    async releaseInterface(n) {
      const i = this.configuration.interfaces.find((x) => x.interfaceNumber === n);
      if (i && i.claimed) { i.claimed = false; note(`release ${this.ids} ${n}`); }
    }
    async clearHalt(direction, n) { note(`clear-halt ${this.ids} ${direction} ${n}`); }
    owns(direction, n) {
      return this.configuration.interfaces.some((i) => i.claimed && i.alternate.endpoints.some((e) => e.endpointNumber === n && e.direction === direction));
    }
    transferIn(n, length) {
      if (!this.opened) return Promise.reject(new DOMException('The device must be opened first.', 'InvalidStateError'));
      if (!this.owns('in', n)) return Promise.reject(new DOMException('The specified endpoint is not part of a claimed and selected alternate interface.', 'NotFoundError'));
      const address = 0x80 | n;
      const give = () => {
        const q = this.queues.get(address) || [];
        const got = q.splice(0, length);
        note(`in ${this.ids} 0x${address.toString(16)} ${hex(got)}`);
        return { status: 'ok', data: new DataView(new Uint8Array(got).buffer) };
      };
      if ((this.queues.get(address) || []).length) return Promise.resolve(give());
      // Transfers in wait their turn: the first one asked for gets what
      // arrives, as a USB controller does, whether or not anyone still
      // waits on it.
      if (!this.waiting.has(address)) this.waiting.set(address, []);
      return new Promise((resolve, reject) => { this.waiting.get(address).push({ take: give, resolve, reject }); });
    }
    transferOut(n, data) {
      if (!this.opened) return Promise.reject(new DOMException('The device must be opened first.', 'InvalidStateError'));
      if (!this.owns('out', n)) return Promise.reject(new DOMException('The specified endpoint is not part of a claimed and selected alternate interface.', 'NotFoundError'));
      const bytes = new Uint8Array(data.buffer ? data.buffer.slice(data.byteOffset || 0, (data.byteOffset || 0) + data.byteLength) : data);
      const text = new TextDecoder().decode(bytes);
      // Bytes it never takes, and bytes it refuses (the desktop fake's).
      if (text === 'SLOW') return new Promise(() => {});
      if (text === 'STALL') return Promise.resolve({ status: 'stall', bytesWritten: 0 });
      note(`out ${this.ids} 0x${n.toString(16).padStart(2, '0')} ${hex(bytes)}`);
      const back = 0x80 | n;
      if (!this.queues.has(back)) this.queues.set(back, []);
      this.queues.get(back).push(...bytes);
      // The bytes are the waiting transfer's the moment they arrive.
      const w = (this.waiting.get(back) || []).shift();
      if (w) { const got = w.take(); setTimeout(() => w.resolve(got), 0); }
      return Promise.resolve({ status: 'ok', bytesWritten: bytes.length });
    }
    controlTransferIn(setup, length) {
      if (!this.opened) return Promise.reject(new DOMException('The device must be opened first.', 'InvalidStateError'));
      note(`control-in ${this.ids} ${setup.requestType} ${setup.recipient} ${setup.request} ${setup.value} ${setup.index} ${length}`);
      let data;
      if (setup.requestType === 'standard') {
        // GET_DESCRIPTOR(device) is 18 bytes; GET_STATUS 2.
        data = setup.request === 0x06 ? [18, 1, 0, 2, 0xff, 0, 0, 64, 0x15, 0x19, 0x77, 0x77, 0, 1, 1, 2, 0, 1] : [0, 0];
      } else {
        data = this.stored.get(`${setup.recipient}/${setup.request}/${setup.value}`) || [];
      }
      return Promise.resolve({ status: 'ok', data: new DataView(new Uint8Array(data.slice(0, length)).buffer) });
    }
    controlTransferOut(setup, data) {
      if (!this.opened) return Promise.reject(new DOMException('The device must be opened first.', 'InvalidStateError'));
      const bytes = data ? new Uint8Array(data.buffer ? data.buffer.slice(data.byteOffset || 0, (data.byteOffset || 0) + data.byteLength) : data) : new Uint8Array(0);
      note(`control-out ${this.ids} ${setup.requestType} ${setup.recipient} ${setup.request} ${setup.value} ${setup.index} ${hex(bytes)}`);
      this.stored.set(`${setup.recipient}/${setup.request}/${setup.value}`, Array.from(bytes));
      return Promise.resolve({ status: 'ok', bytesWritten: bytes.length });
    }
  }
  const connected = [
    new FakeUsbDevice(0x1915, 0x7777, 0xff, 'Radio dongle', [echoIface(), iface(1, 0x03, [endpoint(3, 'in', 'interrupt', 8)])]),
    new FakeUsbDevice(0x1915, 0x7778, 0xff, 'Second dongle', [echoIface()]),
    new FakeUsbDevice(0x1915, 0x0101, 0x00, 'Keyboard', [iface(0, 0x03, [endpoint(1, 'in', 'interrupt', 8)])]),
  ];
  const usb = {
    requestDevice(options) {
      const filters = options && options.filters;
      fake.asked.push({ what: 'usb', active: active(), filters: filters ? filters.map((f) => ({ ...f })) : null });
      if (!active()) return Promise.reject(noGesture());
      if (!Array.isArray(filters)) return Promise.reject(new TypeError("Failed to execute 'requestDevice' on 'USB': required member filters is undefined."));
      // Chrome's list: every device a filter matches (no filters, all).
      const listed = connected.filter((d) => !filters.length || filters.some((f) =>
        (f.vendorId === undefined || f.vendorId === d.vendorId) && (f.productId === undefined || f.productId === d.productId)));
      fake.listed = listed.map((d) => d.ids);
      const pool = fake.usbRogue ? connected : listed;
      const chosen = pool.find((d) => d.ids === fake.usbPick);
      if (!chosen) return Promise.reject(new DOMException('No device selected.', 'NotFoundError'));
      return Promise.resolve(chosen);
    },
    getDevices: () => Promise.resolve([]),
    addEventListener() {},
    removeEventListener() {},
  };
  Object.defineProperty(Navigator.prototype, 'usb', { configurable: true, get: () => usb });
}
