/* A report reaches the founder the moment it arrives (2026-11-23).
 *
 * Studio's "send a report" stored a zip and listed it on the desk, and
 * nothing else: a person whose two builds had failed sent one, and it was
 * seen only because the founder happened to open the desk. Now the hub
 * mails him at once, with what the person asked and what went wrong read
 * out of the zip, so the mail alone says whether it needs him.
 *
 * Driven against the real worker with a stored zip built here, a Map for KV
 * and a stubbed fetch, so nothing leaves the machine.
 *
 *   node --experimental-wasm-modules cloud/worker/test/report-notifies-founder.test.mjs
 */
import assert from "node:assert/strict";
import { crc32 } from "node:zlib";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";
import "./github-stub.mjs";

/// A stored (uncompressed) zip, the shape Studio's report takes.
function zip(files) {
  const enc = new TextEncoder();
  const locals = [];
  const central = [];
  let offset = 0;
  for (const [name, text] of Object.entries(files)) {
    const n = enc.encode(name);
    const data = enc.encode(text);
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(n.length, 26);
    const dir = Buffer.alloc(46);
    dir.writeUInt32LE(0x02014b50, 0);
    dir.writeUInt16LE(20, 4);
    dir.writeUInt16LE(20, 6);
    dir.writeUInt32LE(crc, 16);
    dir.writeUInt32LE(data.length, 20);
    dir.writeUInt32LE(data.length, 24);
    dir.writeUInt16LE(n.length, 28);
    dir.writeUInt32LE(offset, 42);
    locals.push(local, Buffer.from(n), Buffer.from(data));
    central.push(dir, Buffer.from(n));
    offset += 30 + n.length + data.length;
  }
  const dirBytes = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(Object.keys(files).length, 8);
  end.writeUInt16LE(Object.keys(files).length, 10);
  end.writeUInt32LE(dirBytes.length, 12);
  end.writeUInt32LE(offset, 16);
  return new Uint8Array(Buffer.concat([...locals, dirBytes, end]));
}

const kv = new Map();
const env = {
  APPS: {
    get: async (k) => kv.get(k) ?? null,
    put: async (k, v) => { kv.set(k, v); },
    delete: async (k) => { kv.delete(k); },
    list: async ({ prefix }) => ({ keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
  },
  BUNDLES: r2Mock(new Map()),
  PUBLIC_BASE: "https://hub.example",
  KRATE_ADMINS: "boss",
  RESEND_API_KEY: "resend-key",
  FOUNDER_NOTIFY: "founder@example.com",
};

const sent = [];
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String(input.url || input);
  if (url === "https://api.resend.com/emails") {
    sent.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
  }
  return realFetch(input, init);
};

const post = (body, headers = {}) =>
  worker.fetch(new Request("https://hub.example/report", {
    method: "POST",
    headers: { "content-type": "application/zip", "x-krate-session": "s-1", "x-krate-os": "Win32", "x-krate-version": "studio-web", ...headers },
    body,
  }), env);

/* ---- a failed build's report says what was asked and what went wrong --- */
{
  const report = zip({
    "session.json": JSON.stringify({
      id: "s-1",
      title: "A desktop simulator",
      messages: [
        { who: "YOU", body: "A desktop simulator" },
        { who: "KRATE", body: "Here's what I'll build" },
        { who: "YOU", body: "Build a desktop simulator with draggable windows" },
        { who: "KRATE", kind: "fail", body: "That build didn't come together." },
        { who: "KRATE", kind: "fail", body: "That build didn't come together." },
      ],
    }),
    "about.txt": "Krate Studio, in a browser\npage: https://krate.tech/app/\nplatform: Win32\n",
    "build.json": JSON.stringify({ state: "failed", error: "That one grew bigger than a single build can carry." }),
  });
  const r = await post(report);
  assert.equal(r.status, 200, await r.clone().text());
  const { id } = await r.json();
  assert.equal(sent.length, 1, "one mail, at once");
  const mail = sent[0];
  assert.deepEqual(mail.to, ["founder@example.com"], "to the founder's inboxes, as sign-ups are");
  assert.match(mail.subject, /^Report from Someone without an account: That one grew bigger/);
  for (const line of [
    "They asked: Build a desktop simulator with draggable windows",
    "What went wrong: That one grew bigger than a single build can carry.",
    "Failed builds: 2",
    "Session: A desktop simulator",
    "Where: Studio on the web (https://krate.tech/app/) · Win32 · Krate studio-web",
    `Report: ${id}`,
    "https://hub.krate.tech/admin#reports",
  ]) {
    assert.ok(mail.text.includes(line), `the mail says "${line}":\n${mail.text}`);
  }
  assert.match(mail.html, /Open the report/, "the designed copy has the button");
  assert.equal(JSON.parse(kv.get(`report:${id}`)).state, "new", "the report is stored as before");
  console.log("ok  a report mails the founder what was asked and what went wrong");
}

/* ---- a zip that cannot be read still mails, and the report still lands -- */
{
  sent.length = 0;
  const r = await post(new Uint8Array([0x50, 0x4b, 1, 2, 3, 4, 5]), { "x-krate-note": "it froze" });
  assert.equal(r.status, 200);
  assert.equal(sent.length, 1, "an unreadable zip still tells the founder");
  assert.ok(sent[0].text.includes("Their note: it froze"), sent[0].text);
  console.log("ok  an unreadable zip still mails, with what the headers said");
}

/* ---- the mail failing never fails the report ---------------------------- */
{
  sent.length = 0;
  const before = globalThis.fetch;
  globalThis.fetch = async (input, init) => {
    if (String(input.url || input) === "https://api.resend.com/emails") throw new Error("resend is down");
    return before(input, init);
  };
  const quiet = console.error;
  console.error = () => {};
  const r = await post(zip({ "about.txt": "Krate Studio\n" }));
  console.error = quiet;
  globalThis.fetch = before;
  assert.equal(r.status, 200, "the person's report is taken even when the mail cannot go");
  console.log("ok  a mail that cannot go never costs the person their report");
}

/* ---- the desk link opens on Reports ------------------------------------- */
{
  const { readFileSync } = await import("node:fs");
  const page = readFileSync(new URL("../src/index.js", import.meta.url), "utf8");
  assert.ok(page.includes('location.hash==="#reports"?"reports"'), "the desk opens #reports on the Reports tab");
  console.log("ok  the mail's link opens the desk on Reports");
}
