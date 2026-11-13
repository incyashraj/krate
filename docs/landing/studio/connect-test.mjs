// An API the company already has, made a company connection (SMART-FIXES
// section 4): docs/landing/kit/connect.js as it ships.
//
// An OpenAPI 3 description (YAML) and a Swagger 2 one (JSON) become the
// connection form's address, methods, path prefixes and credential kind;
// a description on plain http is refused; apiKey, bearer, basic and OAuth
// schemes ask for the right credential; path prefixes are the shortest that
// cover the description's operations and never reach past the address;
// the API's summary stays under the hub's 24 KB cap with the allowed
// operations first; what Studio's AI reads lists only what the connection
// allows; and every preset is complete.
//
// The two real descriptions are the Swagger Petstore's, saved whole as
// fixtures: connect-petstore3-fixture.yaml (swagger-api/swagger-petstore,
// src/main/resources/openapi.yaml) and connect-petstore2-fixture.json
// (petstore.swagger.io/v2/swagger.json), both Apache 2.0.
//
//   node docs/landing/studio/connect-test.mjs
import { readFileSync } from "node:fs";
import "../kit/connect.js";

const C = globalThis.KrateConnect;
let bad = 0;
function check(ok, name) {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const bytes = (v) => new TextEncoder().encode(JSON.stringify(v)).length;
const op = (method, path, extra = {}) => ({ method, path, id: "", summary: "", query: [], body: false, ...extra });
const spec3 = (extra = {}) => ({ openapi: "3.0.3", info: { title: "Acme Finance", version: "2.1" }, servers: [{ url: "https://api.acme.example/finance" }], paths: { "/invoices": { get: { operationId: "listInvoices" } } }, ...extra });

/* ---- OpenAPI 3, as YAML: the Petstore's own description ------------------ */
{
  const text = readFileSync("docs/landing/studio/connect-petstore3-fixture.yaml", "utf8");
  const read = C.parse(text);
  check(read.spec && read.spec.openapi === "3.0.4" && Object.keys(read.spec.paths).length === 13, "the Petstore's YAML reads: openapi 3.0.4, 13 paths");
  check(read.spec && same(read.spec.components.schemas.Pet.required, ["name", "photoUrls"]) && read.spec.components.schemas.Pet.properties.id.example === 10, "with its schemas, lists and numbers intact");
  check(read.spec && /^This is a sample Pet Store Server based on the OpenAPI 3\.0 specification\. {2}You can find out more about\nSwagger/.test(read.spec.info.description), "and its |- block text, line breaks kept");
  const m = C.fromSpec(read.spec, { url: "https://petstore3.swagger.io/api/v3/openapi.yaml" });
  check(m.base === "https://petstore3.swagger.io/api/v3", "the address is servers[0]: " + m.base);
  check(same(m.methods, ["GET", "POST", "PUT", "DELETE"]) && same(m.ticked, ["GET"]), "every method it uses is offered, GET alone ticked");
  check(same(m.paths, ["/pet", "/store/inventory", "/store/order", "/user"]), "the GET paths, as the shortest prefixes that cover them: " + m.paths.join(" "));
  check(!m.paths.includes("/store") && !m.paths.includes("/"), "siblings are not merged into a wider prefix");
  check(same(m.auth, { kind: "header", header: "api_key" }), "its api_key header is asked for before its OAuth sign-in, whose tokens run out");
  check(m.ops.length === 19 && m.api.operations.length === 19 && m.api.more === 0, "all 19 operations are in the summary");
  const find = m.api.operations.find((o) => o.id === "findPetsByStatus");
  check(find && find.method === "GET" && find.path === "/pet/findByStatus" && same(find.query, ["status"]) && find.summary === "Finds Pets by status.", "each with its method, path, id, summary and query parameters");
  check(m.api.operations.find((o) => o.id === "addPet").body === true, "and whether it takes a body");
  check(m.api.operations.slice(0, 8).every((o) => o.method === "GET"), "the operations it allows come first");
  check(m.api.source === "https://petstore3.swagger.io/api/v3/openapi.yaml" && m.api.title === "Swagger Petstore - OpenAPI 3.0", "it says where it was read from");
  check(m.notes.some((n) => /also has POST, PUT, DELETE/.test(n)), "and says what else the API can do, to tick only if needed");
  const more = C.pathsFor(m.ops, ["GET", "POST"]);
  check(same(more.paths, ["/pet", "/store/inventory", "/store/order", "/user"]) && !more.widened, "ticking POST adds its operations' paths, still exact");
}

/* ---- Swagger 2, as JSON ------------------------------------------------- */
{
  const read = C.parse(readFileSync("docs/landing/studio/connect-petstore2-fixture.json", "utf8"));
  const m = C.fromSpec(read.spec, { url: "https://petstore.swagger.io/v2/swagger.json" });
  check(m.base === "https://petstore.swagger.io/v2", "Swagger 2: the address from schemes (https chosen), host and basePath: " + m.base);
  check(same(m.auth, { kind: "header", header: "api_key" }), "its securityDefinitions' api_key header");
  check(m.ops.length === 20 && same(m.paths, ["/pet", "/store/inventory", "/store/order", "/user"]), "its 20 operations and their GET prefixes");
  const pasted = C.fromSpec(read.spec, {});
  check(pasted.base === "https://petstore.swagger.io/v2" && pasted.api.source === "pasted", "pasted, it still knows its host");
  const http = C.fromSpec({ ...read.spec, schemes: ["http"] }, {});
  check(http.problem && http.problem[0] === "This API is not on https", "a Swagger 2 API on http only is refused");
  const noHost = C.fromSpec({ ...read.spec, host: undefined }, { url: "https://docs.acme.example/swagger.json" });
  check(noHost.base === "https://docs.acme.example/v2", "with no host, it is the site it was read from");
}

/* ---- the address ------------------------------------------------------- */
{
  const http = C.fromSpec(spec3({ servers: [{ url: "http://api.acme.example/finance" }] }));
  check(http.problem && /not on https/.test(http.problem[0]) && /http:\/\/api\.acme\.example\/finance/.test(http.problem[1]), "a description whose servers are all http is refused, saying the address");
  const either = C.fromSpec(spec3({ servers: [{ url: "http://api.acme.example/v1" }, { url: "https://api.acme.example/v1" }] }));
  check(either.base === "https://api.acme.example/v1", "the first https server is used when there is one");
  const rel = C.fromSpec(spec3({ servers: [{ url: "/api/v3" }] }), { url: "https://petstore3.swagger.io/api/v3/openapi.json" });
  check(rel.base === "https://petstore3.swagger.io/api/v3", "a relative server is resolved against where it was read: " + rel.base);
  const relPasted = C.fromSpec(spec3({ servers: [{ url: "/api/v3" }] }));
  check(relPasted.base === null && relPasted.notes.some((n) => /\/api\/v3 without the site/.test(n)), "pasted, a relative server asks for the full address");
  const vars = C.fromSpec(spec3({ servers: [{ url: "https://{tenant}.acme.example/{version}", variables: { tenant: {}, version: { default: "v2" } } }] }));
  check(vars.base === "https://<tenant>.acme.example/v2" && same(vars.needs, ["tenant"]) && vars.notes.some((n) => /Replace <tenant>/.test(n)), "variables take their default; one without asks to be filled: " + vars.base);
  const noServers = C.fromSpec(spec3({ servers: undefined }), { url: "https://api.acme.example/openapi.json" });
  check(noServers.base === "https://api.acme.example", "no servers: the site it was read from");
  const tail = C.fromSpec(spec3({ servers: [{ url: "https://API.Acme.example/finance/?x=1" }] }));
  check(tail.base === "https://api.acme.example/finance", "the host lower-cased, and no trailing slash or query");
}

/* ---- the credential ------------------------------------------------------ */
{
  const auth = (schemes, security) => C.fromSpec(spec3({ components: { securitySchemes: schemes }, security }));
  check(same(auth({ k: { type: "apiKey", in: "header", name: "X-Api-Key" } }).auth, { kind: "header", header: "X-Api-Key" }), "apiKey in a header: that header");
  check(same(auth({ b: { type: "http", scheme: "bearer" } }).auth, { kind: "header", header: "Authorization", prefix: "Bearer " }), "http bearer: Authorization, sent as Bearer");
  check(same(auth({ b: { type: "http", scheme: "basic" } }).auth, { kind: "basic" }), "http basic: a user and password");
  const q = auth({ k: { type: "apiKey", in: "query", name: "key" } });
  check(q.auth === null && q.notes.some((n) => /\?key=\.\.\..*header only/.test(n)), "apiKey in the query: not asked for, and it says why");
  const o = auth({ o: { type: "oauth2", flows: {} } });
  check(same(o.auth, { kind: "header", header: "Authorization", prefix: "Bearer " }) && o.notes.some((n) => /does not refresh/.test(n)), "OAuth: a token sent as Bearer, saying Krate does not refresh it");
  const used = auth({ a: { type: "http", scheme: "basic" }, b: { type: "apiKey", in: "header", name: "X-Key" } }, [{ b: [] }]);
  check(same(used.auth, { kind: "header", header: "X-Key" }), "the scheme the API asks for first wins");
  check(auth({}).auth === null, "none named: the admin chooses");
  const sw = C.fromSpec({ swagger: "2.0", host: "api.acme.example", basePath: "/v1", schemes: ["https"], securityDefinitions: { b: { type: "basic" } }, paths: { "/a": { get: {} } } });
  check(same(sw.auth, { kind: "basic" }) && sw.base === "https://api.acme.example/v1", "Swagger 2 basic");
}

/* ---- path prefixes ----------------------------------------------------- */
{
  const ops = [op("GET", "/pets"), op("GET", "/pets/{id}"), op("GET", "/pets/{id}/photos"), op("GET", "/store/inventory"), op("GET", "/store/order/{id}"), op("POST", "/admin/reset"), op("GET", "/")];
  const p = C.pathsFor(ops, ["GET"]);
  check(same(p.paths, ["/pets", "/store/inventory", "/store/order"]) && !p.widened, "each path's part before its first {parameter}, the ones under another dropped: " + p.paths.join(" "));
  check(!p.paths.includes("/admin/reset"), "only the ticked methods' paths");
  check(!p.paths.includes("/"), "GET / is not allowed as a prefix when other paths exist: it would allow everything");
  check(same(C.pathsFor([op("GET", "/{owner}"), op("GET", "/{owner}/repos")], ["GET"]).paths, ["/"]), "a description whose paths start with a {parameter} is the whole address");
  check(C.staticPrefix("/files/{id}:copy") === "/files" && C.staticPrefix("/v1/files:upload") === "/v1/files:upload", "whole segments only");
  check(same(C.pathsFor([op("GET", "/a b"), op("GET", "/ok")], ["GET"]).paths, ["/ok"]), "a path the hub cannot match exactly is left out");
  // Too many: cut back from the deepest, and every prefix still covers a
  // path the description names, under the address.
  const big = [];
  for (let i = 0; i < 40; i++) big.push(op("GET", `/v1/items${i}/{id}`));
  for (let i = 0; i < 30; i++) big.push(op("GET", `/v1/admin/things${i}`));
  const w = C.pathsFor(big, ["GET"]);
  check(w.widened && w.paths.length <= C.MAX_PATHS, `over ${C.MAX_PATHS} prefixes are cut back: ${w.paths.length}`);
  check(w.paths.includes("/v1/admin") && w.paths.includes("/v1/items0"), "the deepest first, so the shallow ones stay exact");
  check(w.paths.every((x) => big.some((o) => o.path === x || o.path.startsWith(x + "/"))), "and every prefix is a whole-segment prefix of a path it names, never past the address");
}

/* ---- the summary's cap ------------------------------------------------- */
{
  const paths = {};
  for (let i = 0; i < 1500; i++) paths[`/things${i}/{id}`] = { get: { operationId: `getThing${i}`, summary: "Gets one thing — with unicode ünïcödé ✓ ".repeat(3), parameters: [{ name: "expand", in: "query" }, { name: "X-Trace", in: "header" }] }, post: { operationId: `makeThing${i}` } };
  const m = C.fromSpec(spec3({ paths }));
  check(bytes(m.api) <= C.MAX_API && bytes(m.api) > C.MAX_API - 1024, `a big API's summary stays under ${C.MAX_API} bytes, counted in UTF-8: ${bytes(m.api)}`);
  check(m.api.operations.length + m.api.more === 3000 && m.api.more > 0, `the rest are counted, not listed: ${m.api.operations.length} + ${m.api.more}`);
  check(m.api.operations.every((o) => o.method === "GET"), "the allowed GET operations fill it first");
  check(m.api.operations[0].summary.length <= 140 && same(m.api.operations[0].query, ["expand", "header X-Trace"]), "summaries are clipped; header parameters are named as such");
  const withNotes = C.summary(spec3({ paths }), m.ops, null, { methods: ["GET"], paths: ["/"] }, "x".repeat(600));
  check(bytes(withNotes) <= C.MAX_API && withNotes.notes.length <= 600, "notes count inside the cap");
  const hub = readFileSync("cloud/worker/src/org-connections.js", "utf8");
  check(/export const MAX_API = 24 \* 1024;/.test(hub) && C.MAX_API === 24 * 1024, "the hub refuses past the same 24 KB");
}

/* ---- what Studio's AI reads --------------------------------------------- */
{
  const conn = {
    name: "finance-api", base: "https://api.acme.example/finance", allow: { methods: ["GET"], paths: ["/invoices"] },
    api: { title: "Acme Finance", version: "2.1", operations: [
      { method: "GET", path: "/invoices", id: "listInvoices", summary: "List invoices", query: ["status"] },
      { method: "GET", path: "/invoices/{id}", id: "getInvoice" },
      { method: "POST", path: "/invoices", id: "createInvoice", body: true },
      { method: "GET", path: "/rates", id: "listRates" },
      { method: "GET", path: "/invoices-old", id: "oldInvoices" },
    ], more: 2 },
  };
  const t = C.contextText(conn, "Acme");
  check(t.startsWith("# Company connection: finance-api (Acme)") && t.includes('cap = "net.connection:finance-api"'), "it names the connection and how to declare it");
  check(t.includes("https://api.acme.example/finance") && /WITHOUT any Authorization/.test(t), "its address, and that the app sends no credential");
  check(t.includes("`GET /invoices` (listInvoices) -- List invoices Query: status.") && t.includes("`GET /invoices/{id}`"), "the operations it allows");
  check(!/createInvoice|listRates|oldInvoices/.test(t), "never the ones it does not allow (whole segments: /invoices-old is not under /invoices)");
  check(/5 more operations are in the API but not allowed here or not listed: do not call them\./.test(t), "those are counted, with the ones that did not fit");
  const bare = C.contextText({ name: "jira", base: "https://acme.atlassian.net/rest/api/3", allow: { methods: ["GET"], paths: ["/"] } }, "Acme");
  check(/GET on every path under the address/.test(bare) && !/OpenAPI/.test(bare), "a connection with no description still says what it allows");
}

/* ---- reading what was pasted --------------------------------------------- */
{
  check(C.parse("").problem[0] === "Nothing to read", "nothing pasted says so");
  check(/web page/.test(C.parse("<!doctype html><html>").problem[0]), "a web page pasted by mistake says to open the description itself");
  check(/JSON did not read/.test(C.parse("{ \"openapi\": ").problem[0]), "broken JSON says to copy it again");
  const anchors = C.parse("openapi: 3.0.0\nbase: &b\n  a: 1\nother: *b\n");
  check(anchors.problem && /anchors/.test(anchors.problem[1]) && /JSON version/.test(anchors.problem[1]), "YAML anchors are refused, asking for the JSON version");
  check(/not an OpenAPI 3 or Swagger 2/.test(C.fromSpec({ openapi: "2.5" }).problem[0]), "another version is named");
  check(/no openapi: or swagger:/.test(C.fromSpec({ hello: 1 }).problem[1]), "something else says what to check");
  check(/no operations/.test(C.fromSpec(spec3({ paths: {} })).problem[0]), "a description with no operations says so");
  const y = C.parseYaml([
    "a: [1, 'two', {b: c}]",
    "s: \"x\\ty\"",
    "f: >-",
    "  one",
    "  two",
    "n: ~",
    "l:",
    "- - 1",
    "  - 2",
    "- k: v  # a comment",
    "  j: 'it''s'",
    "url: https://x.example/a#b",
  ].join("\n"));
  check(same(y, { a: [1, "two", { b: "c" }], s: "x\ty", f: "one two", n: null, l: [[1, 2], { k: "v", j: "it's" }], url: "https://x.example/a#b" }), "YAML flow lists and maps, quotes, folded text, null, nested lists, comments: " + JSON.stringify(y));
}

/* ---- presets ------------------------------------------------------------- */
{
  const ids = C.PRESETS.map((p) => p.id);
  check(same(ids, ["graph", "google", "salesforce", "servicenow", "jira", "slack", "hubspot", "stripe", "snowflake", "github", "zendesk", "n8n", "make", "rest"]), "the systems buyers name first, then webhooks and any REST API: " + ids.join(","));
  for (const p of C.PRESETS) {
    const ok = typeof p.label === "string" && p.label
      && (p.base === "" || /^https:\/\/[^\s]+[^/]$/.test(p.base))
      && p.auth && (p.auth.kind === "basic" || /^[A-Za-z0-9-]+$/.test(p.auth.header))
      && p.methods.every((m) => C.FORM_METHODS.includes(m)) && p.methods.length
      && p.paths.every((x) => /^\/[^\s?#%]*$/.test(x))
      && (p.id === "rest" || /^https:\/\//.test(p.docs))
      && (p.name === "" || /^[a-z][a-z0-9-]{1,38}$/.test(p.name))
      && (!p.ai || p.ai.length <= 600);
    check(ok, `${p.label}: an https address, a credential, methods the form offers, plain paths, and the vendor's page on making the credential`);
  }
  const ro = C.PRESETS.filter((p) => !["snowflake", "n8n", "make"].includes(p.id));
  check(ro.every((p) => same(p.methods, ["GET"])), "every API preset reads only, GET, except Snowflake whose queries are POST");
  check(C.PRESETS.find((p) => p.id === "github").ai.includes("X-GitHub-Api-Version: 2026-03-10") && /User-Agent/.test(C.PRESETS.find((p) => p.id === "github").ai), "GitHub's required headers go to the AI");
  check(C.PRESETS.filter((p) => p.base.includes("<")).every((p) => /<[a-z-]+>/.test(p.base)), "placeholders are <words> the save step asks to fill");
}

if (bad) {
  console.log(`\n${bad} failed`);
  process.exit(1);
}
console.log("\nall passed");
