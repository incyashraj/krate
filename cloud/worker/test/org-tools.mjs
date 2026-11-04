/* What the organisation tests share (IC-946): the REAL Org Durable Object
 * class over a real SQLite database (node:sqlite), reached through a
 * stand-in for its namespace the way Cloudflare reaches it -- one object
 * per name, through fetch -- and a worker env with KV and R2 mocks.
 *
 * The storage stand-in gives the object what a SQLite-backed Durable
 * Object has: `storage.sql.exec(query, ...bindings)` returning a cursor
 * with toArray(), and `storage.transactionSync(fn)`, which commits when
 * fn returns and rolls back when it throws.
 */
import { DatabaseSync } from "node:sqlite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import "./github-stub.mjs";
import worker, { Org } from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
export const FIXTURES = join(here, "fixtures", "org");
export const fixture = (name) => new Uint8Array(readFileSync(join(FIXTURES, name)));
export const KEYS = JSON.parse(readFileSync(join(FIXTURES, "keys.json"), "utf8"));
export const REVOCATIONS = JSON.parse(readFileSync(join(FIXTURES, "revocations.json"), "utf8"));

export function sqlStorage() {
  const db = new DatabaseSync(":memory:");
  const sql = {
    exec(query, ...bindings) {
      const statement = db.prepare(query);
      const reads = /^\s*(SELECT|WITH|PRAGMA)/i.test(query);
      const rows = reads ? statement.all(...bindings).map((r) => ({ ...r })) : (statement.run(...bindings), []);
      return { toArray: () => rows, one: () => rows[0] };
    },
  };
  let depth = 0;
  return {
    db,
    sql,
    // The object's one alarm, as milliseconds since the epoch; a test runs
    // it by calling the object's alarm() (IC-955's audit export).
    alarmAt: null,
    async getAlarm() {
      return this.alarmAt;
    },
    async setAlarm(at) {
      this.alarmAt = Number(at);
    },
    async deleteAlarm() {
      this.alarmAt = null;
    },
    transactionSync(fn) {
      if (depth > 0) return fn();
      depth += 1;
      db.exec("BEGIN");
      try {
        const out = fn();
        db.exec("COMMIT");
        return out;
      } catch (err) {
        db.exec("ROLLBACK");
        throw err;
      } finally {
        depth -= 1;
      }
    },
  };
}

/* One Org per name, created on first use, as idFromName + get would. */
export function orgNamespace(env) {
  const objects = new Map();
  const ns = {
    requests: 0,
    objects,
    idFromName: (name) => name,
    get(id) {
      if (!objects.has(id)) objects.set(id, new Org({ storage: sqlStorage() }, env));
      const object = objects.get(id);
      return {
        fetch: (url, init) => {
          ns.requests += 1;
          return object.fetch(new Request(url, init));
        },
      };
    },
    object: (slug) => objects.get(`org:${slug}`),
  };
  return ns;
}

/* The worker's env: KV and R2 mocks, and the organisation namespace. The
 * logins are Krate sessions, which verifyGitHub already accepts, so no
 * request goes to GitHub. */
export const PEOPLE = ["alice", "bob", "carol", "dan", "erin", "frank", "stranger"];
export function orgEnv(extra = {}) {
  const kv = new Map();
  PEOPLE.forEach((login, i) => {
    kv.set(`session:krs_${login}`, `u${i}`);
    kv.set(`user:u${i}`, JSON.stringify({ login, name: login, avatar_url: "" }));
  });
  const env = {
    APPS: {
      // This machine's stand-in, never KV itself (1823).
      __standIn: true,
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => {
        kv.set(k, v);
      },
      delete: async (k) => {
        kv.delete(k);
      },
      list: async ({ prefix = "" } = {}) => ({
        keys: [...kv.keys()].filter((k) => k.startsWith(prefix)).map((name) => ({ name })),
        list_complete: true,
      }),
    },
    BUNDLES: r2Mock(),
    PUBLIC_BASE: "https://hub.example",
    _kv: kv,
    ...extra,
  };
  env.ORGS = orgNamespace(env);
  return env;
}

/* The bearer token for `who`. A bare name is that GitHub user (a stubbed
 * GitHub token, test/github-stub.mjs): principal github:<who>. An address
 * is an email account that proved that inbox, signed in with a Krate
 * session, whose `login` is the part before the @ as a real one's is:
 * principal email:<who>. */
export function tokenFor(env, who) {
  if (!who.includes("@")) return `gho_${who}`;
  const id = `mail-${who.replace(/[^a-z0-9]/gi, "-")}`;
  env._kv.set(`session:krs_${id}`, id);
  env._kv.set(
    `user:${id}`,
    JSON.stringify({ id, login: who.split("@")[0], name: "", email: who, avatar_url: "", providers: ["email"], verified_emails: [who.toLowerCase()] }),
  );
  return `krs_${id}`;
}

/* One request to the worker as `who` (or nobody), answering
 * { status, body } with the body parsed when it is JSON. */
export async function call(env, who, method, path, { json: payload, bytes, key, headers = {} } = {}) {
  const h = { ...headers };
  if (who) h.authorization = `Bearer ${tokenFor(env, who)}`;
  if (key) h["idempotency-key"] = key;
  let body;
  if (payload !== undefined) {
    h["content-type"] = "application/json";
    body = JSON.stringify(payload);
  } else if (bytes) {
    body = bytes;
  }
  const res = await worker.fetch(new Request(`https://hub.example${path}`, { method, headers: h, body }), env);
  const raw = await res.arrayBuffer();
  const textBody = new TextDecoder().decode(raw);
  let parsed = null;
  try {
    parsed = textBody ? JSON.parse(textBody) : null;
  } catch (_) {
    parsed = textBody;
  }
  return { status: res.status, body: parsed, headers: res.headers, bytes: new Uint8Array(raw) };
}

/* acme with alice as owner, the root key registered, and bob, carol and
 * dan invited and joined as reviewer, developer and recipient. */
export async function acme(env, { root = KEYS.root, revocations = REVOCATIONS } = {}) {
  const ok = (r, what) => {
    if (r.status >= 300) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body)}`);
    return r;
  };
  ok(await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } }), "create");
  if (root) {
    ok(await call(env, "alice", "PUT", "/orgs/acme/root-key", { json: { public_key: root, revocations } }), "root key");
  }
  for (const [login, role] of [["bob", "reviewer"], ["carol", "developer"], ["dan", "recipient"]]) {
    const inv = ok(await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: login, role } }), `invite ${login}`);
    ok(await call(env, login, "POST", `/orgs/acme/invites/${inv.body.code}/accept`), `accept ${login}`);
  }
}

export { worker };
