/* The receive loop (F-320, K-1132): what a person who was sent a link
 * meets, pinned against the REAL worker.
 *
 *   - an app opened in a browser tab is counted per person: kind "open",
 *     client "tab", the app, home|cloud, the daily visitor hash, whether
 *     the opener is the maker, and the post's ?via= tag; never in KV;
 *   - a shared /a/ link tapped is counted per person too, with its tag,
 *     and the tag rides on to the tab;
 *   - every chat app's previewer gets a card with the app's name, one line
 *     and its picture; a person's browser and the runtime get what they got
 *     before; a team's private app names nothing;
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/receive-loop.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, acme, fixture, worker, tokenFor } from "./org-tools.mjs";
import { sha256Hex } from "./r2-mock.mjs";
import { WEB_PACK_TAG } from "../src/web-player.js";
import { PREVIEWERS, cardDescription } from "../src/receive.js";

const here = dirname(fileURLToPath(import.meta.url));
const BOUNCE = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0]);

const BROWSER = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128 Safari/537.36";
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

/* Each previewer as it really asks: its user agent and its Accept. */
const PREVIEWER_UAS = {
  whatsapp: ["WhatsApp/2.24.20.80 A", "*/*"],
  imessage: ["Mozilla/5.0 (Macintosh; Intel Mac OS X 10_11_1) AppleWebKit/601.2.4 (KHTML, like Gecko) Version/9.0.1 Safari/601.2.4 facebookexternalhit/1.1 Facebot Twitterbot/1.0", "text/html,application/xhtml+xml,*/*"],
  slack: ["Slackbot-LinkExpanding 1.0 (+https://api.slack.com/robots)", "*/*"],
  discord: ["Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)", "*/*"],
  x: ["Twitterbot/1.0", "*/*"],
  linkedin: ["LinkedInBot/1.0 (compatible; Mozilla/5.0; Apache-HttpClient +http://www.linkedin.com)", "text/html"],
  facebook: ["facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)", "*/*"],
  telegram: ["TelegramBot (like TwitterBot)", "text/html"],
};

/* In-app browsers are people: the user agents a tap inside the app sends. */
const IN_APP = {
  linkedin: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 [LinkedInApp]/9.30",
  facebook: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 [FBAN/FBIOS;FBAV/480.0]",
  instagram: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 Instagram 350.0",
};

/* Player assets as Workers static assets would hold them. */
function withAssets(env) {
  const files = { "/run.html": "<!doctype html><title>Player</title>", "/krate-card.png": PNG };
  env.ASSETS = {
    fetch: async (request) => {
      const path = new URL(request.url).pathname;
      return files[path] ? new Response(files[path]) : new Response("not found", { status: 404 });
    },
  };
  return env;
}

/* A hub with bounce.krate published by alice, the points it writes, and a
 * count of KV writes after setup. */
async function hub() {
  const env = withAssets(orgEnv({ VISITOR_SALT: "test-salt", KRATE_ADMINS: "boss" }));
  env.points = [];
  env.USAGE = { writeDataPoint: (p) => env.points.push(p) };
  const hash = await sha256Hex(BOUNCE);
  const up = await call(env, "alice", "POST", "/publish", { bytes: BOUNCE, headers: { "x-krate-description": "A ball that bounces off the walls" } });
  assert.equal(up.status, 200, JSON.stringify(up.body));
  env.points.length = 0;
  let writes = 0;
  const put = env.APPS.put;
  env.APPS.put = async (...a) => {
    writes += 1;
    return put(...a);
  };
  env.kvWrites = () => writes;
  return { env, hash };
}

/* One request with a user agent, an address and Cloudflare's network name. */
async function ask(env, method, path, { ua = BROWSER, accept, ip = "203.0.113.9", org = "Comcast Cable", body, who } = {}) {
  const headers = { "user-agent": ua, "cf-connecting-ip": ip };
  if (accept) headers.accept = accept;
  if (body !== undefined) headers["content-type"] = "application/json";
  if (who) headers.authorization = `Bearer ${tokenFor(env, who)}`;
  const req = new Request(`https://hub.example${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body),
    redirect: "manual",
  });
  Object.defineProperty(req, "cf", { value: { asOrganization: org, country: "SG" } });
  const res = await worker.fetch(req, env, { waitUntil() {} });
  const raw = await res.arrayBuffer();
  const textBody = new TextDecoder().decode(raw);
  let parsed = textBody;
  try {
    parsed = JSON.parse(textBody);
  } catch (_) {}
  return { status: res.status, headers: res.headers, text: textBody, body: parsed, bytes: new Uint8Array(raw) };
}

const tag = (html, property) => {
  const m = new RegExp(`<meta (?:property|name)="${property}" content="([^"]*)"`).exec(html);
  return m ? m[1].replace(/&amp;/g, "&").replace(/&#39;/g, "'").replace(/&quot;/g, '"') : null;
};

// ------------------------------------------------------------- the open

test("an app opened in a tab is one point per person: open, tab, the app, home, the visitor, not the maker, the post's tag", async () => {
  const { env, hash } = await hub();
  const res = await ask(env, "POST", "/tab", { body: { event: "open", app: hash, via: "Reddit" } });
  assert.equal(res.status, 200);
  assert.deepEqual(res.body, { ok: true, maker: false });
  assert.equal(env.points.length, 1);
  const [p] = env.points;
  assert.deepEqual(p.blobs, [
    "open", "-", "tab", "ok", "-", new Date().toISOString().slice(0, 10), "-",
    hash, "home", "other", "reddit", "public", "human",
  ]);
  assert.match(p.indexes[0], /^[0-9a-f]{24}$/, "the daily visitor hash, never an address");
  assert.ok(!JSON.stringify(p).includes("203.0.113.9"));
  assert.equal(env.kvWrites(), 0, "counting writes no KV");

  // The same browser again is the same visitor; another address is another.
  await ask(env, "POST", "/tab", { body: { event: "open", app: hash } });
  await ask(env, "POST", "/tab", { body: { event: "open", app: hash }, ip: "198.51.100.4" });
  assert.equal(env.points[1].indexes[0], p.indexes[0]);
  assert.notEqual(env.points[2].indexes[0], p.indexes[0]);
  assert.equal(env.points[1].blobs[10], "-", "no tag is '-'");
});

test("the maker opening their own app is marked maker; anyone else signed in is other", async () => {
  const { env, hash } = await hub();
  const mine = await ask(env, "POST", "/tab", { body: { event: "open", app: hash }, who: "alice" });
  assert.deepEqual(mine.body, { ok: true, maker: true });
  assert.equal(env.points[0].blobs[9], "maker");
  const theirs = await ask(env, "POST", "/tab", { body: { event: "open", app: hash }, who: "bob" });
  assert.equal(theirs.body.maker, false);
  assert.equal(env.points[1].blobs[9], "other");
});

test("an open from a cloud network or a headless browser is marked so; a made-up or unlisted app is handled", async () => {
  const { env, hash } = await hub();
  await ask(env, "POST", "/tab", { body: { event: "open", app: hash }, org: "Amazon.com, Inc." });
  assert.equal(env.points[0].blobs[8], "cloud");
  await ask(env, "POST", "/tab", { body: { event: "open", app: hash }, ua: "Mozilla/5.0 HeadlessChrome/128" });
  assert.equal(env.points[1].blobs[12], "bot");
  // A hash the hub does not hold is not an open: nothing is counted.
  const fake = await ask(env, "POST", "/tab", { body: { event: "open", app: "e".repeat(64) } });
  assert.equal(fake.status, 204);
  for (const body of [{ event: "open", app: "short" }, { event: "delete", app: hash }, "not json"]) {
    assert.equal((await ask(env, "POST", "/tab", { body })).status, 204);
  }
  assert.equal(env.points.length, 2);
  // Junk in the tag is dropped, not stored.
  await ask(env, "POST", "/tab", { body: { event: "open", app: hash, via: "<script>alert(1)</script>" } });
  assert.equal(env.points[2].blobs[10], "-");
  // An unlisted app is counted as one.
  const record = JSON.parse(env._kv.get(`app:${hash}`));
  env._kv.set(`app:${hash}`, JSON.stringify({ ...record, unlisted: true }));
  await ask(env, "POST", "/tab", { body: { event: "open", app: hash } });
  assert.equal(env.points[3].blobs[11], "unlisted");
});

test("Make your own pressed in the Player is its own kind, credited to the app", async () => {
  const { env, hash } = await hub();
  await ask(env, "POST", "/tab", { body: { event: "make", app: hash, via: "player" } });
  assert.deepEqual(env.points[0].blobs.slice(0, 4), ["badge", "-", "tab", "ok"]);
  assert.equal(env.points[0].blobs[7], hash);
});

test("a team's private app opened in a tab is counted as org, with no maker named", async () => {
  const env = withAssets(orgEnv({ VISITOR_SALT: "s" }));
  env.points = [];
  env.USAGE = { writeDataPoint: (p) => env.points.push(p) };
  await acme(env);
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  assert.equal(up.status, 201, JSON.stringify(up.body));
  env.points.length = 0;
  await ask(env, "POST", "/tab", { body: { event: "open", app: up.body.digest }, who: "dan" });
  assert.equal(env.points.length, 1);
  assert.deepEqual([env.points[0].blobs[9], env.points[0].blobs[11]], ["-", "org"]);
});

// ------------------------------------------------------------ the link

test("a tapped link is counted per person with its tag, and the tag rides on to the tab", async () => {
  const { env, hash } = await hub();
  const desk = await ask(env, "GET", `/a/${hash}?via=hn`, { accept: "text/html" });
  assert.equal(desk.status, 302);
  assert.equal(desk.headers.get("location"), `https://krate.tech/open/?a=${hash}&via=hn`);
  const p = env.points[0];
  assert.deepEqual(p.blobs.slice(0, 5), ["link", "-", "desktop-browser", "ok", "landing"]);
  assert.deepEqual(p.blobs.slice(6), ["hn", hash, "home"]);
  assert.match(p.indexes[0], /^[0-9a-f]{24}$/, "a person, not the app, is the index");

  const phone = await ask(env, "GET", `/a/${hash}?via=hn`, { ua: IPHONE, accept: "text/html" });
  assert.equal(phone.status, 200);
  assert.ok(phone.text.includes(`/run/${hash}?via=hn`), "the phone's Open it carries the tag");
  const plain = await ask(env, "GET", `/a/${hash}`, { accept: "text/html" });
  assert.equal(plain.headers.get("location"), `https://krate.tech/open/?a=${hash}`, "no tag, no parameter");
  assert.equal(env.kvWrites(), 0);
});

// ------------------------------------------------------------ the card

for (const [who, [ua, accept]] of Object.entries(PREVIEWER_UAS)) {
  test(`${who}'s previewer gets the app's card: name, one line, its picture, large`, async () => {
    const { env, hash } = await hub();
    await env.BUNDLES.put(`shot:${hash}`, PNG, { httpMetadata: { contentType: "image/png" } });
    const name = JSON.parse(env._kv.get(`app:${hash}`)).name;
    assert.ok(name, "the published app has a name");
    const res = await ask(env, "GET", `/a/${hash}`, { ua, accept });
    assert.equal(res.status, 200, res.text);
    assert.match(res.headers.get("content-type"), /text\/html/);
    assert.equal(tag(res.text, "og:title"), name);
    assert.equal(tag(res.text, "og:description"), "A ball that bounces off the walls. Made with Krate: opens in your browser with a tap, nothing to install.");
    assert.equal(tag(res.text, "og:image"), `https://hub.example/shot/${hash}`);
    assert.equal(tag(res.text, "og:url"), `https://hub.example/a/${hash}`);
    assert.equal(tag(res.text, "twitter:card"), "summary_large_image");
    assert.ok(res.text.includes(`<link rel="canonical" href="https://hub.example/a/${hash}">`));
    assert.match(res.text, /<meta name="robots" content="noindex">/);
    // Counted as a preview, not as a person's tap.
    assert.equal(env.points[0].blobs[2], "preview");
    // HEAD, which some ask first, answers the same way.
    assert.equal((await ask(env, "HEAD", `/a/${hash}`, { ua, accept })).status, 200);
    // `?dl=1` is still the bytes for anyone.
    const dl = await ask(env, "GET", `/a/${hash}?dl=1`, { ua, accept });
    assert.equal(dl.headers.get("content-type"), "application/octet-stream");
  });
}

test("every named previewer is recognised, and no in-app browser is", () => {
  for (const [ua] of Object.values(PREVIEWER_UAS)) assert.ok(PREVIEWERS.test(ua), ua);
  for (const ua of Object.values(IN_APP)) assert.ok(!PREVIEWERS.test(ua), ua);
  for (const ua of [BROWSER, IPHONE, "krate/0.5.5", "curl/8.4.0"]) assert.ok(!PREVIEWERS.test(ua), ua);
});

test("a person's browser, a phone, an in-app browser and the runtime get what they got before", async () => {
  const { env, hash } = await hub();
  assert.equal((await ask(env, "GET", `/a/${hash}`, { accept: "text/html" })).status, 302);
  const phone = await ask(env, "GET", `/a/${hash}`, { ua: IPHONE, accept: "text/html" });
  assert.ok(phone.text.includes("Open it") && !phone.text.includes("og:title"));
  const inApp = await ask(env, "GET", `/a/${hash}`, { ua: IN_APP.linkedin, accept: "text/html" });
  assert.ok(inApp.text.includes("Open it"), "LinkedIn's in-app browser is a phone, not a previewer");
  const runtime = await ask(env, "GET", `/a/${hash}`, { ua: "krate/0.5.5", accept: "*/*" });
  assert.equal(runtime.headers.get("content-type"), "application/octet-stream");
  assert.deepEqual([...runtime.bytes], [...BOUNCE]);
});

test("an app with no picture gets Krate's card at 1200 x 630; one that opens only in Krate says so", async () => {
  const { env, hash } = await hub();
  const res = await ask(env, "GET", `/a/${hash}`, { ua: PREVIEWER_UAS.whatsapp[0] });
  assert.equal(tag(res.text, "og:image"), "https://hub.example/player/krate-card.png");
  assert.equal(tag(res.text, "og:image:width"), "1200");
  assert.equal(tag(res.text, "og:image:height"), "630");
  const card = await worker.fetch(new Request("https://hub.example/player/krate-card.png"), env);
  assert.equal(card.status, 200);
  assert.equal(card.headers.get("cross-origin-resource-policy"), "cross-origin", "a chat app may load it");
  // The transpiler refused this one: it is not said to open in a browser.
  await env.BUNDLES.put(`web/${WEB_PACK_TAG}/${hash}`, new Uint8Array(0), { customMetadata: { failed: "needs a database" } });
  const native = await ask(env, "GET", `/a/${hash}`, { ua: PREVIEWER_UAS.whatsapp[0] });
  assert.equal(tag(native.text, "og:description"), "A ball that bounces off the walls. Made with Krate: opens as an app on Mac, Windows and Linux.");
  assert.ok(!/browser/.test(tag(native.text, "og:description")));
});

test("the card says what the app is even with no description of its own", () => {
  assert.equal(cardDescription("Notes", "", "tab"), "Notes, made with Krate: opens in your browser with a tap, nothing to install.");
  assert.equal(cardDescription("Notes", "Quick notes!", ""), "Quick notes! Made with Krate: tap to open it.");
});

test("a name with quotes and angle brackets is escaped in the card", async () => {
  const { env, hash } = await hub();
  const record = JSON.parse(env._kv.get(`app:${hash}`));
  env._kv.set(`app:${hash}`, JSON.stringify({ ...record, name: `"><script>x()</script>`, description: "" }));
  const res = await ask(env, "GET", `/a/${hash}`, { ua: PREVIEWER_UAS.slack[0] });
  assert.ok(!res.text.includes("<script>"), res.text);
  assert.equal(tag(res.text, "og:title"), "&quot;&gt;&lt;script&gt;x()&lt;/script&gt;".replace(/&quot;/g, '"'));
});

test("a /run/ link gets the same card; the Player page is still the Player for a person", async () => {
  const { env, hash } = await hub();
  const card = await ask(env, "GET", `/run/${hash}`, { ua: PREVIEWER_UAS.discord[0] });
  assert.equal(tag(card.text, "og:url"), `https://hub.example/a/${hash}`);
  const page = await ask(env, "GET", `/run/${hash}`);
  assert.match(page.text, /<title>Player<\/title>/);
  assert.equal(page.headers.get("cross-origin-embedder-policy"), "require-corp");
});

test("a team's private app: the card names no app, no team and shows no picture", async () => {
  const env = withAssets(orgEnv());
  await acme(env);
  const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes: fixture("candidate-v1.krate") });
  const digest = up.body.digest;
  assert.equal((await call(env, "bob", "POST", `/orgs/acme/candidates/${digest}/approve`)).status, 200);
  await env.BUNDLES.put(`shot:${digest}`, PNG);
  for (const path of [`/a/${digest}`, `/run/${digest}`]) {
    const res = await ask(env, "GET", path, { ua: PREVIEWER_UAS.slack[0] });
    assert.equal(res.status, 200, res.text);
    assert.equal(tag(res.text, "og:title"), "A private app");
    assert.equal(tag(res.text, "og:image"), "https://hub.example/player/krate-card.png");
    assert.ok(!/acme|Acme|candidate|shot\//i.test(res.text), `${path} says nothing about the app or the team`);
    assert.match(res.headers.get("cache-control"), /no-store/);
  }
  // A person's browser still goes to the sign-in, the runtime still asks.
  assert.equal((await ask(env, "GET", `/a/${digest}`, { accept: "text/html" })).status, 303);
});

test("a taken-down app gets no card, only the notice", async () => {
  const { env, hash } = await hub();
  env._kv.set(`takedown:${hash}`, JSON.stringify({ hash, reason: "a scam", scope: "listing", at: 1 }));
  for (const path of [`/a/${hash}`, `/run/${hash}`]) {
    const res = await ask(env, "GET", path, { ua: PREVIEWER_UAS.facebook[0] });
    assert.equal(res.status, 451);
    assert.ok(!res.text.includes("og:title"));
  }
});
