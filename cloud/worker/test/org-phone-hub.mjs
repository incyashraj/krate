/* The real hub Worker on this machine with one organisation in it, for the
 * phone's team-app proof (crates/player-android/tests/org_team_app.rs,
 * K-1171).
 *
 * cloud/worker/src/index.js itself and the real Org Durable Object over
 * SQLite (org-tools.mjs), with acme made as the organisation tests make it:
 * alice the owner, bob a reviewer, carol a developer, dan a recipient, and
 * `stranger` in none of it. A starter app as the site serves it
 * (docs/landing/orgs/starters/<file>) is marked and signed for acme under a
 * root key made here, uploaded by carol and approved by bob -- the release a
 * member's link opens. A token gho_<login> is that GitHub user.
 *
 *   node --experimental-wasm-modules org-phone-hub.mjs <starter file> <out.krate>
 *
 * Writes the released bundle to <out.krate> and prints one JSON line,
 * {"base", "digest", "app_id"}, once it is listening.
 */
import http from "node:http";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, acme, call, worker } from "./org-tools.mjs";
import { rootKey, sign } from "./org-bundle-tools.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const [starter, out] = process.argv.slice(2);
if (!starter || !out) {
  console.error("usage: org-phone-hub.mjs <starter file> <out.krate>");
  process.exit(2);
}

const env = orgEnv();
const key = rootKey();
await acme(env, { root: key.public, revocations: null });
const shipped = new Uint8Array(readFileSync(join(here, "..", "..", "..", "docs", "landing", "orgs", "starters", starter)));
const bytes = sign(shipped, key);
const up = await call(env, "carol", "POST", "/orgs/acme/candidates", { bytes });
if (up.status !== 201) throw new Error(`upload: ${up.status} ${JSON.stringify(up.body)}`);
const ok = await call(env, "bob", "POST", `/orgs/acme/candidates/${up.body.digest}/approve`);
if (ok.status !== 200) throw new Error(`approve: ${ok.status} ${JSON.stringify(ok.body)}`);
writeFileSync(out, bytes);

const server = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(", ") : v);
  const { port } = server.address();
  const request = new Request(`http://127.0.0.1:${port}${req.url}`, {
    method: req.method,
    headers,
    body: req.method === "GET" || req.method === "HEAD" ? undefined : body,
  });
  let answer;
  try {
    answer = await worker.fetch(request, env, { waitUntil() {} });
  } catch (e) {
    answer = new Response(String(e && e.stack), { status: 500 });
  }
  const sent = {};
  answer.headers.forEach((v, k) => {
    sent[k] = v;
  });
  res.writeHead(answer.status, sent);
  res.end(Buffer.from(await answer.arrayBuffer()));
});

server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  env.PUBLIC_BASE = `http://127.0.0.1:${port}`;
  console.log(JSON.stringify({ base: env.PUBLIC_BASE, digest: up.body.digest, app_id: up.body.app_id || null }));
});
