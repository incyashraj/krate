/* A developer's own name: alice.krate.tech (K-849).
 *
 * The hub has always kept a channel per published app --
 * `channel:<login>/<slug>`, mutable, with history and rollback, moved by
 * that publisher alone. Nothing ever showed it to a person. This serves it
 * on a name a developer can put in their own README, and the same URL has
 * to answer two callers: a browser gets a page, a runtime gets the bytes.
 *
 * Driven against the real handler with a real published bundle, because
 * the interesting parts are the ones a mock would paper over: that the hub
 * still answers on its own hostname, that a reserved name can never be a
 * person, and that a publisher-supplied app name cannot put script into
 * somebody else's browser.
 *
 *   node --experimental-wasm-modules cloud/worker/test/developer-subdomain.test.mjs
 */
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

// Real bundles: the publish door validates, so a handful of bytes is
// refused as "not a zip archive" and would test nothing.
const here = dirname(fileURLToPath(import.meta.url));
const V1 = new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", "bounce.krate")));
const V2 = new Uint8Array(readFileSync(join(here, "fixtures", "bounce-stored.krate")));

function env() {
  const kv = new Map([
    ["session:krs_alice", "u1"],
    ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      delete: async (k) => { kv.delete(k); },
      list: async ({ prefix }) => ({
        keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })),
        list_complete: true,
      }),
    },
    BUNDLES: r2Mock(),
    PUBLIC_BASE: "https://hub.krate.tech",
    _kv: kv,
  };
}

const BROWSER = { accept: "text/html,application/xhtml+xml" };
const TOOL = { accept: "*/*" };

const hit = async (e, urlString, headers = TOOL) => {
  const res = await worker.fetch(new Request(urlString, { headers }), e);
  return { status: res.status, headers: res.headers, body: await res.text() };
};

async function publish(e, name, bytes) {
  const res = await worker.fetch(
    new Request("https://hub.krate.tech/publish", {
      method: "POST",
      headers: { authorization: "Bearer krs_alice", "x-krate-name": name },
      body: bytes,
    }),
    e,
  );
  assert.strictEqual(res.status, 200, await res.clone().text());
  return JSON.parse(await res.text());
}

// ---- the hub must keep working ------------------------------------------
{
  const e = env();
  const health = await hit(e, "https://hub.krate.tech/health");
  assert.strictEqual(health.status, 200, "the hub still answers on its own hostname");
  console.log("ok  the hub's own hostname is untouched");
}

// ---- a reserved name is never a person ----------------------------------
{
  const e = env();
  for (const name of ["hub", "www", "api", "app", "studio", "admin"]) {
    const res = await hit(e, `https://${name}.krate.tech/health`);
    assert.strictEqual(
      res.status,
      200,
      `${name}.krate.tech must reach the hub, not a developer page`,
    );
  }
  console.log("ok  service names cannot be claimed as developers");
}

// ---- one label only -----------------------------------------------------
{
  const e = env();
  // Free Universal SSL covers one level; a second label has no certificate
  // and must not be treated as a developer.
  // Publish first, so `alice` really is a developer with a page: a 404
  // from a host that was never going to resolve proves nothing.
  await publish(e, "Notes", V1);
  const real = await hit(e, "https://alice.krate.tech/", BROWSER);
  assert.strictEqual(real.status, 200, "one label reaches her page");
  const deep = await hit(e, "https://beta.alice.krate.tech/", BROWSER);
  assert.notStrictEqual(deep.status, 200, "a two-label host is not a developer");
  assert.ok(
    !deep.body.includes("Notes"),
    "and must not serve her page on a hostname with no certificate",
  );
  console.log("ok  only one label is a developer name");
}

// ---- the page, and the bytes, on the same URL ---------------------------
{
  const e = env();
  const published = await publish(e, "Notes", V1);
  assert.ok(published.id, "the app published");

  // A person's browser gets a page naming the app.
  const page = await hit(e, "https://alice.krate.tech/", BROWSER);
  assert.strictEqual(page.status, 200, "alice has a page");
  assert.match(page.headers.get("content-type") || "", /text\/html/, "served as HTML");
  assert.match(page.body, /Notes/, "the page names her app");
  assert.match(page.body, /alice/, "the page names her");

  // A tool asking the same host gets JSON, not a page.
  const api = await hit(e, "https://alice.krate.tech/");
  assert.match(api.headers.get("content-type") || "", /application\/json/, "a tool gets JSON");
  assert.deepStrictEqual(JSON.parse(api.body).apps, ["notes"], "and it lists her apps");

  // The app's own address: a browser gets its page...
  const appPage = await hit(e, "https://alice.krate.tech/notes", BROWSER);
  assert.strictEqual(appPage.status, 200, "the app has a page");
  assert.match(appPage.body, /krate run alice\.krate\.tech\/notes/, "it shows the runnable address");

  // ...and a runtime gets sent to the fixed content address, never bytes
  // from a mutable name.
  const run = await hit(e, "https://alice.krate.tech/notes");
  assert.strictEqual(run.status, 302, "a runtime is redirected");
  const location = run.headers.get("location") || "";
  assert.match(location, /\/a\/[0-9a-f]{64}\?dl=1$/, "to the content address it can verify");
  console.log("ok  a browser gets the page and a runtime gets the bytes");
}

// ---- an app that does not exist -----------------------------------------
{
  const e = env();
  const missing = await hit(e, "https://alice.krate.tech/nothing", BROWSER);
  assert.strictEqual(missing.status, 404, "an unknown app is a 404");
  assert.match(missing.body, /no app called/, "and says so in words");
  console.log("ok  a missing app says so rather than failing blankly");
}

// ---- publisher text cannot become script --------------------------------
{
  const e = env();
  await publish(e, "<script>alert(1)</script>", V2);
  const page = await hit(e, "https://alice.krate.tech/", BROWSER);
  assert.ok(
    !page.body.includes("<script>alert(1)</script>"),
    "a publisher-supplied name is escaped, not executed, in a stranger's browser",
  );
  assert.match(page.body, /&lt;script&gt;/, "and is shown as the text it is");
  console.log("ok  publisher text is escaped on the way out");
}

// ---- a name nobody has published under ----------------------------------
//
// The wildcard record means EVERY name reaches the worker, so this is the
// common case, not an edge one. It used to render the empty developer page
// at 200 -- "Apps by nobody-here" for somebody who had never signed in --
// which made a real developer's page indistinguishable from a stranger's
// and, with robots.txt allowing everything, minted an unbounded set of
// indexable pages asserting people are on Krate (K-856).
{
  const e = env();
  const page = await hit(e, "https://nobody-here.krate.tech/", BROWSER);
  assert.strictEqual(page.status, 404, "an unpublished name is not a developer");
  assert.ok(
    !page.body.includes("Apps by nobody-here"),
    "and must not be described as one: " + page.body.slice(0, 200),
  );
  assert.match(page.body, /Nothing published here/, "it says so in words");
  assert.match(
    page.body,
    /<meta name="robots" content="noindex/,
    "and tells crawlers not to index a page about a person who may not exist",
  );
  // The words have to be true. The hub cannot tell a real login from a
  // made-up one -- there is no login index -- so the page must not claim
  // the person does not exist, only that nothing is published.
  assert.ok(
    !/no such (person|user|developer)/i.test(page.body),
    "it must not assert a fact the hub cannot know: " + page.body.slice(0, 200),
  );

  // A tool gets the same answer, in its own shape.
  const api = await hit(e, "https://nobody-here.krate.tech/");
  assert.strictEqual(api.status, 404, "a tool is told the same thing");
  assert.match(api.headers.get("content-type") || "", /application\/json/);

  // And the links on that page have to go somewhere real. /apps answers
  // JSON to a browser, so pointing a person at it would hand them raw
  // JSON from a page written to help them.
  assert.ok(
    !/href="[^"]*hub\.krate\.tech\/apps"/.test(page.body),
    "the gallery link must not be the hub's JSON route: " + page.body.slice(0, 400),
  );
  console.log("ok  a name nobody published under is not served as a developer");
}

// ---- a real developer is untouched ---------------------------------------
//
// The half that keeps the fix honest: a 404 for everyone would also pass
// every assertion above.
{
  const e = env();
  await publish(e, "Notes", V1);
  const page = await hit(e, "https://alice.krate.tech/", BROWSER);
  assert.strictEqual(page.status, 200, "somebody who published still has a page");
  assert.match(page.body, /Notes/, "with their app on it");
  assert.ok(
    !/noindex/.test(page.body),
    "and it stays indexable -- this page is the point of the feature",
  );
  console.log("ok  publishing one app is what turns a name into a developer");
}

// ---- an app published before channels existed ----------------------------
//
// THE BUG THAT REACHED PEOPLE. A channel is only written on a LISTED
// publish, and channels were added long after publishing was (IC-389), so
// most published apps have none: of 20 real apps across four people on the
// live hub, exactly ONE had a channel. developerApps read channels alone,
// so three of the four developers' pages were empty -- and once K-856 made
// an empty page a 404, three real developers' names stopped resolving.
// Reported by the founder, whose own page worked, which is exactly why
// nobody saw it.
//
// Seeded as an `app:` record with no channel, which is what those 20 apps
// actually look like in KV.
{
  const e = env();
  const hash = "c".repeat(64);
  e._kv.set(
    `app:${hash}`,
    JSON.stringify({
      name: "Weather",
      description: "what it does outside",
      author_login: "aanchalabhongade",
      published: 1788339400,
      size: 109247,
    }),
  );

  const page = await hit(e, "https://aanchalabhongade.krate.tech/", BROWSER);
  assert.strictEqual(page.status, 200, "a real publisher has a page, channel or not");
  assert.match(page.body, /Weather/, "and their app is on it");
  assert.match(
    page.body,
    new RegExp(`/a/${hash}`),
    "linked at the content address, since no channel can serve /<slug>",
  );

  const api = await hit(e, "https://aanchalabhongade.krate.tech/");
  assert.strictEqual(api.status, 200, "a tool is told the same");
  const body = JSON.parse(api.body);
  assert.deepStrictEqual(
    body.apps,
    [],
    "`apps` holds only names this host can resolve -- a hash is not one",
  );
  assert.strictEqual(body.published.length, 1, "but the app is still reported");
  assert.strictEqual(body.published[0].url, `https://hub.krate.tech/a/${hash}`);
  console.log("ok  an app published before channels still reaches its author's page");
}

// ---- what must NOT appear on that page -----------------------------------
{
  const e = env();
  const mine = "d".repeat(64);
  const theirs = "e".repeat(64);
  const hidden = "f".repeat(64);
  e._kv.set(`app:${mine}`, JSON.stringify({ name: "Mine", author_login: "alice", published: 2 }));
  e._kv.set(`app:${theirs}`, JSON.stringify({ name: "Theirs", author_login: "bob", published: 3 }));
  e._kv.set(
    `app:${hidden}`,
    JSON.stringify({ name: "Hidden", author_login: "alice", published: 4, unlisted: true }),
  );

  const page = await hit(e, "https://alice.krate.tech/", BROWSER);
  assert.strictEqual(page.status, 200);
  assert.match(page.body, /Mine/, "her own app is there");
  assert.ok(!page.body.includes("Theirs"), "somebody else's is not");
  assert.ok(
    !page.body.includes("Hidden"),
    "and an unlisted one is not -- it is deliberately not the public face",
  );
  console.log("ok  a page shows this author's listed apps and nobody else's");
}

console.log("ok  a developer's own name works end to end");
