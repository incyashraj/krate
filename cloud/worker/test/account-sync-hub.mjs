/* The real hub Worker on this machine, for the two-device account sync
 * proof (crates/runtime/tests/account_sync_two_devices.rs, IC-981).
 *
 * cloud/worker/src/index.js itself over an in-memory KV and R2 (the R2
 * stand-in the other worker tests use) and the real SyncAccount object per
 * account over SQLite (sync-account-tools.mjs), signed in through the
 * GitHub stub (a token gho_<login> is that GitHub user). One thing is added
 * beside the worker, never inside it: GET /__kv answers what the hub keeps
 * -- every value the KV holds, every row each account's object holds
 * (under "acsync:<object name>" in `kv`), and every object R2 holds
 * (base64) -- so the proof can read exactly the bytes the hub keeps and
 * find no plaintext in them.
 *
 * `writes` is every write the hub made that is not a file body: KV puts
 * plus the object calls that wrote a row (a sync that changed something
 * is one). `kv_writes`, `object_commits` and `object_rows` part it.
 *
 *   node --experimental-wasm-modules account-sync-hub.mjs [port]
 *
 * Prints {"base": "http://127.0.0.1:<port>"} once it is listening.
 */
import http from "node:http";
import "./github-stub.mjs";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";
import { syncNamespace } from "./sync-account-tools.mjs";

const KV = new Map();
let writes = 0;
const R2 = r2Mock();
let r2Writes = 0;
const r2Put = R2.put;
R2.put = async (...args) => {
  r2Writes += 1;
  return r2Put(...args);
};
const env = {
  PUBLIC_BASE: "http://127.0.0.1",
  BUNDLES: R2,
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
const SYNC = syncNamespace(env);
env.SYNC_ACCOUNTS = SYNC;

const server = http.createServer(async (req, res) => {
  if (req.url === "/__kv") {
    res.writeHead(200, { "content-type": "application/json" });
    const r2 = {};
    for (const [k, v] of R2._blobs) if (!k.includes("\u0000")) r2[k] = Buffer.from(v).toString("base64");
    console.error(`MEASURE hub: ${writes} KV writes, ${SYNC.commits} object calls that wrote (${SYNC.rows} rows), ${r2Writes} R2 writes`);
    const kv = Object.fromEntries(KV);
    for (const [name, tables] of Object.entries(SYNC.dump())) kv[`acsync:${name}`] = tables;
    res.end(
      JSON.stringify({
        writes: writes + SYNC.commits,
        kv_writes: writes,
        object_commits: SYNC.commits,
        object_rows: SYNC.rows,
        kv,
        r2_writes: r2Writes,
        r2,
      }),
    );
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
