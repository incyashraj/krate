/* The real hub Worker on this machine, for the two-device account sync
 * proof (crates/runtime/tests/account_sync_two_devices.rs, IC-981).
 *
 * cloud/worker/src/index.js itself over an in-memory KV, signed in through
 * the GitHub stub (a token gho_<login> is that GitHub user). One thing is
 * added beside the worker, never inside it: GET /__kv answers every value
 * the KV holds, so the proof can read exactly the bytes the hub keeps and
 * find no plaintext in them.
 *
 *   node --experimental-wasm-modules account-sync-hub.mjs [port]
 *
 * Prints {"base": "http://127.0.0.1:<port>"} once it is listening.
 */
import http from "node:http";
import "./github-stub.mjs";
import worker from "../src/index.js";

const KV = new Map();
let writes = 0;
const env = {
  PUBLIC_BASE: "http://127.0.0.1",
  APPS: {
    __standIn: true,
    get: async (k) => KV.get(k) ?? null,
    put: async (k, v) => {
      writes += 1;
      KV.set(k, v);
    },
    delete: async (k) => {
      KV.delete(k);
    },
    list: async ({ prefix = "" } = {}) => ({ keys: [...KV.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })), list_complete: true }),
  },
};

const server = http.createServer(async (req, res) => {
  if (req.url === "/__kv") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ writes, kv: Object.fromEntries(KV) }));
    return;
  }
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
  const out = {};
  answer.headers.forEach((v, k) => {
    out[k] = v;
  });
  res.writeHead(answer.status, out);
  res.end(Buffer.from(await answer.arrayBuffer()));
});

server.listen(Number(process.argv[2] || 0), "127.0.0.1", () => {
  const { port } = server.address();
  console.log(JSON.stringify({ base: `http://127.0.0.1:${port}` }));
});
