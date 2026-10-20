// The gallery's open counts (openCounts in src/index.js) read Analytics
// Engine with the hub's own credentials and leave CI and build farms out.
// It used to read env names the hub never had, so every app showed 0 opens.
import assert from "node:assert/strict";
import test from "node:test";
import { openCounts } from "../src/index.js";

test("open counts use the hub's analytics credentials and count people, not CI", async () => {
  const seen = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    seen.push({ url: String(url), auth: init.headers && init.headers.Authorization, sql: String(init.body || "") });
    return new Response(JSON.stringify({ data: [{ app: "ab12cd", opens: "7" }] }));
  };
  try {
    const opens = await openCounts({ CF_ACCOUNT_ID: "acct", CF_ANALYTICS_TOKEN: "t" });
    assert.equal(opens.get("ab12cd"), 7);
    assert.equal(seen.length, 1);
    assert.match(seen[0].url, /accounts\/acct\/analytics_engine\/sql$/);
    assert.equal(seen[0].auth, "Bearer t");
    assert.match(seen[0].sql, /blob9 != 'cloud'/, "CI and build farms are left out");
    // Without credentials it answers empty and asks nothing.
    seen.length = 0;
    assert.equal((await openCounts({})).size, 0);
    assert.equal(seen.length, 0);
  } finally {
    globalThis.fetch = realFetch;
  }
});
