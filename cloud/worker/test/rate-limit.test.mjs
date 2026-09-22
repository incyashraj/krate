/* The anonymous write endpoints have a ceiling, and ordinary use is under
 * it (K-767).
 *
 * There was no rate limiting anywhere in the worker. One comment mentioned
 * the concept and no code implemented it. Confirmed live against
 * hub.krate.tech: six rapid POST /share/new and five rapid anonymous POST
 * /report all returned 200, never 429. Each one makes a KV row or an R2
 * object, so every anonymous write endpoint was an unbounded Cloudflare
 * bill and an unbounded pile of junk, reachable by anyone.
 *
 * Two things have to be true at once and neither is interesting alone. A
 * flood must eventually be refused -- and an ordinary single request must
 * still go through, because a wall that stops normal use is worse than no
 * wall. `return text("no", 429)` at the top of the router would pass the
 * first assertion and fail the product.
 *
 * This drives the REAL worker rather than mirroring its logic, for the same
 * reason upload-bounds.test.mjs does: a mirror is written by whoever is sure
 * they know what the product does, and it would have passed during every
 * month the product had no wall at all.
 *
 *   node --experimental-wasm-modules cloud/worker/test/rate-limit.test.mjs
 */
import assert from "node:assert";
import worker from "../src/index.js";
import { r2Mock } from "./r2-mock.mjs";

/* The KV the worker counts in, with the two rows a signed-in session needs
 * so the support case can have an identity. `get` is what the limiter
 * reads and it is strongly consistent in the real thing; `list` is not,
 * which is why the limiter never lists and why this mock's list is empty. */
function env() {
  const kv = new Map([
    ["session:krs_test", "u1"],
    ["user:u1", JSON.stringify({ login: "alice", name: "Alice", avatar_url: "" })],
  ]);
  return {
    APPS: {
      get: async (k) => kv.get(k) ?? null,
      put: async (k, v) => { kv.set(k, v); },
      list: async () => ({ keys: [], list_complete: true }),
    },
    BUNDLES: r2Mock(),
    _kv: kv,
  };
}

/* One caller, as Cloudflare names them. The client cannot forge this --
 * Cloudflare overwrites whatever the caller sent -- so the test sets it
 * the way the edge would. */
const ALICE = { "cf-connecting-ip": "203.0.113.7" };
const BOB = { "cf-connecting-ip": "198.51.100.9" };

function reportZip() {
  return new Uint8Array([0x50, 0x4b, 0x03, 0x04, 0, 0, 0, 0]);
}

/* Each endpoint with the smallest body it accepts, so a refusal in these
 * tests is the wall and never a validation failure. The `limit` is the
 * ceiling the worker declares for that bucket. */
const ENDPOINTS = [
  {
    name: "/share/new",
    limit: 10,
    make: (who) => new Request("https://hub.example/share/new", { method: "POST", headers: { ...who } }),
  },
  {
    name: "/report",
    limit: 5,
    make: (who) => new Request("https://hub.example/report", {
      method: "POST",
      headers: { "content-type": "application/zip", ...who },
      body: reportZip(),
    }),
  },
  {
    name: "/founding",
    limit: 5,
    make: (who) => new Request("https://hub.example/founding", {
      method: "POST",
      headers: { "content-type": "application/json", ...who },
      body: JSON.stringify({ email: "someone@example.com" }),
    }),
  },
  {
    name: "/makeit",
    limit: 5,
    make: (who) => new Request("https://hub.example/makeit", {
      method: "POST",
      headers: { "content-type": "application/json", ...who },
      body: JSON.stringify({ email: "someone@example.com", request: "a tip calculator" }),
    }),
  },
  {
    name: "/usage",
    limit: 60,
    make: (who) => new Request("https://hub.example/usage", {
      method: "POST",
      headers: { "content-type": "application/json", ...who },
      body: JSON.stringify({ id: "abcdef01", action: "open", version: "0.5.0", os: "macos" }),
    }),
  },
  {
    name: "/support/new",
    limit: 5,
    make: (who) => new Request("https://hub.example/support/new", {
      method: "POST",
      headers: { "content-type": "application/json", ...who },
      body: JSON.stringify({ email: "someone@example.com", subject: "it broke", text: "here is what happened" }),
    }),
  },
];

async function run() {
  /* ---- 1. ORDINARY USE GOES THROUGH -----------------------------------
   *
   * This half comes first on purpose. A limiter that refuses the first
   * request satisfies every flood assertion below and destroys the
   * product, and that failure mode is the likelier one: the endpoints
   * differ in what they accept, and a ceiling set too low reads exactly
   * like a working wall until somebody tries to use the hub. */
  for (const ep of ENDPOINTS) {
    const res = await worker.fetch(ep.make(ALICE), env());
    assert.notStrictEqual(
      res.status, 429,
      `${ep.name} refused a single ordinary request -- a wall that stops normal use is worse than none`,
    );
    assert.ok(
      res.status < 400,
      `${ep.name} did not accept an ordinary request: ${res.status} ${await res.text()}`,
    );
  }

  /* ---- 2. A FLOOD IS EVENTUALLY REFUSED --------------------------------
   *
   * One env, so one KV, so the count accumulates the way it does for one
   * caller against the live hub. The loop runs a little past the ceiling
   * and the assertion is that a 429 appears -- not exactly where, because
   * where is the ceiling's business and the ceiling may be retuned. */
  for (const ep of ENDPOINTS) {
    const e = env();
    const seen = [];
    for (let i = 0; i < ep.limit + 3; i++) {
      const res = await worker.fetch(ep.make(ALICE), e);
      seen.push(res.status);
    }
    assert.ok(
      seen.includes(429),
      `${ep.name} never refused a flood of ${seen.length}: got ${seen.join(",")} -- ` +
        `this is the live finding, six rapid /share/new all returning 200`,
    );
    // And the first ones through must have been accepted, or the wall is
    // just a broken endpoint. A limit of N means at least N succeed.
    const ok = seen.filter((s) => s < 400).length;
    assert.ok(
      ok >= ep.limit,
      `${ep.name} refused before its own ceiling: only ${ok} of ${ep.limit} got through (${seen.join(",")})`,
    );
  }

  /* ---- 3. THE REFUSAL SAYS HOW LONG TO WAIT ----------------------------
   *
   * A 429 with no retry-after leaves a client guessing, and a client that
   * guesses wrong retries immediately -- which is the flood again. */
  {
    const e = env();
    let refusal = null;
    for (let i = 0; i < 20 && !refusal; i++) {
      const res = await worker.fetch(ENDPOINTS[1].make(ALICE), e);
      if (res.status === 429) refusal = res;
    }
    assert.ok(refusal, "a flood of /report must produce a refusal to inspect");
    const wait = Number(refusal.headers.get("retry-after"));
    assert.ok(
      Number.isInteger(wait) && wait > 0 && wait <= 60,
      `retry-after must be a whole number of seconds within the window, got ${refusal.headers.get("retry-after")}`,
    );
    assert.strictEqual(
      refusal.headers.get("access-control-expose-headers"),
      "retry-after",
      "a page script cannot read retry-after unless CORS exposes it, and every one of these is called from a page",
    );
  }

  /* ---- 4. ONE CALLER'S FLOOD DOES NOT REFUSE ANOTHER PERSON ------------
   *
   * The ceiling is per caller. If it were global, one machine hammering
   * /usage would silence the whole hub for everybody -- which converts a
   * cost problem into an outage anyone can cause. */
  {
    const e = env();
    for (let i = 0; i < 12; i++) await worker.fetch(ENDPOINTS[0].make(ALICE), e);
    const mine = await worker.fetch(ENDPOINTS[0].make(ALICE), e);
    assert.strictEqual(mine.status, 429, "the flooding caller is over their ceiling by now");
    const theirs = await worker.fetch(ENDPOINTS[0].make(BOB), e);
    assert.notStrictEqual(
      theirs.status, 429,
      "a second caller must not be refused because someone else flooded -- that is an outage, not a limit",
    );
  }

  /* ---- 5. ONE ENDPOINT'S CEILING IS NOT ANOTHER'S ----------------------
   *
   * Counted per bucket. Sending a bug report must not use up the shared
   * store you were about to make.
   *
   * Which endpoint floods and which is then checked is not arbitrary, and
   * getting it backwards makes this case unable to fail. Two cuts of it
   * survived the sabotage that replaces the per-bucket key with one fixed
   * key:
   *
   *   Flooding /report (ceiling 5) and checking /share/new (ceiling 10)
   *   cannot work at all. A refused request does not increment -- the
   *   limiter returns before the write, which is correct -- so a shared
   *   counter freezes at 5 no matter how long the flood runs, and 5 is
   *   under /share/new's 10. The lower ceiling caps the shared counter
   *   below the higher one forever.
   *
   * So the flood goes at the HIGHER ceiling and the check at the LOWER
   * one: /share/new floods to 10, and a shared counter would then be past
   * /report's 5 and refuse it. Sabotage confirms this direction bites. */
  {
    const e = env();
    const loose = ENDPOINTS[0]; // /share/new, the higher ceiling
    const tight = ENDPOINTS[1]; // /report, the lower one
    assert.ok(loose.limit > tight.limit, "this case needs the flooded endpoint to have the higher ceiling");
    for (let i = 0; i < loose.limit + 4; i++) await worker.fetch(loose.make(ALICE), e);
    const flooded = await worker.fetch(loose.make(ALICE), e);
    assert.strictEqual(flooded.status, 429, `${loose.name} is over its ceiling by now`);
    const other = await worker.fetch(tight.make(ALICE), e);
    assert.notStrictEqual(
      other.status, 429,
      `${tight.name} must have its own count -- flooding ${loose.name} must not close it`,
    );
  }

  /* ---- 6. A KV FAILURE DOES NOT CLOSE THE HUB --------------------------
   *
   * The thing being protected is a bill, not a secret. A KV blip that
   * turned every anonymous write into a 429 would be a worse outage than
   * the problem -- the K-082 shape, where a counter takes down the product
   * it guards. */
  {
    const e = env();
    // Only the limiter's own rows fail. Breaking the whole of KV would
    // break the handlers too and prove nothing about the limiter -- the
    // first cut of this did exactly that and reported a 500 from
    // shareNew's own put as if the wall had caused it.
    const get = e.APPS.get, put = e.APPS.put;
    let touched = 0;
    e.APPS.get = async (k) => {
      if (k.startsWith("rate:")) { touched++; throw new Error("KV get() limit exceeded"); }
      return get(k);
    };
    e.APPS.put = async (k, v, o) => {
      if (k.startsWith("rate:")) { touched++; throw new Error("KV put() limit exceeded"); }
      return put(k, v, o);
    };
    const res = await worker.fetch(ENDPOINTS[0].make(ALICE), e);
    assert.ok(touched > 0, "the limiter never touched KV, so this case tested nothing");
    assert.notStrictEqual(
      res.status, 429,
      "a KV failure must let the request through, not refuse it",
    );
    assert.notStrictEqual(res.status, 500, `a KV failure must not surface as an error: ${await res.text()}`);
  }

  /* ---- 7. AN OVERSIZE REPORT IS REFUSED WITHOUT BEING READ -------------
   *
   * `putReport` did `new Uint8Array(await request.arrayBuffer())` and only
   * THEN checked the 12 MiB cap, so a client announcing 500 MB was
   * allocated 500 MB before being told the ceiling. /publish already did
   * this correctly and the pattern was in the same file.
   *
   * The probe is the one upload-bounds.test.mjs arrived at the hard way: a
   * stream that throws when pulled proves nothing, because Node's Request
   * drains the body a tick after construction whether or not anyone reads
   * it. What only the HANDLER can do is call arrayBuffer() or take the
   * body stream, so both are counted on a real Request with everything
   * else left intact. */
  const watched = (headers) => {
    const req = new Request("https://hub.example/report", {
      method: "POST",
      headers,
      body: reportZip(),
    });
    let read = false;
    const real = req.arrayBuffer.bind(req);
    req.arrayBuffer = () => { read = true; return real(); };
    const bodyGetter = Object.getOwnPropertyDescriptor(Request.prototype, "body").get;
    Object.defineProperty(req, "body", {
      get() { read = true; return bodyGetter.call(this); },
    });
    return { req, wasRead: () => read };
  };
  const ZIP = { "content-type": "application/zip", ...ALICE };
  const MAX_REPORT = 12 * 1024 * 1024;

  {
    const { req, wasRead } = watched({ ...ZIP, "content-length": String(500 * 1024 * 1024) });
    const res = await worker.fetch(req, env());
    // Order matters here: assert on the READ first, because that is the
    // defect. Asserting the status first reports "expected 413, got 200,
    // {ok:true}" for a worker that materialised 500 MB and then accepted
    // it, which names the symptom and hides the cause.
    assert.ok(
      !wasRead(),
      "the report body was read before the declared length was judged -- that is the 500 MB " +
        "allocation this refuses, and it is exactly what putReport did before K-767",
    );
    assert.strictEqual(res.status, 413, `a 500 MB declaration must be refused: ${await res.text()}`);
  }
  {
    const { req, wasRead } = watched({ ...ZIP, "content-length": String(MAX_REPORT + 1) });
    const res = await worker.fetch(req, env());
    assert.ok(!wasRead(), "one byte over the limit must also refuse before reading");
    assert.strictEqual(res.status, 413, `one byte over is still over: ${await res.text()}`);
  }
  {
    // Exactly at the ceiling is not too large. An off-by-one here would
    // reject the largest legal report, and a report is only sent by
    // someone already having a bad day.
    const { req } = watched({ ...ZIP, "content-length": String(MAX_REPORT) });
    const res = await worker.fetch(req, env());
    assert.notStrictEqual(res.status, 413, "a report exactly at the ceiling is not too large");
  }
  {
    // The header is a claim, not a fact. An undercount must still meet the
    // check on the bytes that actually arrived, or the fast path becomes
    // the only path and lying past it is free.
    const big = new Uint8Array(MAX_REPORT + 1024);
    big[0] = 0x50; big[1] = 0x4b;
    const res = await worker.fetch(
      new Request("https://hub.example/report", {
        method: "POST",
        headers: { ...ZIP, "content-length": "10" },
        body: big,
      }),
      env(),
    );
    assert.strictEqual(res.status, 413, `a lying header must not buy a bigger body: ${res.status}`);
  }
  {
    // No header at all: a chunked sender. Must not be refused for that,
    // and the real length still applies to it.
    const big = new Uint8Array(MAX_REPORT + 1024);
    big[0] = 0x50; big[1] = 0x4b;
    const res = await worker.fetch(
      new Request("https://hub.example/report", { method: "POST", headers: ZIP, body: big }),
      env(),
    );
    assert.strictEqual(res.status, 413, "an oversized chunked report is still refused by the real length");
  }
  {
    const { req } = watched({ ...ZIP, "content-length": "banana" });
    const res = await worker.fetch(req, env());
    assert.strictEqual(res.status, 400, `a content-length that is not a number is refused: ${res.status}`);
  }

  /* ---- 8. AND THE REPORT STILL WORKS ----------------------------------
   *
   * Everything above is a refusal. K-757 made this endpoint anonymous
   * because the person whose product just broke was being asked to log in
   * first, and none of this may quietly undo that. */
  {
    const e = env();
    const res = await worker.fetch(
      new Request("https://hub.example/report", {
        method: "POST",
        headers: { ...ZIP, "x-krate-version": "0.5.0", "x-krate-os": "macos" },
        body: reportZip(),
      }),
      e,
    );
    assert.strictEqual(res.status, 200, `an ordinary anonymous report must still go through: ${res.status}`);
    const body = await res.json();
    const meta = JSON.parse(e._kv.get(`report:${body.id}`));
    assert.strictEqual(meta.from, "anonymous", "still anonymous, still stored");
    assert.strictEqual(meta.krate, "0.5.0", "and the version still survives the bounded read");
    assert.strictEqual(meta.size, 8, "the bytes that arrived are the bytes that were stored");
  }

  console.log(
    "ok -- every anonymous write endpoint refuses a flood with 429 and retry-after, ordinary single " +
      "requests still go through, the ceiling is per caller and per endpoint, a KV failure opens rather " +
      "than closes, and an oversize report is refused before its body is read",
  );
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
