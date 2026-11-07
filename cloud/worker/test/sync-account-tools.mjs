/* The account sync tests' stand-in for SYNC_ACCOUNTS (IC-981): the REAL
 * SyncAccount class over a real SQLite database (org-tools.mjs
 * sqlStorage), one object per name, reached through fetch the way
 * Cloudflare reaches it.
 *
 * It counts what the objects write as Cloudflare bills it: `rows` is every
 * row inserted, updated or deleted (SQLite's total_changes), and `commits`
 * the calls that wrote at least one row -- one per sync that changed
 * something. `dump()` answers every row every object holds, so a test can
 * read exactly what the hub keeps.
 */
import { sqlStorage } from "./org-tools.mjs";
import { SyncAccount } from "../src/account-sync.js";

const TABLES = ["account_key", "devices", "envelopes", "recovery", "buckets", "versions", "bodies"];

export function syncNamespace(env) {
  const objects = new Map();
  const ns = {
    requests: 0,
    rows: 0,
    commits: 0,
    objects,
    idFromName: (name) => name,
    get(id) {
      if (!objects.has(id)) {
        const storage = sqlStorage();
        objects.set(id, { storage, object: new SyncAccount({ storage }, env) });
      }
      const { storage, object } = objects.get(id);
      const changes = () => storage.db.prepare("SELECT total_changes() AS n").get().n;
      return {
        fetch: async (url, init) => {
          ns.requests += 1;
          const before = changes();
          const res = await object.fetch(new Request(url, init));
          const wrote = changes() - before;
          ns.rows += wrote;
          if (wrote > 0) ns.commits += 1;
          return res;
        },
      };
    },
    /* Every row of every object, by object name and table. */
    dump() {
      const out = {};
      for (const [name, { storage }] of objects) {
        out[name] = {};
        for (const t of TABLES) out[name][t] = storage.db.prepare(`SELECT * FROM ${t}`).all().map((r) => ({ ...r }));
      }
      return out;
    },
  };
  return ns;
}
