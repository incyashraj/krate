/* Serial and USB devices in a browser tab (IC-992): the web Player's two
 * halves, hosts.js in the app's worker and devices.js in the page, against
 * the scripted devices the browser-tab proof gives Chromium
 * (crates/adapter-web/player/fake-devices.mjs: Chrome's navigator.serial and
 * navigator.usb, a serial device that answers PING with PONG, and the
 * desktop fake's USB devices).
 *
 *   node --test cloud/worker/test/player-devices.test.mjs
 *
 * devices-proof.mjs drives the same code through a real Chromium with the
 * probe apps; this pins the rules one by one: what is refused before the
 * page is asked, the model filter, the computer's own devices, the claimed
 * interface's bounds, and a read that runs out losing nothing.
 */
import { test } from "node:test";
import assert from "node:assert";
import { buildImports, NO_SERIAL, NO_USB } from "../../../crates/adapter-web/player/web/hosts.js";
import { pageDevices, usbModels, usbFilters } from "../../../crates/adapter-web/player/web/devices.js";
import { fakeDevices } from "../../../crates/adapter-web/player/fake-devices.mjs";

// ------------------------------------------------------- the worker half

/// The imports for an app holding `granted`, in a browser with `facts`,
/// with a bridge that records every call and answers `answer(op, args)`.
const worker = ({ granted = [], facts = {}, answer = () => ({ json: { ok: null } }) } = {}) => {
  const calls = [];
  const imports = buildImports({
    meta: { refusals: {}, imports: [] },
    grants: { allows: (cap) => granted.includes(cap) },
    bridge: { call: (op, args) => { calls.push({ op, args }); return answer(op, args); } },
    ui: {}, kv: {}, ring: {}, output() {}, notice() {}, present() {},
    devices: facts,
  });
  return { serial: imports["krate:serial/ports"], usb: imports["krate:usb/devices"], calls };
};
const refused = (f, tag, val) => {
  try {
    f();
  } catch (e) {
    assert.deepStrictEqual(e.payload, val === undefined ? { tag } : { tag, val });
    return;
  }
  assert.fail(`expected ${tag}`);
};
const SETTINGS = { baudRate: 115200, dataBits: 8, parity: "none", stopBits: "one", readTimeoutMs: 1000 };

test("a browser without Web Serial or WebUSB answers unavailable, in words, and the page is never asked", () => {
  const w = worker({ granted: ["device.serial", "device.usb:1915:7777"], facts: { serial: false, usb: false, usbModels: ["1915:7777"] } });
  refused(() => w.serial.pick("Choose"), "unavailable", NO_SERIAL);
  refused(() => w.serial.open(1n, SETTINGS), "unavailable", NO_SERIAL);
  refused(() => w.usb.pick("Choose"), "unavailable", NO_USB);
  refused(() => w.usb.bulkIn(1n, 0x81, 64, 100), "unavailable", NO_USB);
  w.serial.close(1n);
  w.usb.close(1n);
  assert.deepStrictEqual(w.calls, []);
  assert.match(NO_SERIAL, /Chrome or Edge, or in Krate/);
});

test("without the grant, refused in the desktop's words before the page is asked", () => {
  const w = worker({ facts: { serial: true, usb: true, usbModels: [] } });
  refused(() => w.serial.pick("Choose"), "not-allowed", "this app did not ask for device.serial, so it cannot talk to a serial device");
  refused(() => w.usb.pick("Choose"), "not-allowed", "this app did not ask for device.usb, so it cannot talk to a USB device");
  // A model the page lists but the grants do not hold is not a grant.
  const forged = worker({ facts: { usb: true, usbModels: ["1915:7777"] } });
  refused(() => forged.usb.claim(1n, 0), "not-allowed", "this app did not ask for device.usb, so it cannot talk to a USB device");
  assert.deepStrictEqual([...w.calls, ...forged.calls], []);
});

test("what the app hands over is held to the desktop's limits, in its words", () => {
  const w = worker({ granted: ["device.serial", "device.usb:1915:7777"], facts: { serial: true, usb: true, usbModels: ["1915:7777"] } });
  refused(() => w.serial.open(1n, { ...SETTINGS, baudRate: 0 }), "bad-settings", "a baud rate is from 50 to 4000000, not 0");
  refused(() => w.serial.open(1n, { ...SETTINGS, dataBits: 9 }), "bad-settings", "data bits are 5, 6, 7 or 8, not 9");
  refused(() => w.serial.open(1n, { ...SETTINGS, readTimeoutMs: 60001 }), "bad-settings", "a wait is at most 60000 milliseconds, not 60001");
  refused(() => w.serial.read(1n, 10, 60001), "bad-settings", "a wait is at most 60000 milliseconds, not 60001");
  refused(() => w.serial.write(1n, new Uint8Array(65537)), "bad-settings", "a write is at most 65536 bytes, not 65537; write the rest in another call");
  refused(() => w.usb.bulkIn(1n, 0x81, 64, 0), "bad-request", "a wait is from 1 to 60000 milliseconds, not 0");
  refused(() => w.usb.interruptOut(1n, 0x02, new Uint8Array(1), 60001), "bad-request", "a wait is from 1 to 60000 milliseconds, not 60001");
  refused(() => w.usb.bulkOut(1n, 0x01, new Uint8Array(65537), 100), "bad-request", "a transfer is at most 65536 bytes, not 65537; send the rest in another call");
  refused(() => w.usb.controlIn(1n, { requestType: "vendor", recipient: "interface", request: 1, value: 0, index: 0 }, 4097, 100), "bad-request", "a control transfer is at most 4096 bytes, not 4097");
  assert.deepStrictEqual(w.calls, []);
});

test("a pick crosses to the page and comes back with a handle the app keeps", () => {
  const w = worker({
    granted: ["device.usb:1915:7777"],
    facts: { usb: true, usbModels: ["1915:7777"] },
    answer: (op) => (op === "usb.pick" ? { json: { ok: { handle: 3, name: "Radio dongle (1915:7777)", vendorId: 0x1915, productId: 0x7777 } } } : { json: { ok: null }, body: new Uint8Array([1, 2]) }),
  });
  assert.deepStrictEqual(w.usb.pick("Choose the radio"), { handle: 3n, name: "Radio dongle (1915:7777)", vendorId: 0x1915, productId: 0x7777 });
  assert.deepStrictEqual(Array.from(w.usb.bulkIn(3n, 0x81, 64, 100)), [1, 2]);
  // A handle past what a number holds was never given.
  w.usb.claim(18446744073709551615n, 0);
  assert.deepStrictEqual(w.calls.map((c) => [c.op, c.args.handle]), [["usb.pick", undefined], ["usb.in", 3], ["usb.claim", -1]]);
});

// --------------------------------------------------------- the page half

globalThis.window = globalThis;
Object.defineProperty(navigator, "userActivation", { configurable: true, value: { isActive: true } });
fakeDevices();
const page = (granted) => {
  const asked = [];
  const d = pageDevices({
    name: "Probe",
    report: {},
    granted,
    sheet: () => Promise.resolve(true),
    withTap: (go) => go("direct"),
  });
  return { d, h: d.handlers, asked };
};
const okOf = (a) => { assert.ok(!a.json.err, JSON.stringify(a.json)); return a.json.ok; };
const errOf = (a) => a.json.err;

test("the browser's list is filtered to exactly the models the person allowed", async () => {
  assert.deepStrictEqual(usbModels(["device.usb:1915:7777", "device.serial", "device.usb:1a86:7523"]), ["1915:7777", "1a86:7523"]);
  assert.deepStrictEqual(usbFilters(["1915:7777"]), [{ vendorId: 0x1915, productId: 0x7777 }]);
  window.__fake.asked.length = 0;
  Object.assign(window.__fake, { usbPick: "1915:7778", usbRogue: false });
  const { h } = page(["device.usb:1915:7777"]);
  assert.strictEqual(okOf(await h["usb.pick"]({ title: "Choose" })), null, "another model is not in the list");
  assert.deepStrictEqual(window.__fake.asked.at(-1).filters, [{ vendorId: 0x1915, productId: 0x7777 }]);
  // A browser that hands it back anyway: refused, never opened.
  window.__fake.usbRogue = true;
  window.__fake.log.length = 0;
  assert.deepStrictEqual(errOf(await h["usb.pick"]({ title: "Choose" })), { tag: "not-allowed", val: "this app did not ask for device.usb:1915:7778, so it cannot talk to that USB device" });
  assert.ok(!window.__fake.log.some((l) => l.startsWith("open")), window.__fake.log.join("\n"));
  window.__fake.usbRogue = false;
});

test("a keyboard is the computer's, named or not; so is the HID interface beside a dongle's own", async () => {
  Object.assign(window.__fake, { usbPick: "1915:0101" });
  const keys = page(["device.usb:1915:0101"]).h;
  assert.deepStrictEqual(errOf(await keys["usb.pick"]({ title: "" })), { tag: "not-for-apps", val: "Keyboard (1915:0101) is a keyboard, mouse or other input device, which the computer's own drivers serve; Krate does not hand it to an app" });
  Object.assign(window.__fake, { usbPick: "1915:7777" });
  const { h } = page(["device.usb:1915:7777"]);
  const picked = okOf(await h["usb.pick"]({ title: "" }));
  assert.strictEqual(picked.name, "Radio dongle (1915:7777)");
  assert.deepStrictEqual(errOf(await h["usb.claim"]({ handle: picked.handle, number: 1 })), { tag: "not-for-apps", val: "interface 1 of Radio dongle (1915:7777) is a keyboard, mouse or other input device, which the computer's own drivers serve; Krate does not hand it to an app" });
  await h["usb.close"]({ handle: picked.handle });
});

test("every transfer stays inside the claimed interface, in the desktop's words", async () => {
  Object.assign(window.__fake, { usbPick: "1915:7777" });
  const { h } = page(["device.usb:1915:7777"]);
  const { handle } = okOf(await h["usb.pick"]({ title: "" }));
  assert.deepStrictEqual(errOf(await h["usb.in"]({ handle, kind: "bulk", endpoint: 0x81, max: 64, timeout: 10 })), { tag: "not-claimed" });
  okOf(await h["usb.claim"]({ handle, number: 0 }));
  assert.deepStrictEqual(errOf(await h["usb.claim"]({ handle, number: 1 })), { tag: "bad-request", val: "this pick already claimed interface 0; one interface a pick, so pick the device again to reach interface 1" });
  assert.deepStrictEqual(errOf(await h["usb.in"]({ handle, kind: "bulk", endpoint: 0x01, max: 64, timeout: 10 })), { tag: "bad-request", val: "endpoint 0x01 is an OUT endpoint: write to it" });
  assert.deepStrictEqual(errOf(await h["usb.in"]({ handle, kind: "interrupt", endpoint: 0x81, max: 64, timeout: 10 })), { tag: "bad-request", val: "endpoint 0x81 is a bulk endpoint, and this call is for interrupt endpoints" });
  assert.deepStrictEqual(errOf(await h["usb.in"]({ handle, kind: "interrupt", endpoint: 0x83, max: 8, timeout: 10 })), { tag: "bad-request", val: "endpoint 0x83 is not one of interface 0's" });
  const setup = (requestType, recipient, request, index) => ({ requestType, recipient, request, value: 0, index });
  assert.deepStrictEqual(errOf(await h["usb.control-out"]({ handle, setup: setup("vendor", "interface", 1, 1), data: new Uint8Array(0), timeout: 10 })), { tag: "bad-request", val: "a control request for interface 1: this pick claimed interface 0, and a request reaches only that one" });
  assert.deepStrictEqual(errOf(await h["usb.control-out"]({ handle, setup: setup("standard", "device", 9, 0), data: new Uint8Array(0), timeout: 10 })), { tag: "bad-request", val: "standard request 0x09 is the computer's to send; an app sends class and vendor requests, and of the standard ones only GET_STATUS and GET_DESCRIPTOR" });
  assert.deepStrictEqual(errOf(await h["usb.control-in"]({ handle, setup: setup("standard", "endpoint", 6, 0x83), length: 18, timeout: 10 })), { tag: "bad-request", val: "a control request for endpoint 0x83, which is not one of interface 0's" });
  assert.strictEqual(okOf(await h["usb.control-in"]({ handle, setup: setup("standard", "device", 6, 0), length: 18, timeout: 10 })), null);
  assert.deepStrictEqual(errOf(await h["usb.out"]({ handle, kind: "bulk", endpoint: 0x01, bytes: new TextEncoder().encode("SLOW"), timeout: 20 })), { tag: "timed-out", val: "Radio dongle (1915:7777) did not answer in time" });
  await h["usb.close"]({ handle });
  assert.deepStrictEqual(errOf(await h["usb.claim"]({ handle, number: 0 })), { tag: "unknown-handle" });
});

test("a read whose wait runs out answers empty, and what arrives later is the next read's", async () => {
  Object.assign(window.__fake, { usbPick: "1915:7777" });
  const { h } = page(["device.usb:1915:7777"]);
  const { handle } = okOf(await h["usb.pick"]({ title: "" }));
  okOf(await h["usb.claim"]({ handle, number: 0 }));
  const quiet = await h["usb.in"]({ handle, kind: "bulk", endpoint: 0x81, max: 64, timeout: 20 });
  assert.strictEqual(quiet.body.length, 0);
  okOf(await h["usb.out"]({ handle, kind: "bulk", endpoint: 0x01, bytes: new Uint8Array([7, 8, 9]), timeout: 100 }));
  const later = await h["usb.in"]({ handle, kind: "bulk", endpoint: 0x81, max: 64, timeout: 100 });
  assert.deepStrictEqual(Array.from(later.body), [7, 8, 9]);
  await h["usb.close"]({ handle });
});

test("a serial port talks, waits, and is closed with its streams let go", async () => {
  Object.assign(window.__fake, { serialPick: "usb" });
  window.__fake.log.length = 0;
  const { h } = page(["device.serial"]);
  const { handle, name } = okOf(await h["serial.pick"]({ title: "Choose the scale" }));
  assert.strictEqual(name, "USB serial device (0403:6001)");
  assert.deepStrictEqual(errOf(await h["serial.read"]({ handle, max: 8, timeout: 0 })), { tag: "not-open" });
  assert.deepStrictEqual(errOf(await h["serial.open"]({ handle, settings: { ...SETTINGS, dataBits: 5 } })), { tag: "bad-settings", val: "USB serial device (0403:6001) does not take these settings: Requested number of data bits must be 7 or 8." });
  okOf(await h["serial.open"]({ handle, settings: { ...SETTINGS, readTimeoutMs: 50 } }));
  const started = Date.now();
  assert.strictEqual((await h["serial.read"]({ handle, max: 8, timeout: null })).body.length, 0);
  assert.ok(Date.now() - started >= 45, "the port's own wait");
  okOf(await h["serial.write"]({ handle, bytes: new TextEncoder().encode("PING\n") }));
  let heard = "";
  for (let i = 0; i < 20 && !heard.endsWith("PONG\n"); i++) heard += new TextDecoder().decode((await h["serial.read"]({ handle, max: 2, timeout: 100 })).body);
  assert.strictEqual(heard, "PONG\n", "at most `max` a read, nothing lost between reads");
  // Changing the settings of an open port opens it again.
  okOf(await h["serial.open"]({ handle, settings: { ...SETTINGS, baudRate: 9600 } }));
  okOf(await h["serial.close"]({ handle }));
  assert.deepStrictEqual(window.__fake.log.filter((l) => /open|close/.test(l)), ["serial open 115200 8N1", "serial close", "serial open 9600 8N1", "serial close"]);
  assert.deepStrictEqual(errOf(await h["serial.write"]({ handle, bytes: new Uint8Array(1) })), { tag: "unknown-handle" });
});

test("a run holds at most eight devices, said before the list is shown", async () => {
  Object.assign(window.__fake, { serialPick: "usb" });
  const { h, d } = page(["device.serial"]);
  // The one fake port, picked eight times: eight handles.
  for (let i = 0; i < 8; i++) okOf(await h["serial.pick"]({ title: "" }));
  const before = window.__fake.asked.length;
  assert.deepStrictEqual(errOf(await h["serial.pick"]({ title: "" })), { tag: "too-many", val: "this app already holds 8 serial devices; close one first" });
  assert.strictEqual(window.__fake.asked.length, before);
  d.close();
});
