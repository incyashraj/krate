// The real hub Worker, on this machine, for the Player proofs (IC-956).
//
// cloud/worker/src/index.js itself -- the publish door, /a/<hash>/web, the
// Player page and its headers, the organisation checks -- run in Node over
// in-memory KV and R2 and the real Org Durable Object (the worker tests'
// own stand-ins, cloud/worker/test/org-tools.mjs), with the Player's files
// from cloud/worker/player-dist as its ASSETS. Nothing reaches GitHub: a
// token gho_<login> stands for that GitHub user, as in the worker tests.
//
//   node --experimental-wasm-modules hub-local.mjs <port> <seed.json>
//
// <seed.json> lists bundles to publish ({ who, file }) and is answered on
// stdout with each one's hash once the hub is listening.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, normalize } from 'node:path';
import { orgEnv, call, acme, worker } from '../../../cloud/worker/test/org-tools.mjs';

const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..', '..');
const dist = join(repo, 'cloud', 'worker', 'player-dist');
const port = Number(process.argv[2] || 8911);
const seed = process.argv[3] ? JSON.parse(readFileSync(process.argv[3], 'utf8')) : { publish: [] };
const base = `http://127.0.0.1:${port}`;

const env = orgEnv({
  PUBLIC_BASE: base,
  SITE_BASE: 'https://krate.tech',
  ASSETS: {
    async fetch(request) {
      const name = normalize(new URL(request.url).pathname).replace(/^\/+/, '');
      if (name.includes('..')) return new Response('no', { status: 404 });
      try { return new Response(await readFile(join(dist, name))); } catch { return new Response('not found', { status: 404 }); }
    },
  },
});

const answers = {};
async function seedHub() {
  for (const p of seed.publish || []) {
    const bytes = new Uint8Array(readFileSync(p.file));
    const res = await call(env, p.who || 'alice', 'POST', '/publish', { bytes, headers: { 'x-krate-name': p.name || '' } });
    answers[p.label] = { status: res.status, id: res.body && res.body.id, body: res.status === 200 ? undefined : res.body };
  }
  // Bytes placed in the store directly, as a compromised store would hold
  // them: the door never saw them, so only the tab's own checks stand.
  for (const p of seed.inject || []) {
    const bytes = new Uint8Array(readFileSync(p.file));
    const sha = async (b) => [...new Uint8Array(await crypto.subtle.digest('SHA-256', b))].map((x) => x.toString(16).padStart(2, '0')).join('');
    const key = p.underFile ? await sha(new Uint8Array(readFileSync(p.underFile))) : await sha(bytes);
    await env.BUNDLES.put(key, bytes);
    answers[p.label] = { status: 200, id: key };
  }
  if (seed.org) {
    await acme(env);
    const up = await call(env, 'carol', 'POST', '/orgs/acme/candidates', { bytes: new Uint8Array(readFileSync(seed.org.file)) });
    const ok = await call(env, 'bob', 'POST', `/orgs/acme/candidates/${up.body.digest}/approve`);
    answers.org = { status: ok.status, id: up.body.digest };
  }
}

http.createServer(async (req, res) => {
  const chunks = [];
  // A request the browser abandons as the proof closes it ends "aborted";
  // uncaught, that ended the whole proof after every check had passed.
  try { for await (const c of req) chunks.push(c); } catch { return; }
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) headers.set(k, Array.isArray(v) ? v.join(', ') : v);
  const request = new Request(base + req.url, { method: req.method, headers, body: req.method === 'GET' || req.method === 'HEAD' ? undefined : body });
  let answer;
  try { answer = await worker.fetch(request, env, { waitUntil() {} }); } catch (e) { answer = new Response(String(e && e.stack), { status: 500 }); }
  const out = {};
  answer.headers.forEach((v, k) => { out[k] = v; });
  // No caching between proof runs: each cold start is cold.
  if (!/^\/player\//.test(req.url)) out['cache-control'] = out['cache-control'] || 'no-store';
  if (process.env.NO_CACHE) out['cache-control'] = 'no-store';
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
}).listen(port, '127.0.0.1', async () => {
  await seedHub();
  console.log(JSON.stringify({ base, answers }));
});
