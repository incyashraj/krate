// The pages a stranger meets first, run from their shipped source.
//
// Each check lifts the real script out of the real page and runs it
// against a few stub elements, so a change to the page is what is tested,
// not a copy of it.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

/* A document with just enough of an element for these scripts: they only
 * ever set text, hidden, href, src and alt, and read them back. */
function stubDocument(ids) {
  const els = {};
  for (const id of ids) els[id] = { id, textContent: "", hidden: true, href: "", src: "", alt: "" };
  return {
    els,
    getElementById: (id) => els[id] || null,
  };
}

/* The script block that starts at `marker`, from its `(function () {` to
 * the `})();` that closes it. */
function lift(src, marker) {
  const at = src.indexOf(marker);
  assert.ok(at > 0, `the page still has ${JSON.stringify(marker)}`);
  const start = src.lastIndexOf("(function () {", at);
  const end = src.indexOf("\n})();", at);
  assert.ok(start > 0 && end > at, `the block around ${JSON.stringify(marker)} is whole`);
  return src.slice(start, end + "\n})();".length);
}

const settle = () => new Promise((r) => setTimeout(r, 0));

/* ---- K-858: a shared link opened in a desktop browser ----------------
 *
 * The hub sends a desktop browser to /open/?a=<id> instead of the file.
 * That page named the app and its size and offered only the Krate
 * download, so the one thing the person was sent could not be got. */
{
  const page = readFileSync("docs/open/index.html", "utf8");
  const code = lift(page, "var hash;");
  const HASH = "7edf315d97eca7db3142c9976cb19a465673c7f698e5ff13732cf64fb9196036";

  async function open(ua) {
    const doc = stubDocument([
      "taName", "taSize", "taShot", "theirApp", "pageHead", "galleryCard",
      "taRow", "taGet", "openTitle", "openBody",
    ]);
    const fetched = [];
    const fetch = async (url) => {
      fetched.push(url);
      return {
        ok: true,
        json: async () => ({ meta: { name: "Weather", size: 848626 }, shot: null }),
      };
    };
    new Function("document", "fetch", "location", "navigator", "URLSearchParams", code)(
      doc, fetch, { search: `?a=${HASH}` }, { userAgent: ua }, URLSearchParams,
    );
    await settle(); await settle(); await settle();
    return { doc: doc.els, fetched };
  }

  const mac = await open("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Chrome/120");
  assert.equal(mac.doc.theirApp.hidden, false, "the app's card shows");
  assert.equal(mac.doc.taRow.hidden, false, "a desktop browser is offered the app itself");
  assert.equal(mac.doc.taGet.href, `https://hub.krate.tech/a/${HASH}?dl=1`,
    "the link asks the hub for the bytes, so it cannot bounce back to this page");
  assert.match(mac.doc.taGet.textContent, /Weather/, "the button names the app");
  assert.match(mac.doc.openTitle.textContent, /Weather/, "the open step names the app too");

  const phone = await open("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)");
  assert.equal(phone.doc.theirApp.hidden, false, "a phone still sees what it was sent");
  assert.equal(phone.doc.taRow.hidden, true, "a phone is not handed a file it cannot open");
  console.log("ok  a shared link on a desktop browser offers the app itself (K-858)");
}

/* ---- K-865: the request that made an app is not its description ------
 *
 * Apps published before the publish sheet learned to describe them carry
 * the person's first request: "make a weather app". The hub's developer
 * pages and the gallery's app page each keep a copy of one rule; both are
 * run here on the live gallery's own sentences, so they cannot drift. */
{
  function liftRule(file) {
    const src = readFileSync(file, "utf8");
    const at = src.indexOf("function plainDescription(");
    assert.ok(at >= 0, `${file} has plainDescription`);
    const ret = src.indexOf("return s;", at);
    const end = src.indexOf("}", ret);
    return new Function(`${src.slice(at, end + 1)}; return plainDescription;`)();
  }
  const rules = {
    hub: liftRule("cloud/worker/src/index.js"),
    gallery: liftRule("docs/cloud/app/index.html"),
  };
  // Requests, from the live gallery (2026-09-28) and the usual shapes.
  const requests = [
    "make a weather app",
    "make a calculator app for me for simple calculations",
    "make a app which plays like a chess ame",
    "Make a NES game like contra 2d with each and everything just like original game",
    "can you make any app?",
    "Please build me a timer",
    "I want a todo list",
  ];
  // Descriptions, from the same gallery.
  const descriptions = [
    "A music player made with Krate",
    "What a 2.4 second request does to a window, blocking versus polled.",
    "A living northern-lights scene over a mirrored lake, live Kp index in the corner",
    "100,000 rows, only the visible window drawn",
    "Shows the weather where you are",
  ];
  for (const [where, rule] of Object.entries(rules)) {
    for (const r of requests) assert.equal(rule(r), "", `${where} hides the request ${JSON.stringify(r)}`);
    for (const d of descriptions) assert.equal(rule(d), d, `${where} keeps ${JSON.stringify(d)}`);
    assert.equal(rule(""), "");
    assert.equal(rule(undefined), "");
  }
  // And the gallery's app page uses it where it prints one.
  const page = readFileSync("docs/cloud/app/index.html", "utf8");
  assert.doesNotMatch(page, /esc\(m\.description\)/, "the app page never prints the raw description");
  console.log("ok  an app is not described by the request that made it (K-865)");
}
