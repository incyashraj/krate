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
  KRATE_BUILDER_URL: "https://builder.example",
  KRATE_BUILDER_SECRET: "builder-secret",
};

const BUILD = "cc9eb2165bfefae27d7965e41bfdb5fe";
const sent = [];
const asked = [];
const realFetch = globalThis.fetch;
globalThis.fetch = async (input, init = {}) => {
  const url = String(input.url || input);
  if (url === "https://api.resend.com/emails") {
    sent.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
  }
  // The builder's support door (cloud/builder /support/build/), as it answers.
  if (url.startsWith("https://builder.example/support/build/")) {
    asked.push({ url, secret: new Headers(init.headers).get("x-krate-builder") });
    if (!url.endsWith(BUILD)) return new Response("no such build", { status: 404 });
    return new Response(JSON.stringify({
      id: BUILD, state: "failed", stage: "test", line: "checking it builds", kind: "build",
      error: "That one grew bigger than a single build can carry.",
      started: Date.parse("2026-11-23T00:01:25Z"), finished: Date.parse("2026-11-23T00:10:32Z"),
      usd: 2.58, rounds: 16, paidBy: "krate", spend: { usd: 2.58, rounds: 16, model: "claude-opus-5" },
      account: "sam", request: "Build a desktop simulator with draggable windows",
      recent: [{ id: "a728f33ab8de5e4e8575811282f1fd17", state: "failed", started: Date.parse("2026-11-22T23:48:33Z"), usd: 2.5825, rounds: 15, error: "That one grew bigger than a single build can carry." }],
      log: Array.from({ length: 80 }, (_, i) => `engine line ${i + 1}`).join("\n") + "\n",
      logBytes: 90000,
    }), { headers: { "content-type": "application/json" } });
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
    "build.json": JSON.stringify({ id: BUILD, state: "failed", stage: "read", line: "error: author command failed", error: "That one grew bigger than a single build can carry." }),
  });
  const r = await post(report);
  assert.equal(r.status, 200, await r.clone().text());
  const { id } = await r.json();
  assert.equal(sent.length, 1, "one mail, at once");
  const mail = sent[0];
  assert.deepEqual(mail.to, ["founder@example.com"], "to the founder's inboxes, as sign-ups are");
  assert.equal(mail.reply_to, undefined, "a reply goes from the desk, never from this inbox");
  assert.match(mail.subject, /^Report from Someone without an account: That one grew bigger/);
  assert.deepEqual(asked, [{ url: `https://builder.example/support/build/${BUILD}`, secret: "builder-secret" }],
    "the hub asks the builder about the reported build, with the shared secret");
  for (const line of [
    // who, what, where
    "They asked: Build a desktop simulator with draggable windows",
    "What they saw: That one grew bigger than a single build can carry.",
    "Failed builds in this session: 2",
    "App: A desktop simulator",
    "Where: Studio on the web (https://krate.tech/app/) · Win32 · Krate studio-web",
    `Report: ${id} · session s-1`,
    // the build, from the builder's own record
    `Build: ${BUILD}`,
    "Ended: failed at test",
    "Cost: $2.58 · 16 rounds · claude-opus-5 · paid by krate",
    "Took: 9 min",
    "Last engine line: checking it builds",
    // the account's other builds
    "failed  $2.58 15 rounds  a728f33ab8de5e4e8575811282f1fd17",
    // the whole conversation, in order
    "THEM: A desktop simulator",
    "KRATE: Here's what I'll build",
    "KRATE (failed): That build didn't come together.",
    // the end of the engine log, and where the rest is
    "-- The engine log, last 60 lines of 88 KB --",
    "engine line 21\nengine line 22",
    "engine line 80",
    `fly ssh console -a krate-builder -C "cat /work/state/logs/${BUILD}.log"`,
    "-- In the report --\nsession.json\nabout.txt\nbuild.json",
    "https://hub.krate.tech/admin#reports",
  ]) {
    assert.ok(mail.text.includes(line), `the mail says "${line}":\n${mail.text}`);
  }
  assert.ok(!mail.text.includes("engine line 20\n"), "only the last 60 log lines are in the body");
  assert.match(mail.html, /Open the report/, "the designed copy has the button");
  assert.match(mail.html, /engine line 80/, "and the log");
  const names = mail.attachments.map((a) => a.filename);
  assert.deepEqual(names, [`krate-report-${id}.zip`, `engine-${BUILD}.log`], "the report and the engine log are attached");
  assert.equal(Buffer.from(mail.attachments[0].content, "base64").length, report.length, "the whole report, byte for byte");
  assert.match(Buffer.from(mail.attachments[1].content, "base64").toString(), /^engine line 1\n[\s\S]*engine line 80\n$/, "the whole log the builder gave");
  assert.equal(JSON.parse(kv.get(`report:${id}`)).state, "new", "the report is stored as before");
  console.log("ok  a report mails the founder everything needed to find the fault");
}

/* ---- a builder that does not answer still leaves a full mail ----------- */
{
  sent.length = 0;
  const report = zip({
    "session.json": JSON.stringify({ id: "s-2", title: "x", messages: [{ who: "YOU", body: "a timer" }] }),
    "build.json": JSON.stringify({ id: "dddd4444dddd4444dddd4444dddd4444", state: "failed", error: "boom" }),
  });
  const r = await post(report);
  assert.equal(r.status, 200);
  assert.ok(sent[0].text.includes("Build: dddd4444dddd4444dddd4444dddd4444 (the builder did not answer"), sent[0].text);
  assert.deepEqual(sent[0].attachments.map((a) => a.filename).length, 1, "the report still travels");
  console.log("ok  without the builder's record the mail still names the build");
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
