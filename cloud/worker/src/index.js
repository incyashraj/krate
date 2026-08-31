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
      if (request.method === "POST" && pathname === "/founding") {
        // The founding 200: an email and a timestamp, nothing else. No
        // checkout here -- this is the list that gets the $79/yr lock when
        // Studio leaves preview.
        return cors(await founding(request, env));
      }
      if (request.method === "POST" && pathname === "/makeit") {
        // "We'll make it for you": when a build dies on someone, Studio
        // offers a human fallback. What lands here is what we need to
        // build their app by hand: their request, their answers to the
        // AI's questions, and an email to send the file back to.
        return cors(await makeit(request, env));
      }
      if (request.method === "POST" && pathname === "/usage") {
        return cors(await usage(request, env));
      }
      // Shared stores: a key-value bucket shared between the machines that
      // hold its invite code. This is how a generated app becomes a
      // household app -- a shopping list two people see -- without the app
      // author running a backend or anyone creating an account. Possession
      // of the code IS the membership, exactly like a shared album link;
      // the runtime tells the person that plainly before granting
      // `store.shared`.
      if (request.method === "POST" && pathname === "/share/new") {
        return cors(await shareNew(env));
      }
      if (request.method === "GET" && pathname.startsWith("/play/")) {
        // A live game room. The code IS the room: the Durable Object named
        // by it springs into existence on the first connection, the second
        // connection becomes player 2, and everything either sends is
        // relayed to the other. Same possession-is-membership model as the
        // shared store: anyone holding the code is in the room.
        if (request.headers.get("Upgrade") !== "websocket") {
          return cors(text("this endpoint speaks WebSocket", 426));
        }
        const code = pathname.slice("/play/".length);
        if (!/^[a-z0-9]{4,32}$/.test(code)) {
          return cors(text("bad room code", 400));
        }
        const id = env.ROOMS.idFromName(code);
        return env.ROOMS.get(id).fetch(request);
      }
      if (request.method === "GET" && pathname.startsWith("/share/")) {
        return cors(await shareGet(pathname.slice(7), env));
      }
      if (request.method === "PUT" && pathname.startsWith("/share/")) {
        return cors(await sharePut(request, pathname.slice(7), env));
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
      if (request.method === "GET" && pathname.startsWith("/meta/")) {
        // One app's public face, by hash or short alias -- name, size,
        // shot. This is what the receive page personalizes from, and it
        // must work for UNLISTED apps too: possession of the link is the
        // access, exactly like the bytes themselves.
        return cors(await meta(pathname.slice(6), env));
      }
      if (request.method === "POST" && pathname.startsWith("/shot/")) {
        return cors(await putShot(request, pathname.slice(6), env));
      }
      if (request.method === "GET" && pathname.startsWith("/shot/")) {
        return cors(await getShot(pathname.slice(6), env));
      }
      if (request.method === "POST" && pathname.startsWith("/icon/")) {
        return cors(await putIcon(request, pathname.slice(6), env));
      }
      if (request.method === "GET" && pathname.startsWith("/icon/")) {
        return cors(await getIcon(pathname.slice(6), env));
      }
      if (request.method === "POST" && pathname === "/report") {
        return cors(await putReport(request, env));
      }
      if (request.method === "GET" && pathname === "/admin/reports") {
        return cors(await listReports(request, env));
      }
      // ---- billing: the paid plan, end to end -------------------------
      if (request.method === "GET" && pathname === "/billing/config") {
        return cors(await billingConfig(env));
      }
      if (request.method === "POST" && pathname === "/billing/checkout") {
        return cors(await billingCheckout(request, env));
      }
      if (request.method === "POST" && pathname === "/billing/webhook") {
        return await billingWebhook(request, env);
      }
      if (request.method === "GET" && pathname === "/billing/status") {
        return cors(await billingStatus(request, env));
      }
      // ---- the free-tier device counter -------------------------------
      // The three-a-month wall is device-bound. The device keeps its own
      // count (plan.json), and this mirror makes deleting that file
      // pointless: the studio takes the max of both whenever it is online.
      // The device hash is anonymous and is the whole identity here.
      if (request.method === "POST" && pathname === "/plan/count") {
        return cors(await planCount(request, env, true));
      }
      if (request.method === "POST" && pathname === "/plan/get") {
        return cors(await planCount(request, env, false));
      }
      // ---- the account: profile, apps, referrals, the portal ----------
      if (request.method === "GET" && pathname === "/me") {
        return cors(await meProfile(request, env));
      }
      if (request.method === "GET" && pathname === "/my/apps") {
        return cors(await myApps(request, env));
      }
      if (request.method === "POST" && pathname === "/referral/claim") {
        return cors(await referralClaim(request, env));
      }
      if (request.method === "POST" && pathname === "/billing/portal") {
        return cors(await billingPortal(request, env));
      }
      // ---- support: tickets with real conversations -------------------
      if (request.method === "POST" && pathname === "/support/new") {
        return cors(await supportNew(request, env));
      }
      if (request.method === "POST" && pathname === "/support/list") {
        return cors(await supportList(request, env));
      }
      if (request.method === "POST" && pathname === "/support/reply") {
        return cors(await supportReply(request, env));
      }
      // ---- the admin desk ---------------------------------------------
      if (request.method === "GET" && (pathname === "/admin" || pathname === "/admin/")) {
        return adminPage();
      }
      if (pathname.startsWith("/admin/api/")) {
        return cors(await adminApi(request, pathname, env));
      }
      if (request.method === "GET" && pathname.startsWith("/admin/report/")) {
        return cors(await getReport(request, pathname.slice("/admin/report/".length), env));
      }
      if (request.method === "DELETE" && pathname.startsWith("/app/")) {
        return cors(await unpublish(request, pathname.slice(5), env));
      }
      if (request.method === "DELETE" && pathname.startsWith("/blob/")) {
        return cors(await purgeBundle(request, pathname.slice(6), env));
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
    unlisted: header(request, "x-krate-unlisted") === "1",
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

  // The same person republishing the same app is a new version, not a new
  // app: retire every older listing that shares this author and name, so
  // the gallery shows one row per app. The old bundles and aliases stay --
  // every link already sent keeps working -- only the gallery rows go.
  try {
    const listing = await env.APPS.list({ prefix: "app:", limit: 200 });
    for (const key of listing.keys) {
      const otherHash = key.name.slice("app:".length);
      if (otherHash === hash) continue;
      const raw = await env.APPS.get(key.name);
      if (!raw) continue;
      const other = JSON.parse(raw);
      if (other.author_login === meta.author_login && other.name === meta.name) {
        await env.APPS.delete(key.name);
      }
    }
  } catch (e) {
    // Retiring old rows is a nicety; quota trouble must not fail a publish.
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

/// Receive one support report: a zip the studio built after the person
/// agreed to send it. Signed in, so a report has a name attached and the
/// endpoint cannot be used as anonymous storage.
///
/// The bytes go to R2 and a small row to KV, so the admin list is one read
/// rather than a bucket scan.
async function putReport(request, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  const body = new Uint8Array(await request.arrayBuffer());
  if (body.length === 0 || body.length > 12 * 1024 * 1024) {
    return text("a report must be a zip under 12 MiB", 413);
  }
  // PK\x03\x04: a zip and nothing else. The studio builds these; anything
  // else is a client we do not recognise.
  if (!(body[0] === 0x50 && body[1] === 0x4b)) {
    return text("that is not a report file", 422);
  }
  const id = (await sha256Hex(body)).slice(0, 16);
  await env.BUNDLES.put(`report:${id}`, body, {
    httpMetadata: { contentType: "application/zip" },
  });
  const meta = {
    id,
    from: identity.login,
    name: identity.name || identity.login,
    session: header(request, "x-krate-session") || "",
    krate: header(request, "x-krate-version") || "",
    os: header(request, "x-krate-os") || "",
    note: (header(request, "x-krate-note") || "").slice(0, 400),
    size: body.length,
    received: Math.floor(Date.now() / 1000),
    state: "new",
  };
  await env.APPS.put(`report:${id}`, JSON.stringify(meta));
  return json({ ok: true, id });
}

/// Is this caller one of us? A comma-separated list of GitHub logins in the
/// KRATE_ADMINS var, checked against a verified identity -- never a header
/// the caller controls.
async function isAdmin(request, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return null;
  const admins = (env.KRATE_ADMINS || "")
    .split(",")
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
  return admins.includes((identity.login || "").toLowerCase()) ? identity : null;
}

async function listReports(request, env) {
  if (!(await isAdmin(request, env))) return text("not found", 404);
  const listing = await env.APPS.list({ prefix: "report:", limit: 300 });
  const reports = [];
  for (const key of listing.keys) {
    const raw = await env.APPS.get(key.name);
    if (raw) reports.push(JSON.parse(raw));
  }
  reports.sort((a, b) => (b.received || 0) - (a.received || 0));
  return json({ reports });
}

async function getReport(request, id, env) {
  if (!(await isAdmin(request, env))) return text("not found", 404);
  if (!/^[0-9a-f]{16}$/.test(id)) return text("not found", 404);
  const object = await env.BUNDLES.get(`report:${id}`);
  if (!object) return text("not found", 404);
  return new Response(object.body, {
    headers: {
      "content-type": "application/zip",
      "content-disposition": `attachment; filename="krate-report-${id}.zip"`,
    },
  });
}

/// Take an app off the gallery. Only its author can; the bundle and every
/// link already shared keep working -- unpublishing removes the listing,
/// not the content people were sent.
async function unpublish(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`app:${hash}`);
  if (!raw) return text("not listed", 404);
  const meta = JSON.parse(raw);
  if (meta.author_login !== identity.login) {
    return text("only the app's author can remove it", 403);
  }
  // Delisting is not deleting. The listing is one of four places an app
  // exists: the bundle blob (served by /a/<hash> forever), the short-link
  // alias that resolves to it, and the screenshot. Removing only the
  // listing left the app downloadable by anyone holding the URL -- which is
  // the opposite of what "remove it" means to the person clicking it.
  await env.APPS.delete(`app:${hash}`);
  await env.BUNDLES.delete(hash);
  await env.BUNDLES.delete(`shot:${hash}`);
  await env.BUNDLES.delete(`icon:${hash}`);
  // The alias is keyed by prefix, so clear every prefix length a short link
  // could have used.
  for (let length = 8; length <= 32; length += 1) {
    await env.APPS.delete(`alias:${hash.slice(0, length)}`);
  }
  return json({ ok: true });
}

/// Remove a bundle whose listing is already gone.
///
/// Unpublish used to delete only the listing, so bundles published before
/// that was fixed are still downloadable by anyone holding the URL with no
/// endpoint able to remove them: unpublish 404s on the missing listing
/// before it reaches the blob. This is the way to finish that job.
///
/// Gated on an admin login rather than the app's own metadata -- the
/// metadata is exactly what is missing here.
async function purgeBundle(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  const admins = (env.KRATE_ADMINS || "").split(",").map((name) => name.trim());
  if (!admins.includes(identity.login)) return text("not found", 404);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  await env.APPS.delete(`app:${hash}`);
  await env.BUNDLES.delete(hash);
  await env.BUNDLES.delete(`shot:${hash}`);
  await env.BUNDLES.delete(`icon:${hash}`);
  for (let length = 8; length <= 32; length += 1) {
    await env.APPS.delete(`alias:${hash.slice(0, length)}`);
  }
  return json({ ok: true, purged: hash });
}

/// Store a small square logo for a published app. Same contract as the
/// screenshot: separate from the bundle, replaceable, author-gated by the
/// same sign-in the publish used.
async function putIcon(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const body = new Uint8Array(await request.arrayBuffer());
  if (body.length === 0 || body.length > 512 * 1024) {
    return text("an icon must be a PNG under 512 KiB", 413);
  }
  if (!(body[0] === 0x89 && body[1] === 0x50 && body[2] === 0x4e && body[3] === 0x47)) {
    return text("that is not a PNG", 422);
  }
  await env.BUNDLES.put(`icon:${hash}`, body, {
    httpMetadata: { contentType: "image/png" },
  });
  return json({ ok: true });
}

async function getIcon(hash, env) {
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const object = await env.BUNDLES.get(`icon:${hash}`);
  if (!object) return text("not found", 404);
  return new Response(object.body, {
    headers: {
      "content-type": "image/png",
      "cache-control": "public, max-age=86400",
    },
  });
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
  // The fifth blob says which door they got: "direct" is the bytes,
  // "landing" is a person being walked to the receive page first -- the
  // number that tells us whether first-click is working.
  const client = classifyClient(request);
  const wantsBytes = url.searchParams.has("dl");
  const landing = wantsBytes
    ? null
    : client.startsWith("mobile-")
      ? "mobile"
      : client === "desktop-browser"
        ? "desktop"
        : null;
  if (env.USAGE) {
    env.USAGE.writeDataPoint({
      blobs: [
        "link",
        "-",
        client,
        "ok",
        landing ? "landing" : "direct",
        new Date().toISOString().slice(0, 10),
      ],
      doubles: [1],
      indexes: [hash],
    });
  }

  // A phone browser gets a small page instead of a file the phone cannot
  // open -- the soft landing from the mobile plan. `?dl=1` still hands the
  // raw bytes to anyone who insists.
  if (landing === "mobile") {
    return await mobileLanding(hash, env);
  }

  // A desktop browser holding this link is a person, not a runtime -- the
  // receiver K-195 is about. Send them to the receive page with the app's
  // full address carried along, so the page shows THEIR app: its shot, its
  // name, and the install-once-then-it-opens walk. The page's own download
  // button comes back here with `?dl=1`, so the two can never loop, and
  // `krate run <url>` never sees this branch (it does not ask for HTML).
  if (landing === "desktop") {
    return new Response(null, {
      status: 302,
      headers: { location: `https://krate.tech/open/?a=${hash}` },
    });
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
<title>${esc(name)} -- a Krate app</title>
<style>
  body { margin: 0; background: #0a0a0a; color: #fff; font-family: -apple-system, system-ui, sans-serif;
         min-height: 100vh; display: flex; align-items: center; justify-content: center;
         -webkit-font-smoothing: antialiased; }
  main { max-width: 340px; padding: 32px 24px; text-align: center; }
  .card { border: 1px solid #1f2228; border-radius: 16px; background: #0f1012; overflow: hidden;
          margin-bottom: 20px; box-shadow: 0 20px 60px rgba(0,0,0,0.5); }
  .card img { width: 100%; display: block; }
  h1 { font-size: 22px; font-weight: 600; letter-spacing: -0.02em; margin: 0 0 4px; }
  .by { color: rgba(255,255,255,0.45); font-size: 14px; margin: 0 0 18px; }
  p { color: rgba(255,255,255,0.6); font-size: 15px; line-height: 1.55; margin: 0 0 24px; }
  button { width: 100%; padding: 14px; border: 0; border-radius: 999px; background: #fff;
           color: #0a0a0a; font-size: 15.5px; font-weight: 600; margin-bottom: 14px; }
  a { color: rgba(255,255,255,0.5); font-size: 13px; }
</style>
</head>
<body>
<main>
  ${shot ? `<div class="card"><img src="${shot}" alt=""></div>` : ""}
  <h1>${esc(name)}</h1>
  ${author ? `<p class="by">by ${esc(author)}</p>` : ""}
  <p>Someone made this and shared it. It runs on computers today: open this link on your Mac, Windows, or Linux machine, install Krate once (small and free), and it opens -- after showing you what it may touch.</p>
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

async function founding(request, env) {
  let email = "";
  try {
    const body = await request.json();
    email = String(body.email || "").trim().toLowerCase();
  } catch (_) {
    return text("bad request", 400);
  }
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(email) || email.length > 254) {
    return text("that does not look like an email address", 400);
  }
  const key = `founding:${email}`;
  const existing = await env.APPS.get(key);
  if (!existing) {
    await env.APPS.put(key, JSON.stringify({ at: Math.floor(Date.now() / 1000) }));
  }
  return json({ ok: true });
}

async function makeit(request, env) {
  let body;
  try {
    body = await request.json();
  } catch (_) {
    return text("bad request", 400);
  }
  const email = String(body.email || "").trim().toLowerCase();
  const req = String(body.request || "").trim().slice(0, 4000);
  const answers = String(body.answers || "").trim().slice(0, 4000);
  const agent = String(body.agent || "").slice(0, 40);
  const why = String(body.why || "").slice(0, 400);
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(email) || email.length > 254) {
    return text("that does not look like an email address", 400);
  }
  if (!req) return text("say what the app should be", 400);
  const at = Math.floor(Date.now() / 1000);
  await env.APPS.put(
    `makeit:${at}:${crypto.randomUUID().slice(0, 8)}`,
    JSON.stringify({ at, email, request: req, answers, agent, why }),
  );
  return json({ ok: true });
}

async function meta(hash, env) {
  if (/^[0-9a-f]{8,32}$/.test(hash)) {
    const full = await env.APPS.get(`alias:${hash}`);
    if (!full) return text("not found", 404);
    hash = full;
  }
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`app:${hash}`);
  if (!raw) return text("not found", 404);
  let m = {};
  try {
    m = JSON.parse(raw);
  } catch (_) {
    return text("not found", 404);
  }
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const shot = await env.BUNDLES.head(`shot:${hash}`);
  return json({
    id: hash,
    url: `${base}/a/${hash}`,
    shot: shot ? `${base}/shot/${hash}` : null,
    meta: { name: m.name, description: m.description, author: m.author, size: m.size },
  });
}

async function list(env) {
  const listing = await env.APPS.list({ prefix: "app:", limit: 200 });
  const apps = [];
  for (const key of listing.keys) {
    const raw = await env.APPS.get(key.name);
    if (!raw) continue;
    // Unlisted apps have working links and no gallery row.
    try {
      if (JSON.parse(raw).unlisted) continue;
    } catch (_) {
      // An unreadable record should still not hide a listed app.
    }
    const hash = key.name.slice("app:".length);
    const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
    const shot = await env.BUNDLES.head(`shot:${hash}`);
    const icon = await env.BUNDLES.head(`icon:${hash}`);
    apps.push({
      id: hash,
      url: `${base}/a/${hash}`,
      shot: shot ? `${base}/shot/${hash}` : null,
      icon: icon ? `${base}/icon/${hash}` : null,
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

/* ---- shared stores ---------------------------------------------------- */

// Bounds that keep a share a household list, not a database: enough for
// years of shopping lists and meal plans, small enough that one KV value
// holds the whole bucket and sync is one GET.
const SHARE_MAX_KEYS = 512;
const SHARE_MAX_VALUE = 64 * 1024;
const SHARE_MAX_TOTAL = 512 * 1024;

/// The invite code: 10 characters from an alphabet with no 0/O/1/I, so it
/// survives being read aloud across a kitchen. 32^10 is ~10^15 -- unguessable
/// in practice for a rate-limited endpoint, and the code is the only secret.
function shareCode() {
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  const bytes = crypto.getRandomValues(new Uint8Array(10));
  return [...bytes].map((b) => alphabet[b % alphabet.length]).join("");
}

function validShareCode(code) {
  return /^[a-z2-9]{10}$/.test(code);
}

async function shareNew(env) {
  const code = shareCode();
  await env.APPS.put(
    `share:${code}`,
    JSON.stringify({ kv: {}, created: Date.now() }),
  );
  return json({ code });
}

async function shareGet(code, env) {
  if (!validShareCode(code)) return json({ error: "bad code" }, 404);
  const raw = await env.APPS.get(`share:${code}`);
  if (!raw) return json({ error: "no such share" }, 404);
  return new Response(raw, {
    headers: { "content-type": "application/json" },
  });
}

/// Merge one machine's pending writes, last-writer-wins per key by the
/// writer's timestamp. The whole bucket is one KV value: reads are one GET,
/// and concurrent PUTs can race -- for a household list the loser is one
/// item re-added by hand, which is the right price for having no accounts,
/// no locks and no backend anyone maintains.
async function sharePut(request, code, env) {
  if (!validShareCode(code)) return json({ error: "bad code" }, 404);
  const key = `share:${code}`;
  const raw = await env.APPS.get(key);
  if (!raw) return json({ error: "no such share" }, 404);
  let body;
  try {
    body = await request.json();
  } catch {
    return json({ error: "bad body" }, 400);
  }
  const writes = Array.isArray(body.writes) ? body.writes : [];
  const share = JSON.parse(raw);
  share.kv = share.kv || {};
  for (const w of writes) {
    if (typeof w.key !== "string" || w.key.length === 0 || w.key.length > 128) continue;
    const t = Number(w.t) || 0;
    const existing = share.kv[w.key];
    if (existing && Number(existing.t) >= t) continue;
    if (w.v === null || w.v === undefined) {
      // A delete is a tombstone, not an absence: without one, the other
      // machine's next push would resurrect every item ever removed.
      share.kv[w.key] = { v: null, t };
    } else {
      if (typeof w.v !== "string" || w.v.length > SHARE_MAX_VALUE * 1.4) continue;
      share.kv[w.key] = { v: w.v, t };
    }
  }
  const names = Object.keys(share.kv);
  if (names.length > SHARE_MAX_KEYS) {
    return json({ error: "too many keys" }, 413);
  }
  const out = JSON.stringify(share);
  if (out.length > SHARE_MAX_TOTAL) {
    return json({ error: "share too large" }, 413);
  }
  await env.APPS.put(key, out);
  return new Response(out, {
    headers: { "content-type": "application/json" },
  });
}

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
  // Indexed per user so support can revoke a person's sessions.
  await env.APPS.put(`usess:${userId}:${token}`, "1", { expirationTtl: 90 * 24 * 3600 });
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
  headers.set("access-control-allow-methods", "GET, POST, DELETE, OPTIONS");
  headers.set(
    "access-control-allow-headers",
    "authorization, content-type, x-krate-name, x-krate-description, x-krate-category, x-krate-session, x-krate-version, x-krate-os, x-krate-note, x-krate-unlisted",
  );
  return new Response(response.body, { status: response.status, headers });
}


// One live game room: up to two players, everything relayed to the other.
//
// The room holds no game state and never inspects a message -- the players'
// machines own the game, the room owns only the pipe. That keeps it correct
// for any game an app invents, and keeps this code too small to be wrong.
export class Room {
  constructor(state) {
    this.state = state;
    this.sessions = [];
  }

  async fetch(request) {
    if (request.headers.get("Upgrade") !== "websocket") {
      return new Response("this endpoint speaks WebSocket", { status: 426 });
    }
    if (this.sessions.length >= 2) {
      return new Response("room is full", { status: 409 });
    }
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    server.accept();
    const player = this.sessions.length === 0 ? 1 : 2;
    const session = { socket: server, player };
    this.sessions.push(session);

    // Tell the joiner who they are, and both sides who is present.
    server.send(JSON.stringify({ t: "role", p: player, peers: this.sessions.length - 1 }));
    for (const other of this.sessions) {
      if (other !== session) {
        other.socket.send(JSON.stringify({ t: "peer-joined", p: player }));
      }
    }

    server.addEventListener("message", (event) => {
      // Relay only; bounded so one player cannot balloon the other's memory.
      if (typeof event.data === "string" && event.data.length > 8192) return;
      for (const other of this.sessions) {
        if (other !== session) {
          try {
            other.socket.send(event.data);
          } catch (e) {}
        }
      }
    });

    const drop = () => {
      this.sessions = this.sessions.filter((s) => s !== session);
      for (const other of this.sessions) {
        try {
          other.socket.send(JSON.stringify({ t: "peer-gone", p: player }));
        } catch (e) {}
      }
    };
    server.addEventListener("close", drop);
    server.addEventListener("error", drop);

    return new Response(null, { status: 101, webSocket: client });
  }
}

// ===================================================================== paid
//
// The paid plan, end to end. Stripe is the processor; the worker never sees
// a card. Entitlements live in KV (`ent:{userId}`) and are written ONLY by
// verified Stripe webhooks or an admin override -- the client can ask, never
// tell. Everything is gated on the Stripe secrets existing, so this whole
// surface is dormant until they are set and the studio's limit stays soft.

function billingLive(env) {
  return Boolean(env.STRIPE_SECRET_KEY && env.STRIPE_PRICE_MONTHLY && env.STRIPE_PRICE_YEARLY);
}

const FOUNDING_SEATS = 200;

async function foundingSold(env) {
  return parseInt((await env.APPS.get("foundingSold")) || "0", 10);
}

async function billingConfig(env) {
  const sold = await foundingSold(env);
  return json({
    live: billingLive(env),
    // The founding door closes itself the moment the 200th seat is taken:
    // every buy button keyed on this flag disappears with it.
    founding: Boolean(env.STRIPE_PRICE_FOUNDING) && sold < FOUNDING_SEATS,
    prices: { monthly: "$12/month", yearly: "$96/year", founding: "$79/year" },
  });
}

/// The signed-in user behind a request: a Krate session or a GitHub token.
async function authedUser(request, env) {
  const auth = request.headers.get("authorization") || "";
  const token = auth.replace(/^Bearer\s+/i, "").trim();
  if (!token) return null;
  if (token.startsWith("krs_")) {
    const userId = await env.APPS.get(`session:${token}`);
    if (!userId) return null;
    return JSON.parse((await env.APPS.get(`user:${userId}`)) || "null");
  }
  const gh = await fetch("https://api.github.com/user", {
    headers: {
      authorization: `Bearer ${token}`,
      accept: "application/vnd.github+json",
      "user-agent": "krate-hub",
    },
  });
  if (!gh.ok) return null;
  const p = await gh.json();
  return ensureUser(env, "github", String(p.id), {
    login: p.login,
    name: p.name || "",
    avatar_url: p.avatar_url || "",
    email: (p.email || "").toLowerCase() || undefined,
  });
}

function planForPrice(env, priceId) {
  if (priceId === env.STRIPE_PRICE_MONTHLY) return "monthly";
  if (priceId === env.STRIPE_PRICE_YEARLY) return "yearly";
  if (priceId === env.STRIPE_PRICE_FOUNDING) return "founding";
  return "unknown";
}

async function stripe(env, path, form) {
  const response = await fetch(`https://api.stripe.com/v1/${path}`, {
    method: form ? "POST" : "GET",
    headers: {
      authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
      ...(form ? { "content-type": "application/x-www-form-urlencoded" } : {}),
    },
    body: form ? new URLSearchParams(form) : undefined,
  });
  const body = await response.json();
  if (!response.ok) {
    throw new Error((body.error && body.error.message) || `stripe ${path} failed`);
  }
  return body;
}

async function billingCheckout(request, env) {
  if (!billingLive(env)) return text("Billing is not open yet.", 503);
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const { plan } = await request.json().catch(() => ({}));
  if (plan === "founding" && (await foundingSold(env)) >= FOUNDING_SEATS) {
    return text("The founding 200 is full. The monthly and yearly plans are open.", 409);
  }
  const price =
    plan === "monthly" ? env.STRIPE_PRICE_MONTHLY
    : plan === "yearly" ? env.STRIPE_PRICE_YEARLY
    : plan === "founding" ? env.STRIPE_PRICE_FOUNDING
    : null;
  if (!price) return text("Unknown plan.", 400);
  try {
    const session = await stripe(env, "checkout/sessions", {
      mode: "subscription",
      "line_items[0][price]": price,
      "line_items[0][quantity]": "1",
      client_reference_id: user.id,
      ...(user.email ? { customer_email: user.email } : {}),
      "subscription_data[metadata][krate_user]": user.id,
      allow_promotion_codes: "true",
      success_url: "https://krate.tech/billing/done/",
      cancel_url: "https://krate.tech/studio/#pricing",
    });
    return json({ url: session.url });
  } catch (err) {
    return text(`Checkout could not start: ${err.message}`, 502);
  }
}

/// Verify a Stripe webhook signature: HMAC-SHA256 of `t.payload`.
async function stripeSignatureValid(request, payload, env) {
  const header = request.headers.get("stripe-signature") || "";
  const parts = Object.fromEntries(
    header.split(",").map((p) => p.split("=")).filter((p) => p.length === 2),
  );
  if (!parts.t || !parts.v1) return false;
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(env.STRIPE_WEBHOOK_SECRET || ""),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign(
    "HMAC",
    key,
    new TextEncoder().encode(`${parts.t}.${payload}`),
  );
  const hex = [...new Uint8Array(mac)].map((b) => b.toString(16).padStart(2, "0")).join("");
  return hex === parts.v1;
}

async function writeEntitlementFromSubscription(env, sub) {
  const userId =
    (sub.metadata && sub.metadata.krate_user) || (await env.APPS.get(`cust:${sub.customer}`));
  if (!userId) return;
  const priceId =
    sub.items && sub.items.data && sub.items.data[0] ? sub.items.data[0].price.id : "";
  await env.APPS.put(`cust:${sub.customer}`, userId);
  // The founding seat count follows real transitions: taken when a founding
  // subscription becomes active, freed when that subscription ends. The evt
  // replay guard upstream keeps retries from double-counting.
  {
    const plan = planForPrice(env, priceId);
    const active = sub.status === "active" || sub.status === "trialing";
    const prior = JSON.parse((await env.APPS.get(`ent:${userId}`)) || "null");
    const priorFounding = Boolean(prior && prior.plan === "founding" && entitlementActive(prior));
    const nowFounding = plan === "founding" && active;
    if (nowFounding && !priorFounding) {
      await env.APPS.put("foundingSold", String((await foundingSold(env)) + 1));
    } else if (priorFounding && !nowFounding) {
      await env.APPS.put("foundingSold", String(Math.max(0, (await foundingSold(env)) - 1)));
    }
  }
  await env.APPS.put(
    `ent:${userId}`,
    JSON.stringify({
      plan: planForPrice(env, priceId),
      status: sub.status,
      until: (sub.current_period_end || 0) * 1000,
      sub: sub.id,
      customer: sub.customer,
      at: Date.now(),
    }),
  );
}

async function billingWebhook(request, env) {
  const payload = await request.text();
  if (!env.STRIPE_WEBHOOK_SECRET || !(await stripeSignatureValid(request, payload, env))) {
    return text("bad signature", 400);
  }
  let event;
  try {
    event = JSON.parse(payload);
  } catch (_) {
    return text("bad payload", 400);
  }
  // Stripe retries; process each event once.
  if (await env.APPS.get(`evt:${event.id}`)) return json({ ok: true });
  await env.APPS.put(`evt:${event.id}`, "1", { expirationTtl: 7 * 24 * 3600 });

  const obj = event.data && event.data.object;
  if (event.type === "checkout.session.completed" && obj) {
    if (obj.client_reference_id && obj.customer) {
      await env.APPS.put(`cust:${obj.customer}`, obj.client_reference_id);
    }
    if (obj.subscription) {
      try {
        const sub = await stripe(env, `subscriptions/${obj.subscription}`);
        await writeEntitlementFromSubscription(env, sub);
      } catch (_) { /* the subscription.updated event will carry it */ }
    }
  } else if (
    (event.type === "customer.subscription.updated"
      || event.type === "customer.subscription.deleted"
      || event.type === "customer.subscription.created") && obj
  ) {
    await writeEntitlementFromSubscription(env, obj);
  } else if (event.type === "invoice.payment_succeeded" && obj) {
    const userId = await env.APPS.get(`cust:${obj.customer}`);
    await env.APPS.put(
      `pay:${Date.now()}:${crypto.randomUUID().slice(0, 8)}`,
      JSON.stringify({
        userId: userId || "",
        email: obj.customer_email || "",
        amount: obj.amount_paid,
        currency: obj.currency,
        invoice: obj.id,
        at: Date.now(),
      }),
    );
  }
  return json({ ok: true });
}


// ================================================================ free tier

async function planCount(request, env, increment) {
  const body = await request.json().catch(() => ({}));
  const device = String(body.device || "");
  const user = await authedUser(request, env);

  // Two keys, and an allowance is spent if EITHER says so.
  //
  // The account is the primary one: makes belong to a person, and follow
  // them between machines. But an account is free to create, so the
  // account alone is not a limit -- it is an invitation to make another
  // email. The device hash is the second key precisely because it does not
  // change when the account does.
  //
  // Neither is airtight on its own and neither needs to be. Together they
  // mean getting a fourth free app costs either a new machine or real
  // effort, which is the honest bar for a free tier: enough friction that
  // it is easier to pay $12 than to cheat.
  const keys = [];
  if (user) keys.push(`mkacct:${user.id}`);
  if (/^[0-9a-f]{64}$/.test(device)) keys.push(`mkdev:${device}`);
  if (!keys.length) {
    return text("Sign in first, or send a device id.", 400);
  }

  // No month in the key. Three EVER, per Yashraj's ruling (2026-09-01):
  // the words must not promise a reset the wall will not honour.
  const counts = await Promise.all(keys.map((k) => env.APPS.get(k)));
  let n = counts.reduce((most, raw) => Math.max(most, parseInt(raw || "0", 10)), 0);

  // A device that made apps offline reports the higher local number; the
  // mirror never goes backward, so an offline make is still counted.
  const local = Number.isFinite(body.n) ? Math.max(0, Math.floor(body.n)) : 0;
  n = Math.max(n, local);
  if (increment) n = n + 1;

  if (increment) {
    // Both keys carry the same number, so removing either one does not
    // hand back an allowance. No TTL: an expiry is a monthly reset by
    // another name.
    await Promise.all(keys.map((k) => env.APPS.put(k, String(n))));
    // The old per-month key is left alone. It expires on its own, and
    // deleting it would give anyone mid-month a silent extra make.
  }
  return json({ n, keys: keys.length });
}

// ================================================================ account
//
// One profile call for the website's account page and the studio: who you
// are, what plan you are on, how your referrals stand. Referrals: send
// three people who actually sign up and you get a month of Studio on us.

async function getOrMakeRefCode(env, user) {
  if (user.refcode) return user.refcode;
  const code = crypto.randomUUID().replaceAll("-", "").slice(0, 8);
  user.refcode = code;
  await env.APPS.put(`user:${user.id}`, JSON.stringify(user));
  await env.APPS.put(`rcode:${code}`, user.id);
  return code;
}

async function meProfile(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const ent = JSON.parse((await env.APPS.get(`ent:${user.id}`)) || "null");
  const refs = JSON.parse((await env.APPS.get(`refs:${user.id}`)) || "[]");
  const awards = parseInt((await env.APPS.get(`refawards:${user.id}`)) || "0", 10);
  return json({
    user: {
      name: user.name, login: user.login, email: user.email,
      avatar_url: user.avatar_url, created: user.created,
    },
    plan: {
      plan: ent ? ent.plan : "free",
      active: entitlementActive(ent),
      until: ent ? ent.until : 0,
      via: (ent && ent.via) || "",
      portal: Boolean(ent && ent.customer),
    },
    referral: { code: await getOrMakeRefCode(env, user), count: refs.length, awards },
  });
}

async function myApps(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const listing = await env.APPS.list({ prefix: "app:", limit: 1000 });
  const out = [];
  for (const k of listing.keys) {
    const a = JSON.parse((await env.APPS.get(k.name)) || "null");
    if (a && a.author_login && a.author_login === user.login) {
      out.push({
        hash: k.name.slice("app:".length),
        name: a.name, description: a.description,
        published: a.published, size: a.size, unlisted: a.unlisted,
      });
    }
  }
  out.sort((a, b) => b.published - a.published);
  return json({ apps: out.slice(0, 50) });
}

// The claim comes from the NEW person's browser right after their first
// sign-in; the credit lands on the referrer. Only a genuinely fresh
// account counts, so codes cannot be farmed by re-claiming old sign-ins.
async function referralClaim(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const body = await request.json().catch(() => ({}));
  const code = String(body.code || "").trim().toLowerCase();
  if (!/^[a-z0-9]{6,12}$/.test(code)) return text("No such code.", 400);
  const ownerId = await env.APPS.get(`rcode:${code}`);
  if (!ownerId || ownerId === user.id) return text("No such code.", 400);
  if (await env.APPS.get(`refby:${user.id}`)) return json({ ok: true, already: true });
  if (Date.now() - (user.created || 0) > 48 * 3600 * 1000) return json({ ok: true, stale: true });
  await env.APPS.put(`refby:${user.id}`, ownerId);
  const refs = JSON.parse((await env.APPS.get(`refs:${ownerId}`)) || "[]");
  if (!refs.includes(user.id)) refs.push(user.id);
  await env.APPS.put(`refs:${ownerId}`, JSON.stringify(refs.slice(0, 500)));
  const awards = parseInt((await env.APPS.get(`refawards:${ownerId}`)) || "0", 10);
  const due = Math.floor(refs.length / 3);
  if (due > awards) {
    // A month of Studio on us, stacked on whatever time is already there.
    // If a paid subscription event later overwrites this record the person
    // is paying anyway; the credit mostly serves free accounts, which is
    // exactly who referrals recruit.
    await env.APPS.put(`refawards:${ownerId}`, String(due));
    const ent = JSON.parse((await env.APPS.get(`ent:${ownerId}`)) || "null");
    const base = ent && ent.until && ent.until > Date.now() ? ent.until : Date.now();
    await env.APPS.put(`ent:${ownerId}`, JSON.stringify({
      plan: ent && entitlementActive(ent) && ent.via !== "referral" ? ent.plan : "studio",
      status: "active",
      until: base + (due - awards) * 30 * 24 * 3600 * 1000,
      via: "referral",
      sub: (ent && ent.sub) || "",
      customer: (ent && ent.customer) || "",
      at: Date.now(),
    }));
  }
  return json({ ok: true, counted: true });
}

// Stripe's own customer portal: change card, switch plan, cancel. We never
// touch any of it; the person manages their money on stripe.com.
async function billingPortal(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const ent = JSON.parse((await env.APPS.get(`ent:${user.id}`)) || "null");
  if (!ent || !ent.customer) return text("No subscription on this account yet.", 404);
  const session = await stripe(env, "billing_portal/sessions", {
    customer: ent.customer,
    return_url: "https://krate.tech/account/",
  });
  return json({ url: session.url });
}

function entitlementActive(ent) {
  if (!ent) return false;
  if (ent.plan === "comp") return true;
  const okStatus = ent.status === "active" || ent.status === "trialing";
  const grace = 3 * 24 * 3600 * 1000;
  return okStatus && (!ent.until || ent.until + grace > Date.now());
}

async function billingStatus(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const ent = JSON.parse((await env.APPS.get(`ent:${user.id}`)) || "null");
  return json({
    live: billingLive(env),
    plan: ent ? ent.plan : "free",
    active: entitlementActive(ent),
    until: ent ? ent.until : 0,
  });
}

// ================================================================== support
//
// Tickets with real conversations. A ticket is one KV record carrying its
// whole thread. A signed-in person's tickets are indexed by user; a
// signed-out person keeps a per-ticket secret key instead, so support works
// before login does. Admin replies land in the same thread and show up in
// the studio.

async function supportNew(request, env) {
  const body = await request.json().catch(() => ({}));
  const user = await authedUser(request, env);
  const email = user ? user.email : String(body.email || "").trim().toLowerCase();
  const subject = String(body.subject || "").trim().slice(0, 140);
  const first = String(body.text || "").trim().slice(0, 4000);
  if (!subject || !first) return text("Say what it is about, and what happened.", 400);
  if (!user && !/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(email)) {
    return text("An email address, so the answer can reach you.", 400);
  }
  const id = crypto.randomUUID().slice(0, 8);
  const key = crypto.randomUUID().replaceAll("-", "");
  // Paying people are promised the front of the queue; the stamp travels
  // with the ticket so the desk can honor that without a lookup per row.
  const ent = user ? JSON.parse((await env.APPS.get(`ent:${user.id}`)) || "null") : null;
  const ticket = {
    id,
    key,
    userId: user ? user.id : "",
    login: user ? user.login : "",
    email,
    subject,
    priority: entitlementActive(ent),
    status: "open",
    created: Date.now(),
    updated: Date.now(),
    messages: [{ who: "user", text: first, at: Date.now() }],
  };
  await env.APPS.put(`tick:${id}`, JSON.stringify(ticket));
  if (user) {
    const ids = JSON.parse((await env.APPS.get(`utick:${user.id}`)) || "[]");
    ids.unshift(id);
    await env.APPS.put(`utick:${user.id}`, JSON.stringify(ids.slice(0, 100)));
  }
  return json({ id, key });
}

function publicTicket(t) {
  const { key, userId, ...rest } = t;
  return rest;
}

async function supportList(request, env) {
  const body = await request.json().catch(() => ({}));
  const user = await authedUser(request, env);
  const out = [];
  if (user) {
    const ids = JSON.parse((await env.APPS.get(`utick:${user.id}`)) || "[]");
    for (const id of ids) {
      const t = JSON.parse((await env.APPS.get(`tick:${id}`)) || "null");
      if (t) out.push(publicTicket(t));
    }
  }
  for (const ref of Array.isArray(body.keys) ? body.keys.slice(0, 20) : []) {
    const t = JSON.parse((await env.APPS.get(`tick:${ref.id}`)) || "null");
    if (t && t.key === ref.key && !out.some((x) => x.id === t.id)) out.push(publicTicket(t));
  }
  out.sort((a, b) => b.updated - a.updated);
  return json({ tickets: out });
}

async function supportReply(request, env) {
  const body = await request.json().catch(() => ({}));
  const t = JSON.parse((await env.APPS.get(`tick:${body.id}`)) || "null");
  if (!t) return text("No such ticket.", 404);
  const user = await authedUser(request, env);
  const owns = (user && user.id === t.userId) || (body.key && body.key === t.key);
  if (!owns) return text("Not your ticket.", 403);
  const line = String(body.text || "").trim().slice(0, 4000);
  if (!line) return text("Say something.", 400);
  t.messages.push({ who: "user", text: line, at: Date.now() });
  t.status = "open";
  t.updated = Date.now();
  await env.APPS.put(`tick:${t.id}`, JSON.stringify(t));
  return json({ ok: true });
}

// ==================================================================== admin
//
// The support desk: one page, the same verified-GitHub-login gate as
// /admin/reports (KRATE_ADMINS). Users, plans, payments, tickets, the
// make-it-for-me queue, and session revocation.

async function adminApi(request, pathname, env) {
  const admin = await isAdmin(request, env);
  if (!admin) return text("not found", 404);
  const route = pathname.slice("/admin/api/".length);
  const body = request.method === "POST" ? await request.json().catch(() => ({})) : {};

  if (route === "overview") {
    const users = await env.APPS.list({ prefix: "user:", limit: 1000 });
    const ticks = await env.APPS.list({ prefix: "tick:", limit: 1000 });
    const pays = await env.APPS.list({ prefix: "pay:", limit: 1000 });
    return json({
      users: users.keys.length,
      tickets: ticks.keys.length,
      payments: pays.keys.length,
      billing_live: billingLive(env),
    });
  }
  if (route === "users") {
    const q = (new URL(request.url).searchParams.get("q") || "").toLowerCase();
    const listing = await env.APPS.list({ prefix: "user:", limit: 1000 });
    const out = [];
    for (const k of listing.keys) {
      const u = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (!u) continue;
      const hay = `${u.email} ${u.login} ${u.name}`.toLowerCase();
      if (q && !hay.includes(q)) continue;
      const ent = JSON.parse((await env.APPS.get(`ent:${u.id}`)) || "null");
      out.push({ ...u, ent, active: entitlementActive(ent) });
      if (out.length >= 100) break;
    }
    return json({ users: out });
  }
  if (route === "user/plan" && request.method === "POST") {
    // The support override: comp a user, or clear an override.
    if (body.plan === "comp") {
      await env.APPS.put(`ent:${body.id}`, JSON.stringify({ plan: "comp", status: "active", until: 0, at: Date.now(), by: admin.login }));
    } else {
      await env.APPS.delete(`ent:${body.id}`);
    }
    return json({ ok: true });
  }
  if (route === "user/logout" && request.method === "POST") {
    const sess = await env.APPS.list({ prefix: `usess:${body.id}:`, limit: 1000 });
    for (const k of sess.keys) {
      const token = k.name.split(":").pop();
      await env.APPS.delete(`session:${token}`);
      await env.APPS.delete(k.name);
    }
    return json({ revoked: sess.keys.length });
  }
  if (route === "tickets") {
    const listing = await env.APPS.list({ prefix: "tick:", limit: 1000 });
    const out = [];
    for (const k of listing.keys) {
      const t = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (t) out.push(t);
    }
    // Open before closed, paying people first within open, newest first.
    out.sort((a, b) =>
      (a.status === "open" ? 0 : 1) - (b.status === "open" ? 0 : 1)
      || (b.priority ? 1 : 0) - (a.priority ? 1 : 0)
      || b.updated - a.updated);
    return json({ tickets: out });
  }
  if (route === "ticket/reply" && request.method === "POST") {
    const t = JSON.parse((await env.APPS.get(`tick:${body.id}`)) || "null");
    if (!t) return text("no such ticket", 404);
    t.messages.push({ who: "krate", text: String(body.text || "").slice(0, 4000), at: Date.now() });
    t.updated = Date.now();
    await env.APPS.put(`tick:${t.id}`, JSON.stringify(t));
    return json({ ok: true });
  }
  if (route === "ticket/status" && request.method === "POST") {
    const t = JSON.parse((await env.APPS.get(`tick:${body.id}`)) || "null");
    if (!t) return text("no such ticket", 404);
    t.status = body.status === "closed" ? "closed" : "open";
    t.updated = Date.now();
    await env.APPS.put(`tick:${t.id}`, JSON.stringify(t));
    return json({ ok: true });
  }
  if (route === "payments") {
    const listing = await env.APPS.list({ prefix: "pay:", limit: 1000 });
    const out = [];
    for (const k of listing.keys.reverse()) {
      const p = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (p) out.push(p);
    }
    return json({ payments: out });
  }
  if (route === "makeit") {
    const listing = await env.APPS.list({ prefix: "makeit:", limit: 1000 });
    const out = [];
    for (const k of listing.keys.reverse()) {
      const m = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (m) out.push({ key: k.name, ...m });
    }
    return json({ requests: out });
  }
  if (route === "makeit/done" && request.method === "POST") {
    await env.APPS.delete(String(body.key || ""));
    return json({ ok: true });
  }
  return text("not found", 404);
}

/// The admin desk page. One file, no build step; signs in through the same
/// krate.tech login the product uses and talks to /admin/api/* with the
/// token. Renders nothing for non-admins because the APIs 404.
function adminPage() {
  const html = `<!doctype html><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Krate desk</title>
<style>
:root{--bg:#0b0e15;--panel:#12151d;--line:#252a36;--ink:#f2f5fa;--mut:#9aa3b5;--acc:#6291ff;--ok:#39d98a;--warn:#ffb082}
*{margin:0;padding:0;box-sizing:border-box}body{background:var(--bg);color:var(--ink);font:14px/1.5 -apple-system,system-ui,sans-serif;padding:24px}
h1{font-size:20px;margin-bottom:4px}.sub{color:var(--mut);font-size:12.5px;margin-bottom:18px}
nav{display:flex;gap:8px;margin-bottom:18px;flex-wrap:wrap}
nav button{background:none;border:1px solid var(--line);color:var(--mut);padding:7px 14px;border-radius:99px;cursor:pointer;font:inherit;font-size:13px}
nav button.on{background:var(--ink);color:#0b0e15;border-color:var(--ink)}
.card{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:14px 16px;margin-bottom:10px}
.row{display:flex;gap:12px;align-items:center;flex-wrap:wrap}
.mut{color:var(--mut);font-size:12.5px}.grow{flex:1}
button.act{background:var(--acc);border:none;color:#fff;padding:6px 13px;border-radius:8px;cursor:pointer;font:inherit;font-size:12.5px}
button.ghost{background:none;border:1px solid var(--line);color:var(--mut);padding:6px 13px;border-radius:8px;cursor:pointer;font:inherit;font-size:12.5px}
input,textarea{background:#0e1118;border:1px solid var(--line);color:var(--ink);border-radius:8px;padding:8px 11px;font:inherit;width:100%}
textarea{min-height:70px;resize:vertical}
.msg{padding:8px 12px;border-radius:10px;margin:6px 0;max-width:640px;white-space:pre-wrap}
.msg.user{background:#1a2233}.msg.krate{background:#15251c;margin-left:32px}
.pill{font-size:11px;border:1px solid var(--line);border-radius:99px;padding:2px 9px;color:var(--mut)}
.pill.open{color:var(--warn);border-color:#5a4636}.pill.active{color:var(--ok);border-color:#2c5243}
#login{max-width:420px}
</style>
<h1>Krate desk</h1><p class="sub" id="who">signing in&hellip;</p>
<div id="login" class="card" style="display:none">
  <p style="margin-bottom:10px">Sign in with the same account Studio uses. Admins only; everyone else sees nothing.</p>
  <button class="act" onclick="location='https://krate.tech/login/?next=admin'">Sign in</button>
</div>
<div id="app" style="display:none">
<nav>
  <button data-t="tickets" class="on">Tickets</button>
  <button data-t="users">Users</button>
  <button data-t="payments">Payments</button>
  <button data-t="makeit">Make-it queue</button>
</nav>
<div id="view"></div>
</div>
<script>
const HUB=location.origin;
let TOKEN=localStorage.getItem("desk_token")||"";
const frag=new URLSearchParams(location.hash.slice(1));
if(frag.get("token")){TOKEN=frag.get("token");localStorage.setItem("desk_token",TOKEN);history.replaceState(null,"",location.pathname);}
const api=(p,opt)=>fetch(HUB+"/admin/api/"+p,{...opt,headers:{authorization:"Bearer "+TOKEN,"content-type":"application/json",...(opt&&opt.headers)}}).then(r=>{if(!r.ok)throw new Error(r.status);return r.json()});
const el=s=>document.querySelector(s);
const esc=s=>String(s??"").replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
const when=ms=>new Date(ms).toLocaleString();
async function boot(){
  try{const o=await api("overview");el("#who").textContent=\`\${o.users} users · \${o.tickets} tickets · \${o.payments} payments · billing \${o.billing_live?"LIVE":"not configured"}\`;el("#app").style.display="";show("tickets");}
  catch(e){el("#who").textContent="not signed in, or not an admin";el("#login").style.display="";}
}
document.querySelectorAll("nav button").forEach(b=>b.onclick=()=>{document.querySelectorAll("nav button").forEach(x=>x.classList.remove("on"));b.classList.add("on");show(b.dataset.t);});
async function show(tab){
  const v=el("#view");v.innerHTML='<p class="mut">loading…</p>';
  if(tab==="tickets"){
    const {tickets}=await api("tickets");
    v.innerHTML=tickets.length?"":'<p class="mut">No tickets.</p>';
    for(const t of tickets){
      const d=document.createElement("div");d.className="card";
      d.innerHTML=\`<div class="row"><b>\${esc(t.subject)}</b>\${t.priority?'<span class="pill" style="border-color:#5a4a17;background:#2b2410;color:#e7c766">priority</span>':''}<span class="pill \${t.status}">\${t.status}</span><span class="grow"></span><span class="mut">\${esc(t.login||t.email)} · \${when(t.updated)}</span></div>
      <div>\${t.messages.map(m=>\`<div class="msg \${m.who}">\${esc(m.text)}<div class="mut">\${m.who==="krate"?"you":"them"} · \${when(m.at)}</div></div>\`).join("")}</div>
      <div class="row" style="margin-top:8px"><textarea placeholder="Reply…"></textarea></div>
      <div class="row" style="margin-top:8px"><button class="act">Send reply</button><button class="ghost">\${t.status==="open"?"Close":"Reopen"}</button></div>\`;
      d.querySelector(".act").onclick=async()=>{const x=d.querySelector("textarea").value.trim();if(!x)return;await api("ticket/reply",{method:"POST",body:JSON.stringify({id:t.id,text:x})});show("tickets");};
      d.querySelector(".ghost").onclick=async()=>{await api("ticket/status",{method:"POST",body:JSON.stringify({id:t.id,status:t.status==="open"?"closed":"open"})});show("tickets");};
      v.appendChild(d);
    }
  }
  if(tab==="users"){
    v.innerHTML='<div class="card row"><input id="q" placeholder="Search email, login, name…"><button class="act" id="go">Search</button></div><div id="ulist"></div>';
    const load=async()=>{
      const {users}=await api("users?q="+encodeURIComponent(el("#q").value||""));
      const u=el("#ulist");u.innerHTML=users.length?"":'<p class="mut">Nobody matches.</p>';
      for(const x of users){
        const d=document.createElement("div");d.className="card row";
        d.innerHTML=\`<div class="grow"><b>\${esc(x.name||x.login||x.email)}</b> <span class="mut">\${esc(x.email)} · \${(x.providers||[]).join(", ")} · joined \${when(x.created)}</span></div>
        <span class="pill \${x.active?"active":""}">\${x.ent?x.ent.plan:"free"}</span>
        <button class="ghost" data-a="plan">\${x.ent&&x.ent.plan==="comp"?"Remove comp":"Comp plan"}</button>
        <button class="ghost" data-a="out">Sign out everywhere</button>\`;
        d.querySelector('[data-a=plan]').onclick=async()=>{await api("user/plan",{method:"POST",body:JSON.stringify({id:x.id,plan:x.ent&&x.ent.plan==="comp"?"none":"comp"})});load();};
        d.querySelector('[data-a=out]').onclick=async()=>{const r=await api("user/logout",{method:"POST",body:JSON.stringify({id:x.id})});alert(r.revoked+" sessions revoked");};
        u.appendChild(d);
      }
    };
    el("#go").onclick=load;el("#q").onkeydown=e=>{if(e.key==="Enter")load()};load();
  }
  if(tab==="payments"){
    const {payments}=await api("payments");
    v.innerHTML=payments.length?"":'<p class="mut">No payments yet.</p>';
    for(const p of payments){
      const d=document.createElement("div");d.className="card row";
      d.innerHTML=\`<b>\${(p.amount/100).toFixed(2)} \${esc((p.currency||"usd").toUpperCase())}</b><span class="mut grow">\${esc(p.email||p.userId)} · \${when(p.at)} · \${esc(p.invoice)}</span>\`;
      v.appendChild(d);
    }
  }
  if(tab==="makeit"){
    const {requests}=await api("makeit");
    v.innerHTML=requests.length?"":'<p class="mut">Queue is empty.</p>';
    for(const m of requests){
      const d=document.createElement("div");d.className="card";
      d.innerHTML=\`<div class="row"><b>\${esc(m.email)}</b><span class="grow"></span><span class="mut">\${when(m.at*1000)}</span><button class="ghost">Done</button></div>
      <p style="margin-top:6px">\${esc(m.request)}</p>\${m.answers?\`<p class="mut" style="margin-top:4px">answers: \${esc(m.answers)}</p>\`:""}\`;
      d.querySelector("button").onclick=async()=>{await api("makeit/done",{method:"POST",body:JSON.stringify({key:m.key})});show("makeit");};
      v.appendChild(d);
    }
  }
}
boot();
</script>`;
  return new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } });
}
