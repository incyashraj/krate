// krate.tech sends a signed-in visitor straight to Studio -- but only one
// arriving from elsewhere, only on the site's own token, and never where
// they asked for the page itself. The head script is lifted out of the
// shipped page and run, not re-typed.
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const page = readFileSync("docs/landing/index.html", "utf8");
const at = page.indexOf("/* Signed in, and arriving from anywhere but this site");
assert.ok(at > 0, "the redirect is in the page's head");
const start = page.indexOf("(function () {", at);
const end = page.indexOf("})();", start) + "})();".length;
const code = page.slice(start, end);
assert.ok(start < page.indexOf("</head>"), "and it runs in the head, before the page draws");

function visit({ token = null, search = "", hash = "", referrer = "" } = {}) {
  let went = null;
  const store = new Map(token ? [["krate_tok", token]] : []);
  const env = {
    document: { documentElement: { classList: { add() {} } }, referrer },
    localStorage: { getItem: (k) => (store.has(k) ? store.get(k) : null) },
    location: { search, hash, origin: "https://krate.tech", replace: (u) => { went = u; } },
  };
  new Function("document", "localStorage", "location", code)(env.document, env.localStorage, env.location);
  return went;
}

assert.equal(visit({ token: "krs_x" }), "/app/", "signed in, typed krate.tech: Studio");
assert.equal(visit({ token: "krs_x", referrer: "https://www.google.com/" }), "/app/", "signed in, from a search: Studio");
assert.equal(visit({}), null, "signed out: the page");
assert.equal(visit({ token: "krs_x", referrer: "https://krate.tech/docs/" }), null, "from within the site: the page");
assert.equal(visit({ token: "krs_x", search: "?home" }), null, "?home: the page");
assert.equal(visit({ token: "krs_x", hash: "#downloads" }), null, "a section link: the page");
assert.doesNotMatch(code, /location\.(href|assign)\s*=|URLSearchParams/, "the address cannot choose where anyone goes");
console.log("ok  signed-in visitors land in Studio; the page stays one click away");
