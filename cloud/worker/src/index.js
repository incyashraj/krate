// The Krate hub.
//
// Same four routes the Rust hub served, with two additions that only matter
// once it is public: uploads are attributed to a real GitHub account, and the
// listing is what the cloud page renders.
//
//   GET  /health          is it up
//   POST /publish         store a .krate, return its URL
//   GET  /a/<hash>        fetch a .krate by content hash
//   GET  /apps            every published app, newest first
//
// Storage is split deliberately. Bundles are blobs in R2, which is cheap per
// byte and has no size ceiling worth worrying about. Metadata is in KV, which
// is fast to list and read -- the cloud page loads it on every visit, and
// pulling thirty-kilobyte bundles to render a list of names would be absurd.

/// A bundle over this is refused. Generous for a Krate app (the reference
/// apps are 20-40 KB) and small enough that a bad upload cannot cost real
/// money before anyone notices.
const MAX_UPLOAD_BYTES = 5 * 1024 * 1024;

/// How long a verified GitHub identity is trusted without asking GitHub
/// again. Long enough that publishing several apps costs one round trip,
/// short enough that a revoked token stops working the same day.
const IDENTITY_TTL_SECONDS = 6 * 60 * 60;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const { pathname } = url;

    // The website and the CLI both call this from other origins.
    if (request.method === "OPTIONS") {
      return cors(new Response(null, { status: 204 }));
    }

    try {
      if (request.method === "GET" && pathname === "/health") {
        return cors(text("ok"));
      }
      if (request.method === "POST" && pathname === "/publish") {
        return cors(await publish(request, env));
      }
      if (request.method === "GET" && pathname === "/login/start") {
        return loginStart(url, env);
      }
      if (request.method === "GET" && pathname === "/login/callback") {
        return loginCallback(url, env);
      }
      if (request.method === "GET" && pathname === "/login/google/start") {
        return googleStart(url, env);
      }
      if (request.method === "GET" && pathname === "/login/google/callback") {
        return googleCallback(url, env);
      }
      if (request.method === "POST" && pathname === "/login/email") {
        // Fetched from page scripts on krate.tech, unlike the login
        // redirects around it, so the answer must carry CORS headers or
        // the browser discards it and reports a phantom network failure.
        return cors(await emailStart(request, env));
      }
      if (request.method === "GET" && pathname === "/login/email/verify") {
        return emailVerify(url, env);
      }
      if (request.method === "POST" && pathname === "/auth/start") {
        return cors(await authStart(env));
      }
      if (request.method === "POST" && pathname === "/auth/poll") {
        return cors(await authPoll(request, env));
      }
      if (request.method === "POST" && pathname === "/usage") {
        return cors(await usage(request, env));
      }
      // One pixel-free page view. No cookie, no id, no query string, no
      // referrer: the page name and the day, and nothing else. That is
      // enough to answer "how many people visited krate.tech", which is the
      // number that matters before a launch, and it cannot identify anyone.
      if (request.method === "POST" && pathname === "/view") {
        try {
          const body = await request.json();
          const page = String(body.page || "/").slice(0, 64);
          env.USAGE.writeDataPoint({
            blobs: ["view", page],
            doubles: [1],
            indexes: ["view"],
          });
        } catch {
          // A malformed beacon is not worth an error to the visitor.
        }
        return cors(new Response(null, { status: 204 }));
      }
      if (request.method === "GET" && pathname === "/stats") {
        return cors(await stats(env));
      }
      if (request.method === "GET" && pathname === "/apps") {
        return cors(await list(env));
      }
      if (request.method === "POST" && pathname.startsWith("/shot/")) {
        return cors(await putShot(request, pathname.slice(6), env));
      }
      if (request.method === "GET" && pathname.startsWith("/shot/")) {
        return cors(await getShot(pathname.slice(6), env));
      }
      if (request.method === "GET" && pathname.startsWith("/a/")) {
        return cors(await fetchBundle(request, url, pathname.slice(3), env));
      }
      return cors(text("not found", 404));
    } catch (err) {
      // Never leak a stack trace to a caller; log it for us instead.
      console.error("unhandled", err && err.stack ? err.stack : String(err));
      return cors(text("something went wrong on our side", 500));
    }
  },
};

// ---------------------------------------------------------------- publishing

async function publish(request, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) {
    return text(
      "publishing needs a GitHub sign-in -- run `krate publish` and it will ask",
      401,
    );
  }

  const body = new Uint8Array(await request.arrayBuffer());
  if (body.length === 0) {
    return text("empty body", 400);
  }
  if (body.length > MAX_UPLOAD_BYTES) {
    return text("bundle too large (5 MiB max)", 413);
  }

  // It must actually be a .krate. Checking here keeps the store from filling
  // with things `krate run` would only reject later, and it is the one piece
  // of validation worth doing at the door.
  const problem = looksLikeKrate(body);
  if (problem) {
    return text(`not a valid .krate bundle: ${problem}`, 422);
  }

  const hash = await sha256Hex(body);

  // Content-addressed: republishing the same bytes is a no-op that returns the
  // same URL, so a person who publishes twice does not get two entries.
  const existing = await env.BUNDLES.head(hash);
  if (!existing) {
    await env.BUNDLES.put(hash, body, {
      httpMetadata: { contentType: "application/octet-stream" },
    });
  }

  const meta = {
    name: header(request, "x-krate-name") || "Untitled app",
    description: header(request, "x-krate-description") || "",
    // A small fixed shelf list: free-text categories fragment a gallery.
    category: (() => {
      const c = (header(request, "x-krate-category") || "").toLowerCase();
      return ["games", "productivity", "tools", "media", "learning"].includes(c)
        ? c
        : "apps";
    })(),
    author: identity.name || identity.login,
    author_login: identity.login,
    author_url: `https://github.com/${identity.login}`,
    avatar_url: identity.avatar_url || "",
    published: Math.floor(Date.now() / 1000),
    size: body.length,
  };

  // Keyed so KV's own lexicographic listing comes back newest-first when
  // reversed, which saves sorting the whole set on every page load.
  let listed = true;
  try {
    await env.APPS.put(`app:${hash}`, JSON.stringify(meta));
  } catch (e) {
    // KV write quota exhausted. The bundle is already safe in R2 and the
    // URL works -- refusing the whole publish over the gallery row would
    // let a metadata write take the product down. Say what degraded.
    listed = false;
  }

  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");

  // A short link people can actually read out loud. The 64-hex URL is the
  // content address and works forever -- every link already sent stays
  // valid -- but nobody pastes 64 characters into a chat happily. The alias
  // is the hash's own prefix, so it is stable across republishes of the
  // same bytes; on the astronomically unlikely prefix collision, widen once,
  // then fall back to the full hash rather than ever serving the wrong app.
  let short = hash;
  try {
    for (const len of [12, 16]) {
      const candidate = hash.slice(0, len);
      const taken = await env.APPS.get(`alias:${candidate}`);
      if (!taken || taken === hash) {
        await env.APPS.put(`alias:${candidate}`, hash);
        short = candidate;
        break;
      }
    }
  } catch (e) {
    // Alias minting is a nicety; quota trouble must not fail a publish.
  }
  const result = { url: `${base}/a/${short}`, full_url: `${base}/a/${hash}`, id: hash };
  if (!listed) {
    result.note =
      "published and runnable at the URL, but the gallery listing is " +
      "delayed -- republishing tomorrow will list it";
  }
  return json(result);
}

/// Store a screenshot for an app already published.
///
/// Separate from the bundle upload so a shot can be added or replaced without
/// republishing, and so a publisher with no screenshot still gets a working
/// listing rather than a rejection.
async function putShot(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);

  const body = new Uint8Array(await request.arrayBuffer());
  // A PNG and a sane size. Anything else is refused rather than served back
  // to a browser as an image later.
  if (body.length === 0 || body.length > 2 * 1024 * 1024) {
    return text("a shot must be a PNG under 2 MiB", 413);
  }
  if (!(body[0] === 0x89 && body[1] === 0x50 && body[2] === 0x4e && body[3] === 0x47)) {
    return text("that is not a PNG", 422);
  }
  await env.BUNDLES.put(`shot:${hash}`, body, {
    httpMetadata: { contentType: "image/png" },
  });
  return json({ ok: true });
}

async function getShot(hash, env) {
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const object = await env.BUNDLES.get(`shot:${hash}`);
  if (!object) return text("not found", 404);
  return new Response(object.body, {
    headers: {
      "content-type": "image/png",
      "cache-control": "public, max-age=86400",
    },
  });
}

// ----------------------------------------------------------------- fetching

/// What kind of client is on the other end of a /a/ hit. The CLI and every
/// other tool asks for bytes; only a human's browser says `Accept:
/// text/html`. Among browsers the user agent splits phone from desktop.
/// This is the M0 measurement from the mobile plan: what fraction of
/// shared-link opens land on a device that cannot run the app today.
function classifyClient(request) {
  const accept = request.headers.get("accept") || "";
  if (!accept.includes("text/html")) return "tool";
  const ua = request.headers.get("user-agent") || "";
  if (/Android/i.test(ua)) return "mobile-android";
  if (/iPhone|iPad|iPod/i.test(ua)) return "mobile-ios";
  if (/Mobile/i.test(ua)) return "mobile-other";
  return "desktop-browser";
}

async function fetchBundle(request, url, hash, env) {
  // A short link is the hash's own prefix; resolve it to the full content
  // address first. The strict hex checks stay -- they are the guard that
  // closes off every path-traversal shape at once.
  if (/^[0-9a-f]{8,32}$/.test(hash)) {
    const full = await env.APPS.get(`alias:${hash}`);
    if (!full) {
      return text("not found", 404);
    }
    hash = full;
  }
  if (!/^[0-9a-f]{64}$/.test(hash)) {
    return text("not found", 404);
  }
  const object = await env.BUNDLES.get(hash);
  if (!object) {
    return text("not found", 404);
  }

  // Count the hit before deciding what to serve, one Analytics Engine point
  // in the same dataset the CLI feeds: action "link", the client class in
  // the os slot, the app's hash as the index. No KV -- the K-082 rule.
  const client = classifyClient(request);
  if (env.USAGE) {
    env.USAGE.writeDataPoint({
      blobs: ["link", "-", client, "ok", "direct", new Date().toISOString().slice(0, 10)],
      doubles: [1],
      indexes: [hash],
    });
  }

  // A phone browser gets a small page instead of a file the phone cannot
  // open -- the soft landing from the mobile plan. `?dl=1` still hands the
  // raw bytes to anyone who insists.
  if (client.startsWith("mobile-") && !url.searchParams.has("dl")) {
    return await mobileLanding(hash, env);
  }

  // Name the download after the app, not its hash. "84380a400d91.krate" tells
  // the person nothing, sorts meaninglessly in a downloads folder, and looks
  // like something went wrong -- the app already knows what it is called.
  const meta = await env.APPS.get(`app:${hash}`);
  let filename = `${hash.slice(0, 12)}.krate`;
  if (meta) {
    try {
      const name = JSON.parse(meta).name || "";
      const slug = name
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 48);
      if (slug) filename = `${slug}.krate`;
    } catch (_) {
      // A bad metadata record must not stop the download.
    }
  }

  return new Response(object.body, {
    headers: {
      "content-type": "application/octet-stream",
      // Content-addressed, so a bundle at a given URL can never change and
      // may be cached forever. Vary, because the same URL serves a phone a
      // landing page instead.
      "cache-control": "public, max-age=31536000, immutable",
      vary: "accept, user-agent",
      "content-disposition": `attachment; filename="${filename}"`,
    },
  });
}

/// The page a phone sees when it taps a shared app link. Honest about
/// today's boundary, and it keeps the link alive instead of dumping a file
/// the phone cannot open: copy the link, open it on a computer, done.
async function mobileLanding(hash, env) {
  let name = "A Krate app";
  let author = "";
  const meta = await env.APPS.get(`app:${hash}`);
  if (meta) {
    try {
      const parsed = JSON.parse(meta);
      if (parsed.name) name = parsed.name;
      if (parsed.author) author = parsed.author;
    } catch (_) {
      // A bad metadata record must not stop the page.
    }
  }
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const link = `${base}/a/${hash}`;
  const shot = (await env.BUNDLES.head(`shot:${hash}`))
    ? `${base}/shot/${hash}`
    : null;
  const esc = (s) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(name)} — a Krate app</title>
<style>
  body { margin: 0; background: #0b0d12; color: #fff; font-family: -apple-system, system-ui, sans-serif;
         min-height: 100vh; display: flex; align-items: center; justify-content: center; }
  main { max-width: 340px; padding: 32px 24px; text-align: center; }
  img { width: 100%; border-radius: 16px; border: 1px solid rgba(255,255,255,0.12); margin-bottom: 20px; }
  h1 { font-size: 22px; margin: 0 0 4px; }
  .by { color: rgba(255,255,255,0.5); font-size: 14px; margin: 0 0 20px; }
  p { color: rgba(255,255,255,0.7); font-size: 15px; line-height: 1.5; margin: 0 0 24px; }
  button { width: 100%; padding: 14px; border: 0; border-radius: 24px; background: #6b8cff;
           color: #fff; font-size: 16px; font-weight: 600; margin-bottom: 12px; }
  a { color: rgba(255,255,255,0.55); font-size: 13px; }
</style>
</head>
<body>
<main>
  ${shot ? `<img src="${shot}" alt="">` : ""}
  <h1>${esc(name)}</h1>
  ${author ? `<p class="by">by ${esc(author)}</p>` : ""}
  <p>This is a Krate app. Krate runs on computers today -- open this link on your Mac, Windows, or Linux machine and it runs there.</p>
  <button onclick="navigator.clipboard.writeText('${link}').then(()=>this.textContent='Copied')">Copy the link</button>
  <a href="https://krate.tech">What is Krate?</a> · <a href="${link}?dl=1">Download the file anyway</a>
</main>
</body>
</html>`;
  return new Response(html, {
    headers: {
      "content-type": "text/html; charset=utf-8",
      // The page may change as mobile support lands; never let a phone pin it.
      "cache-control": "no-cache",
      vary: "accept, user-agent",
    },
  });
}

async function list(env) {
  const listing = await env.APPS.list({ prefix: "app:", limit: 200 });
  const apps = [];
  for (const key of listing.keys) {
    const raw = await env.APPS.get(key.name);
    if (!raw) continue;
    const hash = key.name.slice("app:".length);
    const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
    const shot = await env.BUNDLES.head(`shot:${hash}`);
    apps.push({
      id: hash,
      url: `${base}/a/${hash}`,
      shot: shot ? `${base}/shot/${hash}` : null,
      meta: JSON.parse(raw),
    });
  }
  apps.sort((a, b) => (b.meta.published || 0) - (a.meta.published || 0));
  return json({ apps });
}

// --------------------------------------------------------------------- usage

// How many people use Krate, and nothing about who they are. The CLI sends a
// random install id, a version, an OS name, and one of three action words.
// Nothing here reads an IP, sets a cookie, or stores a request body.

async function usage(request, env) {
  let event;
  try {
    event = await request.json();
  } catch (_) {
    return text("bad request", 400);
  }

  // A closed set on this side too. Anything unexpected is dropped rather than
  // stored, so a bad or malicious client cannot turn this into free storage.
  const actions = ["install", "make", "open", "publish"];
  const id = String(event.id || "").slice(0, 64);
  const action = actions.includes(event.action) ? event.action : null;
  if (!/^[0-9a-f]{8,64}$/.test(id) || !action) {
    return text("ok");
  }
  const version = String(event.version || "").slice(0, 32);
  const os = String(event.os || "").slice(0, 16);
  const day = new Date().toISOString().slice(0, 10);

  // Why an open did not end in a running app. A closed set here as well as in
  // the CLI: this string is written straight into the dataset, so it must not
  // be able to carry a path, a URL, or an app name. Anything unrecognised
  // becomes "other", which is also the signal that the list needs a new entry.
  //
  // "refused" is the one to watch. The permission wall turning an app away is
  // the product working, and it was previously counted as a plain failure --
  // which is how the 9% open-failure rate (K-100) ended up unreadable.
  const reasons = [
    "refused",
    "not-found",
    "bad-bundle",
    "bad-manifest",
    "version-too-old",
    "no-window",
    "app-failed",
    "other",
  ];
  const why = reasons.includes(event.why) ? event.why : event.ok === false ? "other" : "-";

  // One Analytics Engine data point, and no KV at all. The first version
  // wrote two KV keys per ping (seen: plus a read-modify-write count:), and
  // a single busy day -- CI replays plus one developer -- blew the free
  // tier's 1,000 puts. Because publishes and sign-ins share the namespace,
  // telemetry exhaustion took down the product: counting must never sit on
  // the same budget as publishing. Analytics Engine is Cloudflare's counter
  // product, unmetered at this scale, and uniques fall out of the index.
  if (env.USAGE) {
    env.USAGE.writeDataPoint({
      blobs: [
        action,
        version,
        os,
        event.ok === false ? "failed" : "ok",
        event.ai === true ? "by-ai" : "direct",
        day,
        why,
      ],
      doubles: [1],
      indexes: [id],
    });
  }
  return text("ok");
}



/// Why opens failed, actually queried rather than described.
///
/// The rate is meaningless without this split. `refused` is the permission
/// wall turning an app away, which is the product working exactly as
/// designed -- counting it as a failure is how the rate ended up looking
/// alarming and unactionable at the same time (K-100). Anything else is a
/// real defect worth chasing.
async function failureReasons(env, days) {
  if (!env.CF_ANALYTICS_TOKEN) return null;
  const sql =
    "SELECT blob7 AS why, sum(_sample_interval) AS n FROM krate_usage " +
    "WHERE blob1 = 'open' AND blob4 = 'failed' " +
    `AND timestamp > now() - INTERVAL '${days}' DAY ` +
    "GROUP BY why ORDER BY n DESC";
  const res = await fetch(
    `https://api.cloudflare.com/client/v4/accounts/${env.CF_ACCOUNT_ID}/analytics_engine/sql`,
    { method: "POST", headers: { Authorization: `Bearer ${env.CF_ANALYTICS_TOKEN}` }, body: sql },
  );
  if (!res.ok) return { error: `reason query failed: ${res.status}` };
  const body = await res.json();
  const out = {};
  for (const row of body.data || []) out[row.why || "-"] = Number(row.n);
  return out;
}

/// Read the live numbers out of Analytics Engine.
///
/// Counting moved to Analytics Engine on 2026-08-10 (a busy day of telemetry
/// in KV returned 429s to every publish and sign-in), and `/stats` kept
/// reading the retired KV keys -- so from that day on it reported nothing
/// newer, while the events themselves were arriving fine. That is the worst
/// shape a metric can have: it looks alive and is four days stale.
///
/// Needs a read token. Without one this returns null and `/stats` says so,
/// rather than reporting a zero that reads as "nobody used it".
async function liveStats(env, days) {
  if (!env.CF_ANALYTICS_TOKEN) return null;
  const sql =
    "SELECT toDate(timestamp) AS day, blob1 AS action, blob4 AS outcome, " +
    // The install id is the INDEX (index1), not a blob. blob3 is the
    // operating system, so counting that distinct answered "how many
    // platforms" -- it returned 7 against 74 installs in a single day, the
    // kind of number that looks plausible until you check it.
    "sum(_sample_interval) AS n, count(DISTINCT index1) AS installs " +
    "FROM krate_usage " +
    `WHERE timestamp > now() - INTERVAL '${days}' DAY ` +
    "GROUP BY day, action, outcome ORDER BY day";
  const res = await fetch(
    `https://api.cloudflare.com/client/v4/accounts/${env.CF_ACCOUNT_ID}/analytics_engine/sql`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${env.CF_ANALYTICS_TOKEN}` },
      body: sql,
    },
  );
  if (!res.ok) return { error: `analytics query failed: ${res.status}` };
  const body = await res.json();
  const byDay = {};
  let installs = 0;
  for (const row of body.data || []) {
    const day = (byDay[row.day] ||= {});
    const key = row.outcome === "failed" ? `${row.action}-failed` : row.action;
    day[key] = (day[key] || 0) + Number(row.n);
    if (row.action === "install") installs += Number(row.installs);
  }
  return { actions_by_day: byDay, distinct_installs: installs };
}

/// The numbers, for us. Distinct installs and action counts by day.
async function stats(env) {
  const seen = await env.APPS.list({ prefix: "seen:", limit: 1000 });
  const counts = await env.APPS.list({ prefix: "count:", limit: 1000 });

  const installsByDay = {};
  const allInstalls = new Set();
  for (const key of seen.keys) {
    const [, day, id] = key.name.split(":");
    installsByDay[day] = (installsByDay[day] || 0) + 1;
    allInstalls.add(id);
  }
  const actions = {};
  for (const key of counts.keys) {
    const [, day, action] = key.name.split(":");
    const value = parseInt((await env.APPS.get(key.name)) || "0", 10);
    actions[day] = actions[day] || {};
    actions[day][action] = value;
  }

  // Everything since 2026-08-10 lives in Analytics Engine, so read it and
  // put it beside the KV history rather than leaving the endpoint frozen.
  const live = await liveStats(env, 30);
  const why = await failureReasons(env, 30);

  return json({
    live: live || {
      note:
        "no CF_ANALYTICS_TOKEN set on the worker, so the numbers since " +
        "2026-08-10 cannot be read here. Add it with: wrangler secret put " +
        "CF_ANALYTICS_TOKEN (needs Account Analytics:Read).",
    },
    // KV history stops on 2026-08-10; its TTLs retire it over 90 days.
    // Everything after that date is in the USAGE Analytics Engine dataset
    // (krate_usage), queryable from the dashboard or the SQL API -- moved
    // there so counting can never again exhaust the namespace publishes
    // and sign-ins live in.
    legacy_kv_until: "2026-08-10",
    distinct_installs_90d: allInstalls.size,
    active_installs_by_day: installsByDay,
    actions_by_day: actions,
    // Why an open failed lives in the Analytics Engine dataset, not here:
    // this endpoint only ever reads the retired KV keys. Until a reader for
    // that dataset exists, say where the answer is rather than leave the
    // impression that `open-failed` above has no explanation (K-100).
    // Actually queried now, not just described. `refused` is the wall
    // working; exclude it before quoting a failure rate anywhere.
    open_failure_reasons_30d: why || { note: "needs CF_ANALYTICS_TOKEN" },
    open_failure_reasons: {
      note: "blob7 of the krate_usage dataset, from v0.1.12 onward",
      query:
        "SELECT blob7 AS why, sum(_sample_interval) AS n FROM krate_usage " +
        "WHERE blob1 = 'open' AND blob4 = 'failed' GROUP BY why ORDER BY n DESC",
      values: [
        "refused",
        "not-found",
        "bad-bundle",
        "bad-manifest",
        "version-too-old",
        "no-window",
        "app-failed",
        "other",
      ],
    },
  });
}

// -------------------------------------------------------------- browser auth

// The website cannot talk to GitHub's device endpoints itself: they send no
// CORS headers, so a browser request is blocked before it starts. These two
// routes are a thin proxy, and deliberately thin -- they add no state, hold no
// secret (the device flow has none), and simply pass a call through.

const GITHUB_CLIENT_ID = "Ov23liV2n8Dxi0okyv0F";

// ------------------------------------------------------------------ accounts
//
// A user is one KV record; each way of proving who you are (GitHub, Google,
// an email you can read) maps to it through an `ident:` key, so signing in
// with a second provider later lands on the same account when the email
// matches. A session is a `krs_` token with a 90-day TTL; the engine sends it
// as a bearer exactly like a GitHub token, and verifyIdentity below accepts
// either, so nothing published under the old flow breaks.

async function ensureUser(env, provider, stableId, profile) {
  const identKey = `ident:${provider}:${stableId}`;
  let userId = await env.APPS.get(identKey);
  if (!userId && profile.email) {
    // Same inbox, same person: unify across providers by verified email.
    userId = await env.APPS.get(`ident:email:${profile.email.toLowerCase()}`);
  }
  if (!userId) {
    userId = crypto.randomUUID();
    await env.APPS.put(`user:${userId}`, JSON.stringify({
      id: userId,
      created: Date.now(),
      name: profile.name || "",
      login: profile.login || (profile.email ? profile.email.split("@")[0] : ""),
      email: profile.email || "",
      avatar_url: profile.avatar_url || "",
      providers: [provider],
    }));
  } else {
    const record = JSON.parse((await env.APPS.get(`user:${userId}`)) || "{}");
    if (!(record.providers || []).includes(provider)) {
      record.providers = [...(record.providers || []), provider];
    }
    record.name = record.name || profile.name || "";
    record.avatar_url = record.avatar_url || profile.avatar_url || "";
    record.email = record.email || profile.email || "";
    await env.APPS.put(`user:${userId}`, JSON.stringify(record));
  }
  await env.APPS.put(identKey, userId);
  if (profile.email) {
    await env.APPS.put(`ident:email:${profile.email.toLowerCase()}`, userId);
  }
  return JSON.parse(await env.APPS.get(`user:${userId}`));
}

async function newSession(env, userId) {
  const token = `krs_${crypto.randomUUID().replaceAll("-", "")}`;
  await env.APPS.put(`session:${token}`, userId, { expirationTtl: 90 * 24 * 3600 });
  return token;
}

/// Deliver a finished sign-in to wherever it started. Everything rides in
/// the fragment, which never leaves the browser.
function deliver(from, user, token) {
  const hand = new URLSearchParams({
    token,
    login: user.login || "",
    name: user.name || "",
    avatar_url: user.avatar_url || "",
  });
  const suffix = from === "app" ? "?app=1" : "";
  return Response.redirect(`https://krate.tech/login/done/${suffix}#${hand.toString()}`, 302);
}

// ------------------------------------------------------------------- google

async function googleStart(url, env) {
  if (!env.GOOGLE_CLIENT_ID || !env.GOOGLE_CLIENT_SECRET) {
    return text("Google sign-in is not configured on this hub yet.", 503);
  }
  const from = url.searchParams.get("from") === "app" ? "app" : "web";
  const state = crypto.randomUUID();
  await env.APPS.put(`login:${state}`, from, { expirationTtl: 600 });
  const auth = new URL("https://accounts.google.com/o/oauth2/v2/auth");
  auth.searchParams.set("client_id", env.GOOGLE_CLIENT_ID);
  auth.searchParams.set("redirect_uri", `${env.PUBLIC_BASE}/login/google/callback`);
  auth.searchParams.set("response_type", "code");
  auth.searchParams.set("scope", "openid email profile");
  auth.searchParams.set("state", state);
  return Response.redirect(auth.toString(), 302);
}

async function googleCallback(url, env) {
  const state = url.searchParams.get("state") || "";
  const from = await env.APPS.get(`login:${state}`);
  if (!from) return text("This sign-in link expired. Start again from krate.tech/login.", 400);
  await env.APPS.delete(`login:${state}`);
  const exchange = await fetch("https://oauth2.googleapis.com/token", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: env.GOOGLE_CLIENT_ID,
      client_secret: env.GOOGLE_CLIENT_SECRET,
      code: url.searchParams.get("code") || "",
      grant_type: "authorization_code",
      redirect_uri: `${env.PUBLIC_BASE}/login/google/callback`,
    }),
  });
  const result = await exchange.json();
  if (!result.id_token) return text("Google did not complete the sign-in. Try again.", 502);
  // tokeninfo validates the signature and audience for us.
  const info = await fetch(
    `https://oauth2.googleapis.com/tokeninfo?id_token=${encodeURIComponent(result.id_token)}`,
  );
  if (!info.ok) return text("Google's answer could not be verified.", 502);
  const claims = await info.json();
  if (claims.aud !== env.GOOGLE_CLIENT_ID) return text("Wrong audience.", 400);
  const user = await ensureUser(env, "google", claims.sub, {
    email: claims.email,
    name: claims.name,
    avatar_url: claims.picture,
  });
  return deliver(from, user, await newSession(env, user.id));
}

// -------------------------------------------------------------------- email

async function emailStart(request, env) {
  if (!env.RESEND_API_KEY) {
    return text("Email sign-in is not configured on this hub yet.", 503);
  }
  const { email, from } = await request.json().catch(() => ({}));
  if (!email || !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
    return text("That does not look like an email address.", 400);
  }
  const token = crypto.randomUUID().replaceAll("-", "");
  await env.APPS.put(
    `email:${token}`,
    JSON.stringify({ email: email.toLowerCase(), from: from === "app" ? "app" : "web" }),
    { expirationTtl: 900 },
  );
  const link = `${env.PUBLIC_BASE}/login/email/verify?t=${token}`;
  const sent = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.RESEND_API_KEY}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      from: "Krate <login@krate.tech>",
      to: [email],
      subject: "Sign in to Krate",
      text: `Click to sign in to Krate:\n\n${link}\n\nThe link works once and expires in 15 minutes. If you did not ask for this, ignore it.`,
    }),
  });
  if (!sent.ok) return text("The sign-in email could not be sent. Try again.", 502);
  return json({ sent: true });
}

async function emailVerify(url, env) {
  const token = url.searchParams.get("t") || "";
  const stored = await env.APPS.get(`email:${token}`);
  if (!stored) return text("This sign-in link expired or was already used.", 400);
  await env.APPS.delete(`email:${token}`);
  const { email, from } = JSON.parse(stored);
  const user = await ensureUser(env, "email", email, { email });
  return deliver(from, user, await newSession(env, user.id));
}

// --------------------------------------------------------------- web sign-in
//
// The browser flow behind krate.tech/login. The device flow above stays for
// terminals; this one is for the page a person actually sees. Same GitHub
// OAuth app, but the authorization-code exchange requires the client secret,
// which lives as a Worker secret (`wrangler secret put GITHUB_CLIENT_SECRET`)
// and never anywhere else.

async function loginStart(url, env) {
  if (!env.GITHUB_CLIENT_SECRET) {
    return text(
      "Sign-in is not configured on this hub yet: the GITHUB_CLIENT_SECRET " +
        "worker secret is missing.",
      503,
    );
  }
  // Where to deliver the person afterwards. "app" means hand the identity to
  // the desktop app through its URL scheme; anything else means the site.
  const from = url.searchParams.get("from") === "app" ? "app" : "web";
  // The state ties the callback to this start. Ten minutes is enough to type
  // a password and approve; an unused state simply expires.
  const state = crypto.randomUUID();
  await env.APPS.put(`login:${state}`, from, { expirationTtl: 600 });

  const auth = new URL("https://github.com/login/oauth/authorize");
  auth.searchParams.set("client_id", GITHUB_CLIENT_ID);
  auth.searchParams.set("redirect_uri", `${env.PUBLIC_BASE}/login/callback`);
  auth.searchParams.set("scope", "read:user");
  auth.searchParams.set("state", state);
  return Response.redirect(auth.toString(), 302);
}

async function loginCallback(url, env) {
  const state = url.searchParams.get("state") || "";
  const code = url.searchParams.get("code") || "";
  const from = await env.APPS.get(`login:${state}`);
  if (!from) return text("This sign-in link expired. Start again from krate.tech/login.", 400);
  // One shot: a replayed callback with the same state gets the line above.
  await env.APPS.delete(`login:${state}`);

  const exchange = await fetch("https://github.com/login/oauth/access_token", {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: GITHUB_CLIENT_ID,
      client_secret: env.GITHUB_CLIENT_SECRET,
      code,
      redirect_uri: `${env.PUBLIC_BASE}/login/callback`,
    }),
  });
  const result = await exchange.json();
  if (!result.access_token) return text("GitHub did not complete the sign-in. Try again.", 502);

  const user = await fetch("https://api.github.com/user", {
    headers: {
      authorization: `Bearer ${result.access_token}`,
      accept: "application/vnd.github+json",
      "user-agent": "krate-hub",
    },
  });
  if (!user.ok) return text("Signed in, but the profile could not be read.", 502);
  const profile = await user.json();

  const account = await ensureUser(env, "github", String(profile.id), {
    login: profile.login,
    name: profile.name || "",
    avatar_url: profile.avatar_url || "",
    email: (profile.email || "").toLowerCase() || undefined,
  });
  // The GitHub token itself is handed over (not a krs_ session): the engine
  // already publishes with it and the hub already verifies it. The account
  // record exists either way, so a later Google or email sign-in with the
  // same address lands on this same user.
  return deliver(from, { ...account, login: profile.login }, result.access_token);
}

async function authStart(env) {
  const response = await fetch("https://github.com/login/device/code", {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ client_id: GITHUB_CLIENT_ID, scope: "read:user" }),
  });
  if (!response.ok) return text("GitHub would not start a sign-in", 502);
  return json(await response.json());
}

async function authPoll(request, env) {
  const { device_code } = await request.json();
  if (!device_code) return text("no device code", 400);

  const response = await fetch("https://github.com/login/oauth/access_token", {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: GITHUB_CLIENT_ID,
      device_code,
      grant_type: "urn:ietf:params:oauth:grant-type:device_code",
    }),
  });
  const result = await response.json();
  if (!result.access_token) {
    // authorization_pending is the normal answer while someone is still
    // typing the code, so this is not an error path.
    return json({ error: result.error || "pending" });
  }

  const user = await fetch("https://api.github.com/user", {
    headers: {
      authorization: `Bearer ${result.access_token}`,
      accept: "application/vnd.github+json",
      "user-agent": "krate-hub",
    },
  });
  if (!user.ok) return text("signed in, but the profile could not be read", 502);
  const profile = await user.json();

  return json({
    identity: {
      login: profile.login,
      name: profile.name || "",
      avatar_url: profile.avatar_url || "",
      token: result.access_token,
    },
  });
}

// ------------------------------------------------------------------ identity

/// Confirm the bearer token really belongs to a GitHub account.
///
/// The token is checked against GitHub rather than trusted, because anyone can
/// send a header. The answer is cached in KV so publishing several apps does
/// not mean several round trips, and expires so a revoked token stops working
/// without us tracking revocations ourselves.
async function verifyGitHub(request, env) {
  const auth = request.headers.get("authorization") || "";
  const token = auth.replace(/^Bearer\s+/i, "").trim();
  if (!token) return null;

  // A Krate session (Google or email sign-in) is as good as a GitHub token.
  if (token.startsWith("krs_")) {
    const userId = await env.APPS.get(`session:${token}`);
    if (!userId) return null;
    const record = JSON.parse((await env.APPS.get(`user:${userId}`)) || "null");
    if (!record) return null;
    return { login: record.login, name: record.name, avatar_url: record.avatar_url };
  }

  const cacheKey = `token:${await sha256Hex(new TextEncoder().encode(token))}`;
  const cached = await env.APPS.get(cacheKey);
  if (cached) return JSON.parse(cached);

  const response = await fetch("https://api.github.com/user", {
    headers: {
      authorization: `Bearer ${token}`,
      accept: "application/vnd.github+json",
      // GitHub rejects API requests with no user agent.
      "user-agent": "krate-hub",
    },
  });
  if (!response.ok) return null;

  const user = await response.json();
  const identity = {
    login: user.login,
    name: user.name || "",
    avatar_url: user.avatar_url || "",
  };
  try {
    await env.APPS.put(cacheKey, JSON.stringify(identity), {
      expirationTtl: IDENTITY_TTL_SECONDS,
    });
  } catch (e) {
    // The cache is an optimization. A publish died with "KV put() limit
    // exceeded" thrown from THIS line -- a cache write taking the product
    // down, the K-082 disease in a second spot. Without the cache the next
    // call costs one extra GitHub round trip, which is nothing.
  }
  return identity;
}

// ------------------------------------------------------------------- helpers

/// A .krate is a zip carrying manifest.toml and code.wasm. This checks the
/// shape without unzipping: the local file headers name their entries in
/// plain bytes near the start, which is enough to reject something that is
/// not a bundle at all.
function looksLikeKrate(bytes) {
  if (bytes.length < 4) return "too short to be a zip";
  // "PK\x03\x04"
  if (!(bytes[0] === 0x50 && bytes[1] === 0x4b && bytes[2] === 0x03 && bytes[3] === 0x04)) {
    return "not a zip archive";
  }
  const haystack = new TextDecoder("latin1").decode(bytes);
  if (!haystack.includes("manifest.toml")) return "no manifest.toml inside";
  if (!haystack.includes("code.wasm")) return "no code.wasm inside";
  return null;
}

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function header(request, name) {
  const value = request.headers.get(name);
  return value ? value.trim() : "";
}

function text(body, status = 200) {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}

function json(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function cors(response) {
  const headers = new Headers(response.headers);
  headers.set("access-control-allow-origin", "*");
  headers.set("access-control-allow-methods", "GET, POST, OPTIONS");
  headers.set(
    "access-control-allow-headers",
    "authorization, content-type, x-krate-name, x-krate-description, x-krate-category",
  );
  return new Response(response.body, { status: response.status, headers });
}
