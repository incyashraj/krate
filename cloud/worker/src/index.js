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
      if (request.method === "POST" && pathname === "/auth/start") {
        return cors(await authStart(env));
      }
      if (request.method === "POST" && pathname === "/auth/poll") {
        return cors(await authPoll(request, env));
      }
      if (request.method === "POST" && pathname === "/usage") {
        return cors(await usage(request, env));
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
        return cors(await fetchBundle(pathname.slice(3), env));
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
    author: identity.name || identity.login,
    author_login: identity.login,
    author_url: `https://github.com/${identity.login}`,
    avatar_url: identity.avatar_url || "",
    published: Math.floor(Date.now() / 1000),
    size: body.length,
  };

  // Keyed so KV's own lexicographic listing comes back newest-first when
  // reversed, which saves sorting the whole set on every page load.
  await env.APPS.put(`app:${hash}`, JSON.stringify(meta));

  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  return json({ url: `${base}/a/${hash}`, id: hash });
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

async function fetchBundle(hash, env) {
  // The hash is the object key, so it must be a bare hex string. Rejecting
  // anything else closes off every path-traversal shape at once.
  if (!/^[0-9a-f]{64}$/.test(hash)) {
    return text("not found", 404);
  }
  const object = await env.BUNDLES.get(hash);
  if (!object) {
    return text("not found", 404);
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
      // may be cached forever.
      "cache-control": "public, max-age=31536000, immutable",
      "content-disposition": `attachment; filename="${filename}"`,
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

  // Two keys: one marking this install seen today, one counting the action.
  // Together they answer "how many people" and "how much are they doing"
  // without keeping a log of individual events.
  await env.APPS.put(`seen:${day}:${id}`, JSON.stringify({ version, os }), {
    // Ninety days is long enough to see a trend and short enough that this
    // never becomes an archive.
    expirationTtl: 90 * 24 * 60 * 60,
  });
  await bump(env, `count:${day}:${action}`);
  if (event.ok === false) {
    await bump(env, `count:${day}:${action}-failed`);
  }
  if (event.ai === true) {
    await bump(env, `count:${day}:${action}-by-ai`);
  }
  return text("ok");
}

async function bump(env, key) {
  const current = parseInt((await env.APPS.get(key)) || "0", 10);
  await env.APPS.put(key, String(current + 1), {
    expirationTtl: 90 * 24 * 60 * 60,
  });
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

  return json({
    distinct_installs_90d: allInstalls.size,
    active_installs_by_day: installsByDay,
    actions_by_day: actions,
  });
}

// -------------------------------------------------------------- browser auth

// The website cannot talk to GitHub's device endpoints itself: they send no
// CORS headers, so a browser request is blocked before it starts. These two
// routes are a thin proxy, and deliberately thin -- they add no state, hold no
// secret (the device flow has none), and simply pass a call through.

const GITHUB_CLIENT_ID = "Ov23liV2n8Dxi0okyv0F";

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
  await env.APPS.put(cacheKey, JSON.stringify(identity), {
    expirationTtl: IDENTITY_TTL_SECONDS,
  });
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
    "authorization, content-type, x-krate-name, x-krate-description",
  );
  return new Response(response.body, { status: response.status, headers });
}
