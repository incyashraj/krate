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

/// A support report over this is refused, and refused on its declared
/// length before a byte of it is read (K-767). Larger than a bundle
/// because a report carries the session log and a screenshot beside the
/// app, and those are the parts that make it worth reading.
const MAX_REPORT_BYTES = 12 * 1024 * 1024;

// The bundle validator, compiled from crates/bundle (IC-833, K-309). In
// the Worker the import is a WebAssembly.Module (wrangler's CompiledWasm
// rule); under Node's ESM integration it is the instantiated exports.
// Either way it is instantiated once, lazily, and the instance's memory
// is re-read on every call because it can grow.
import * as validatorImport from "./validator.wasm";
import { Org, SLUG, EMAIL, chainOk, messageFor } from "./org.js";
// Organisation lifecycle, billing, mail and the refused-attempt limit
// (IC-948), each in its own module; importing the store's half registers
// its commands with the Org class.
import "./org-lifecycle-store.js";
import { orgLifecycleRoute, sweepOrgs } from "./org-lifecycle.js";
import { orgBillingRoute, orgBillingEvent, orgBillingAlerts } from "./org-billing.js";
import { mailInvite } from "./org-mail.js";
import { refusalsExhausted, noteRefusal, isRefusal, rateLimitedAnswer } from "./org-limits.js";
// Company sign-in and offline access for organisations (IC-948, F and G).
import { ssoRoute, oidcStart, oidcCallback, ssoSlugsFor, ssoMarks } from "./org-sso.js";
import { leaseRoute, leaseKeyRoute } from "./org-lease.js";
// Release control and groups (IC-949, R). After org-lease.js: it guards
// the lease command too.
import { orgReleaseRoute } from "./org-releases.js";
// What an approver sees and must confirm (IC-950); after release control.
import { judgementCaps } from "./org-review.js";
// Company connections (IC-952). After org-releases.js, whose checks a
// credential fetch reuses.
import { connectionRoute, connectionsAsked } from "./org-connections.js";
// SCIM 2.0 provisioning (IC-955); after release control, whose groups it fills.
import { scimRoute, orgScimRoute } from "./org-scim.js";
// Audit export over a signed webhook (IC-955).
import { auditExportRoute } from "./org-audit-export.js";
// Company sign-in by SAML 2.0 (IC-955), beside OIDC.
import { orgSamlRoute, samlStart, samlAcs, samlMetadata } from "./org-saml.js";
// The organisation keyring (F-316) and the apps' team data sealed with it
// (IC-951, store.org).
import { orgKeysRoute } from "./org-keys.js";
// Signing nobody has to think about (IC-958): the signing ring, publishers
// approved in the background, solo release.
import { orgSigningRoute } from "./org-signing.js";
import { orgDataRoute } from "./org-data.js";
// Inventory, provenance and ownership (IC-954), after release control and review.
import { inventoryRoute, provenanceArg } from "./org-inventory.js";
// The browser Player (IC-956): the web pack made at publish, the route that
// serves it behind /a/'s own checks, and the cross-origin-isolated page.
import { playerPage, playerAsset, validatorFile, webPackRoute, ensureWebPack } from "./web-player.js";
// Insights (IC-959), after everything whose counts it reads.
import { insightsRoute } from "./org-insights.js";
// The organisation's own day (F-319): its time zone, set by an owner or admin.
import { orgTimeRoute } from "./org-time.js";
// The organisation's logo, kept small and only as a picture.
import { orgBrandRoute } from "./org-brand.js";
// Declining a version with a reason, release notes, app requests and
// notifications: after everything whose commands they wrap.
import { orgDeclineRoute } from "./org-decline.js";
import { orgNotesRoute, noteHeader } from "./org-notes.js";
import { orgRequestsRoute } from "./org-requests.js";
import { orgNotifyRoute, sendNotices } from "./org-notify.js";
// Signed-in sessions, and signing out everywhere.
import { accountSessionsRoute, recordSession, sessionLabel, endEverySession, GITHUB_INDEX_TTL_SECONDS } from "./account-sessions.js";
// The organisation store (IC-946), one SQLite-backed Durable Object per
// organisation; wrangler.toml binds it as ORGS.
export { Org };
let validatorExports = null;
function validator() {
  if (!validatorExports) {
    validatorExports =
      validatorImport.default instanceof WebAssembly.Module
        ? new WebAssembly.Instance(validatorImport.default, {}).exports
        : validatorImport;
  }
  return validatorExports;
}

/// Judge an upload with the Rust validator. Returns the parsed answer:
/// `{ ok: true, judgement }` or `{ ok: false, problem }`.
function judgeBundle(bytes) {
  return callValidator("krate_judge", bytes);
}

/// Read a revocation list written by `krate revoke`, with the Rust
/// validator: `{ ok: true, root, issued_at, revocations }` once the root
/// it names is checked to have signed it, or `{ ok: false, problem }`
/// (IC-946).
function readRevocations(bytes) {
  return callValidator("krate_revocations", bytes);
}

/// The component of a bundle exactly as the validator judged it, for the
/// web pack (IC-956): `{ ok: true, bytes }` or `{ ok: false, problem }`.
function judgedComponent(bytes) {
  const v = validator();
  const input = v.krate_alloc(bytes.length);
  new Uint8Array(v.memory.buffer, input, bytes.length).set(bytes);
  const out = v.krate_judged_component(input, bytes.length);
  const length = new DataView(v.memory.buffer).getUint32(out, true);
  const body = new Uint8Array(v.memory.buffer, out + 4, length).slice();
  v.krate_free(out, 4 + length);
  v.krate_free(input, bytes.length);
  return body[0] === 1
    ? { ok: true, bytes: body.subarray(1) }
    : { ok: false, problem: new TextDecoder().decode(body.subarray(1)) };
}

function callValidator(name, bytes) {
  const v = validator();
  const input = v.krate_alloc(bytes.length);
  new Uint8Array(v.memory.buffer, input, bytes.length).set(bytes);
  const out = v[name](input, bytes.length);
  const view = new DataView(v.memory.buffer);
  const length = view.getUint32(out, true);
  const json = new TextDecoder().decode(new Uint8Array(v.memory.buffer, out + 4, length));
  v.krate_free(out, 4 + length);
  v.krate_free(input, bytes.length);
  return JSON.parse(json);
}

/// What the archive may declare, judged from the central directory before
/// any entry is inflated (IC-833, test 1833). The same ceilings
/// crates/bundle applies when `krate run` opens the file, so the hub does
/// not admit what the recipient would refuse. Declared sizes are the
/// archive's own claim; they bound what a later reader could be asked to
/// allocate, and the manifest inflate below is bounded on actual bytes.
const MAX_ENTRY_COUNT = 8192;
const MAX_ENTRY_BYTES = 512 * 1024 * 1024;
const MAX_DECLARED_TOTAL_BYTES = 1024 * 1024 * 1024;
const MAX_PATH_DEPTH = 16;
const MAX_PATH_BYTES = 180;

/// How long a verified GitHub identity is trusted without asking GitHub
/// again. Long enough that publishing several apps costs one round trip,
/// short enough that a revoked token stops working the same day.
const IDENTITY_TTL_SECONDS = 6 * 60 * 60;

// ------------------------------------------------------- the anonymous wall
//
// Every write endpoint that takes no sign-in gets a ceiling per caller per
// minute (K-767). Before this there was none anywhere: six rapid POST
// /share/new and five rapid anonymous POST /report all returned 200, never
// 429, and each one makes a KV row or an R2 object. That is an unbounded
// hosting bill and an unbounded pile of junk, reachable by anyone.
//
// Counted in a Durable Object's memory, never in KV (K-912). The
// first cut counted in KV so a test could see it, and that cost one KV
// write per allowed request. The free plan allows 1,000 KV writes a day
// for the whole account, so the busiest bucket -- the CLI's usage beacon,
// 60 a minute per caller -- let ONE machine spend the day's writes in
// seventeen minutes. On 2026-09-27 a day of test runs did exactly that
// (36,246 beacons from one address), and every publish, sign-in and build
// record after it failed with "could not record who is publishing this".
// A counter took down the product it guards: the K-082 disease again.
//
// The count lives in a Durable Object per caller (RateGate), in memory, so
// it costs no KV and no storage and is exact. The tests drive the real
// RateGate class through a stand-in for the namespace.
//
// The trade this accepts: the count is per address, so a flood spread
// across many addresses gets a multiple of the ceiling, and each counted
// request is one Durable Object request (the free plan allows 100,000 a
// day; past that the gate errors and the wall opens). That is fine. This is not a security boundary -- it is a cost ceiling,
// and turning "unbounded" into "bounded by a smallish number" is the
// whole win.

/// How long one counting window lasts. A minute is short enough that an
/// ordinary person never notices and long enough that a flood cannot
/// outrun it by waiting.
const RATE_WINDOW_SECONDS = 60;

/// How many of each kind of anonymous write one caller gets per window.
///
/// Set from what real use looks like, with room to spare. A person sending
/// a bug report sends one; a person making a shared store makes one. The
/// telemetry beacon is the busiest of them because the CLI sends one per
/// action, so it gets the loosest ceiling.
const RATE_LIMITS = {
  report: 5,
  "share/new": 10,
  usage: 60,
  founding: 5,
  makeit: 5,
  "support/new": 5,
  // Requests with no session to an organisation's routes or private
  // links (IC-946). They never reach the organisation's store -- so they
  // cannot grow its audit -- and this bounds what they cost. A person
  // opening a private link before signing in sends one.
  "org/anonymous": 20,
  // Requests to /scim/v2/ with no token or a wrong one (IC-955). They are
  // never audited, so this is what bounds them.
  "scim/refused": 20,
};

/// Who is asking, as far as the edge can tell.
///
/// `CF-Connecting-IP` is set by Cloudflare itself and cannot be forged by
/// the client -- Cloudflare overwrites whatever the caller sent. A request
/// that somehow carries none is counted under one shared bucket rather
/// than let through: an unattributable anonymous write is exactly what
/// this is here to bound.
function rateCaller(request) {
  return request.headers.get("cf-connecting-ip") || "unattributed";
}

/// Has this caller used up `bucket` for this window?
///
/// Returns a 429 Response to send back, or null to carry on. The window is
/// the clock divided into fixed minutes, so the key changes on its own and
/// the KV TTL sweeps the old one -- no cleanup pass, no listing.
///
/// A KV failure lets the request through. The alternative is a KV blip
/// taking down every anonymous write on the hub, and the thing being
/// protected is a bill, not a secret. K-082's lesson twice over: a cache
/// or a counter must never be able to break the product it guards.
async function rateLimited(request, env, bucket) {
  const limit = RATE_LIMITS[bucket];
  if (!limit) return null;
  const key = `${bucket}:${rateCaller(request)}`;
  let allowed = true;
  try {
    // One Durable Object per caller holds that caller's counts in memory:
    // exact, because every request from one address meets the same
    // object, and free of KV. Measured live on 2026-09-28, the two cheaper
    // counters are no wall: 40 requests from one address landed on about
    // fifteen isolates, so an isolate's own count never passed 4, and the
    // rate-limit binding answered success to 200 of 200 a minute against
    // a ceiling of 60 ("permissive, eventually consistent" by its own
    // documentation). A worker with no gate -- wrangler dev, an older
    // config -- counts in this isolate's memory, which is better than
    // nothing and still never KV.
    if (env.RATE_GATE) {
      const gate = env.RATE_GATE.get(env.RATE_GATE.idFromName(rateCaller(request)));
      const res = await gate.fetch("https://rate-gate/count", {
        method: "POST",
        body: JSON.stringify({ bucket, limit }),
      });
      allowed = (await res.json()).allowed !== false;
    } else {
      allowed = countInMemory(env, key, limit);
    }
  } catch (_) {
    // Counting failed; the request still goes through. See above.
    return null;
  }
  if (allowed) return null;
  return new Response(
    `too many requests -- wait a minute and try again`,
    {
      status: 429,
      headers: {
        "content-type": "text/plain; charset=utf-8",
        // How long until the window rolls over. A client that honours
        // this backs off exactly far enough and no further.
        "retry-after": String(
          RATE_WINDOW_SECONDS - (Math.floor(Date.now() / 1000) % RATE_WINDOW_SECONDS),
        ),
      },
    },
  );
}

/// A per-minute count kept in memory: the gate's store, and the fallback
/// for a worker with no gate. One fixed window per minute, forgotten when
/// the object or isolate goes. Keyed by `owner` so two workers in one
/// process -- two tests -- do not share a count.
const memoryCounts = new WeakMap();
function countInMemory(owner, key, limit) {
  let counts = memoryCounts.get(owner);
  if (!counts) {
    counts = new Map();
    memoryCounts.set(owner, counts);
  }
  const window = Math.floor(Date.now() / 1000 / RATE_WINDOW_SECONDS);
  const slot = `${key}:${window}`;
  const used = counts.get(slot) || 0;
  if (used >= limit) return false;
  // Old windows are dropped as they pass, so the map stays one minute big.
  for (const k of counts.keys()) if (!k.endsWith(`:${window}`)) counts.delete(k);
  counts.set(slot, used + 1);
  return true;
}

/// Whether one more would be allowed, without counting it.
function peekInMemory(owner, key, limit) {
  const counts = memoryCounts.get(owner);
  const window = Math.floor(Date.now() / 1000 / RATE_WINDOW_SECONDS);
  return ((counts && counts.get(`${key}:${window}`)) || 0) < limit;
}

/// The anonymous wall's counter for one caller (K-912). Nothing is
/// written to storage: the counts only need to outlive a minute, and a
/// Durable Object that goes idle and is evicted forgets them, which at
/// worst lets a caller start a minute early.
export class RateGate {
  constructor(state) {
    this.state = state;
  }

  async fetch(request) {
    const { bucket, limit, peek } = await request.json();
    // `peek` asks without counting: org/refused counts refusals only, but
    // asks on every request (IC-948).
    const allowed = peek
      ? peekInMemory(this, String(bucket), Number(limit) || 0)
      : countInMemory(this, String(bucket), Number(limit) || 0);
    return new Response(JSON.stringify({ allowed }), {
      headers: { "content-type": "application/json" },
    });
  }
}

export default {
  /// The nightly scrub (wrangler `[triggers] crons`): inventory and
  /// reconciliation without repairs, stored where /admin/scrub/last and
  /// /health read it (IC-852, tests 1886 and 1896).
  async scheduled(event, env) {
    // Every ten minutes: the founder's welcome to people who signed up at
    // least ten minutes ago. Nightly: the scrub.
    if (event && event.cron === WELCOME_CRON) {
      // A big upload still waiting for its judgement is asked about again.
      await retryBigJudging(env);
      // Only while the automatic welcome is switched on; the founder writes
      // them himself otherwise, from the desk.
      if (env.WELCOME_AUTO === "1") await sendDueWelcomes(env);
      return;
    }
    await runScrub(env, { repair: false });
    // Closed organisations past their export window (IC-948 D).
    await sweepOrgs(env, orgAsk);
  },
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const { pathname } = url;

    // The website and the CLI both call this from other origins.
    if (request.method === "OPTIONS") {
      return cors(new Response(null, { status: 204 }));
    }

    // A developer's own name: alice.krate.tech.
    //
    // Answered before anything else, because on these hostnames the paths
    // mean something different -- "/" is a person, "/notes" is their app,
    // and neither is a hub route. Everything underneath already existed:
    // the channel is the mutable per-developer name the hub has always
    // kept (channel:<login>/<slug>, 50 moves of history, rollback, moved
    // by that publisher alone), and nothing ever showed it to a human.
    // This is a face on that, not a new subsystem (K-849).
    const subdomain = developerHost(url, env);
    if (subdomain) {
      return cors(await developerRoute(request, url, subdomain, env));
    }

    try {
      if (request.method === "GET" && pathname === "/health") {
        return cors(await health(env));
      }
      // ---- durability: inventory, reconciliation, export, restore -------
      if (request.method === "GET" && pathname === "/admin/scrub") {
        return cors(await scrubRoute(request, env, false));
      }
      if (request.method === "POST" && pathname === "/admin/scrub") {
        return cors(await scrubRoute(request, env, true));
      }
      if (request.method === "GET" && pathname === "/admin/org-billing-alerts") {
        if (!(await isAdmin(request, env))) return cors(text("not found", 404));
        return cors(json(await orgBillingAlerts(env)));
      }
      if (request.method === "GET" && pathname === "/admin/scrub/last") {
        return cors(await lastScrub(request, env));
      }
      if (request.method === "GET" && pathname === "/admin/export") {
        return cors(await exportRoute(request, env));
      }
      if (request.method === "POST" && pathname === "/admin/restore") {
        return cors(await restoreRoute(request, url, env));
      }
      if (request.method === "POST" && pathname === "/publish") {
        return cors(await publish(request, env));
      }
      // The big-file path (IC-945).
      if (request.method === "POST" && pathname === "/publish/big/start") {
        return cors(await bigStart(request, env));
      }
      {
        const big = /^\/publish\/big\/([0-9a-zA-Z]{8,40})(?:\/(part\/(\d+)|finish))?$/.exec(pathname);
        if (big) {
          const [, id, tail, n] = big;
          if (request.method === "PUT" && tail && tail.startsWith("part/")) {
            return cors(await bigPart(request, env, id, Number(n)));
          }
          if (request.method === "POST" && tail === "finish") {
            return cors(await bigFinish(request, env, id));
          }
          if (request.method === "GET" && !tail) {
            return cors(await bigGetStatus(request, env, id));
          }
        }
        const internal = /^\/internal\/big\/([0-9a-zA-Z]{8,40})\/(bytes|verdict)$/.exec(pathname);
        if (internal) {
          const [, id, what] = internal;
          if (request.method === "GET" && what === "bytes") return await bigBytes(request, env, id);
          if (request.method === "POST" && what === "verdict") return await bigVerdict(request, env, id);
        }
      }
      if (request.method === "GET" && pathname === "/login/start") {
        return noteStep(env, "signin", "github-start", loginStart(url, env));
      }
      if (request.method === "GET" && pathname === "/login/callback") {
        return noteStep(env, "signin", "github-done", loginCallback(url, env, request));
      }
      if (request.method === "GET" && pathname === "/login/google/start") {
        return noteStep(env, "signin", "google-start", googleStart(url, env));
      }
      if (request.method === "GET" && pathname === "/login/google/callback") {
        return noteStep(env, "signin", "google-done", googleCallback(url, env, request));
      }
      // Company sign-in for an organisation (IC-948, F).
      if (request.method === "GET" && pathname === "/login/oidc/start") {
        return noteStep(env, "signin", "oidc-start", oidcStart(url, env, signInDeps()));
      }
      if (request.method === "GET" && pathname === "/login/oidc/callback") {
        return noteStep(env, "signin", "oidc-done", oidcCallback(url, env, signInDeps(request)));
      }
      // ... and by SAML 2.0 (IC-955): SP-initiated, the Response posted back.
      if (request.method === "GET" && pathname === "/login/saml/start") {
        return noteStep(env, "signin", "saml-start", samlStart(url, env, signInDeps()));
      }
      if (request.method === "POST" && /^\/login\/saml\/acs\/[a-z0-9-]+$/.test(pathname)) {
        return noteStep(env, "signin", "saml-done", samlAcs(request, url, env, { ...signInDeps(request), readBounded }));
      }
      if (request.method === "GET" && /^\/saml\/[a-z0-9-]+$/.test(pathname)) {
        return samlMetadata(url, env, signInDeps());
      }
      // The public half of the hub's offline-lease key (IC-948, G).
      if (request.method === "GET" && pathname === "/lease-key") {
        return cors(await leaseKeyRoute(env));
      }
      if (request.method === "POST" && pathname === "/login/email") {
        // Fetched from page scripts on krate.tech, unlike the login
        // redirects around it, so the answer must carry CORS headers or
        // the browser discards it and reports a phantom network failure.
        return cors(await noteStep(env, "signin", "email-start", emailStart(request, env)));
      }
      if (request.method === "GET" && pathname === "/login/email/verify") {
        return noteStep(env, "signin", "email-done", emailVerify(url, env, request));
      }
      if (request.method === "POST" && pathname === "/auth/start") {
        return cors(await noteStep(env, "signin", "desktop-start", authStart(env)));
      }
      if (request.method === "POST" && pathname === "/auth/poll") {
        return cors(await authPoll(request, env));
      }
      if (request.method === "POST" && pathname === "/founding") {
        // The founding 200: an email and a timestamp, nothing else. No
        // checkout here -- this is the list that gets the $79/yr lock when
        // Studio leaves preview.
        const over = await rateLimited(request, env, "founding");
        if (over) return cors(over);
        return cors(await founding(request, env));
      }
      if (request.method === "POST" && pathname === "/makeit") {
        // A failed build, sent to us. Studio used to offer this as "a
        // person at Krate builds it and emails you the file" with a
        // within-a-day delivery line -- a human-delivery promise with no
        // queue, no staffing and no SLA behind it (IC-425). The endpoint
        // is unchanged and still useful: it records the request that
        // failed, the answers given, and an email, so one person can read
        // it and the tool can improve. Nothing here promises delivery.
        const over = await rateLimited(request, env, "makeit");
        if (over) return cors(over);
        return cors(await makeit(request, env));
      }
      if (request.method === "POST" && pathname === "/usage") {
        const over = await rateLimited(request, env, "usage");
        if (over) return cors(over);
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
        const over = await rateLimited(request, env, "share/new");
        if (over) return cors(over);
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
          await noteView(request, env, body);
        } catch {
          // A malformed beacon is not worth an error to the visitor.
        }
        return cors(new Response(null, { status: 204 }));
      }
      // The release and star count krate.tech shows, fetched here and
      // cached, so a visitor's browser talks to krate.tech only and never
      // hands their address to a third party just by opening a page.
      if (request.method === "GET" && (pathname === "/release/latest" || pathname === "/repo")) {
        return cors(await githubMirror(pathname, request));
      }
      if (request.method === "GET" && pathname === "/stats") {
        return cors(await stats(env));
      }
      if (request.method === "GET" && pathname === "/apps") {
        return cors(await list(env, url));
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
        const over = await rateLimited(request, env, "report");
        if (over) return cors(over);
        return cors(await putReport(request, env));
      }
      // Registry takedown with notice, reason, scope and appeal (IC-669,
      // tests 1313-1314). A block is a record, never a silent 404.
      if (request.method === "POST" && pathname.startsWith("/admin/takedown/")) {
        const [hash, action] = pathname.slice("/admin/takedown/".length).split("/");
        if (action === "restore") return cors(await takedownRestore(request, hash, env));
        if (action === "deny") return cors(await takedownDeny(request, hash, env));
        if (!action) return cors(await takedownPut(request, hash, env));
        return cors(text("not found", 404));
      }
      if (request.method === "POST" && pathname.startsWith("/takedown/") && pathname.endsWith("/appeal")) {
        return cors(await takedownAppeal(request, pathname.slice("/takedown/".length, -"/appeal".length), env));
      }
      if (request.method === "GET" && pathname === "/takedowns") {
        return cors(await takedownList(env));
      }
      if (request.method === "GET" && pathname.startsWith("/takedown/")) {
        return cors(await takedownNotice(pathname.slice("/takedown/".length), env));
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
      // ---- the funded-case ledger (IC-414) -----------------------------
      // The allowance is counted in CASES -- one per app somebody asked
      // for -- not in raw counter bumps. A failed attempt retries inside
      // its case for free; only a case that produced a file consumes one.
      if (request.method === "POST" && pathname === "/case/open") {
        return cors(await noteStep(env, "try", "case-open", caseOpen(request, env)));
      }
      if (request.method === "POST" && pathname === "/case/attempt") {
        return cors(await caseAttempt(request, env));
      }
      if (request.method === "POST" && pathname === "/case/close") {
        return cors(await caseClose(request, env));
      }
      if (request.method === "POST" && pathname === "/case/list") {
        return cors(await caseList(request, env));
      }
      // ---- the account: profile, apps, referrals, the portal ----------
      // A person's own API key, and what their builds have cost.
      if (request.method === "GET" && pathname === "/keys") {
        return cors(await listKeys(request, env));
      }
      if (request.method === "POST" && pathname === "/keys") {
        return cors(await putKey(request, env));
      }
      if (request.method === "POST" && pathname === "/keys/forget") {
        return cors(await forgetKey(request, env));
      }
      if (request.method === "POST" && pathname === "/keys/use") {
        // NOT cors(): only the build service calls this, server to server.
        // Without the header a browser cannot read the answer, so a script
        // running on a krate.tech page -- an XSS -- cannot turn a stolen
        // session into the person's plaintext key (K-861).
        return useKey(request, env);
      }
      if (request.method === "GET" && pathname === "/spend") {
        return cors(await spendReport(request, env));
      }
      if (request.method === "POST" && pathname === "/spend") {
        return cors(await recordSpend(request, env));
      }
      if (request.method === "GET" && pathname === "/me") {
        return cors(await meProfile(request, env));
      }
      if (request.method === "POST" && pathname === "/me/onboarded") {
        return cors(await markAccountOnboarded(request, env));
      }
      if (request.method === "GET" && pathname === "/my/apps") {
        return cors(await myApps(request, env));
      }
      // Build sessions, kept against the account rather than the machine.
      //
      // Somebody makes an app in the browser, downloads Studio, signs in,
      // and the conversation they were having is there -- editable, not a
      // read-only receipt. Without this the browser's sessions die in
      // localStorage and the desktop's die on one disk, and the hand-off
      // between the two surfaces is a dead end.
      if (request.method === "GET" && pathname === "/sessions") {
        return cors(await sessionsList(request, env));
      }
      if (request.method === "POST" && pathname === "/sessions") {
        return cors(await sessionsPut(request, env));
      }
      if (request.method === "DELETE" && pathname.startsWith("/sessions/")) {
        return cors(await sessionsDelete(request, pathname.slice(10), env));
      }
      if (request.method === "POST" && pathname === "/referral/claim") {
        return cors(await referralClaim(request, env));
      }
      if (request.method === "POST" && pathname === "/billing/portal") {
        return cors(await billingPortal(request, env));
      }
      // ---- support: tickets with real conversations -------------------
      if (request.method === "POST" && pathname === "/support/new") {
        const over = await rateLimited(request, env, "support/new");
        if (over) return cors(over);
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
      // The validator module as a file, for a browser tab that signs as an
      // organisation's device (IC-958). It is the copy in the Player's
      // assets, byte for byte the module the publish door imports.
      if ((request.method === "GET" || request.method === "HEAD") && pathname === "/validator.wasm") {
        return await validatorFile(request, url, env);
      }
      // The browser Player (IC-956): its page, cross-origin isolated, and
      // the files it is made of. Not wrapped in CORS: nothing but this
      // origin loads them.
      if (request.method === "GET" && pathname.startsWith("/run/")) {
        const page = await playerPage(request, url, env);
        if (page) return page;
      }
      if (request.method === "GET" && pathname.startsWith("/player/")) {
        return await playerAsset(request, url, env);
      }
      if (request.method === "GET" && /^\/a\/[0-9a-f]{64}\/web$/.test(pathname)) {
        const hash = pathname.slice(3, 67);
        return cors(await webPackRoute(env, hash, {
          lookup: (h) => bundleObject(h, env),
          access: (object, slug) => privateRefusal(request, hash, object, slug, env),
          judgedComponent,
        }));
      }
      // HEAD answers exactly like GET, minus the body (K-798). Link
      // checkers and chat unfurlers ask with HEAD first, and a 404 there
      // made every shared app link read as dead.
      if ((request.method === "GET" || request.method === "HEAD") && pathname.startsWith("/a/")) {
        // A listing's source or notices, attached through the big-file path.
        const attached = /^\/a\/([0-9a-f]{64})\/(source|notices)$/.exec(pathname);
        if (attached) {
          const res = await fetchAttachment(attached[1], attached[2], env);
          return cors(request.method === "HEAD"
            ? new Response(null, { status: res.status, headers: res.headers })
            : res);
        }
        const res = await fetchBundle(request, url, pathname.slice(3), env);
        return cors(request.method === "HEAD"
          ? new Response(null, { status: res.status, headers: res.headers })
          : res);
      }
      if (request.method === "GET" && pathname.startsWith("/c/")) {
        return cors(await resolveChannel(request, url, pathname.slice(3), env));
      }
      // SCIM 2.0 provisioning (IC-955): server to server, so no CORS.
      if (pathname.startsWith("/scim/v2/")) {
        return await scimRoute(request, url, env, { orgAsk, readBounded, noteOrgOf, rateLimited, slugOk: (s) => SLUG.test(s) });
      }
      // Signed-in sessions: list them, sign one out, sign out the others.
      if (pathname.startsWith("/account/sessions")) {
        const res = await accountSessionsRoute(request, url, env, { verifyGitHub, testIdentity, githubClientId: GITHUB_CLIENT_ID });
        if (res) return cors(res);
      }
      // Organisations (IC-946).
      if (pathname === "/orgs" || pathname.startsWith("/orgs/")) {
        return cors(await orgRoute(request, url, env, ctx));
      }
      return cors(text("not found", 404));
    } catch (err) {
      // Never leak a stack trace to a caller; log it for us instead.
      console.error("unhandled", err && err.stack ? err.stack : String(err));
      // A body WE could not parse is the caller's fault, not ours -- but
      // only when it really was the CALLER'S body.
      //
      // A deeply nested JSON body (~5000 levels) overflows the runtime's
      // stack while parsing, and that RangeError is thrown inside the
      // platform's own json() rather than by our code, so the
      // `.catch(() => ({}))` the handlers use never sees it and it lands
      // here. Every one of /plan/get, /plan/count, /case/open and /case/list
      // answered "something went wrong on our side" to a body the caller
      // chose. Proved live at depth 5000; depth 4000 correctly gives 400.
      //
      // The first cut of this classified on the ERROR TYPE, and that was
      // too loose: there are fourteen unguarded `await <upstream>.json()`
      // calls in this file, so a SyntaxError from GITHUB returning an HTML
      // error page would have been reported as the caller's bad body. Same
      // for an ordinary RangeError from a real bug, or any internal message
      // containing the word "depth" (Cloudflare's own "Too many
      // subrequests: depth limit" among them).
      //
      // So the request's own body is re-read here, and it is the caller's
      // fault only when THAT is what will not parse. Reading it again is
      // cheap and certain, and a body that cannot be re-read is treated as
      // ours, which is the safe direction.
      let theirs = false;
      try {
        const copy = request.clone();
        const raw = await copy.text();
        if (raw) {
          try {
            JSON.parse(raw);
          } catch (_) {
            theirs = true;
          }
        }
      } catch (_) {
        // Body already consumed or unavailable: cannot prove it was theirs.
      }
      if (theirs) {
        return cors(text("that request body could not be read", 400));
      }
      // A 500 is a claim that WE broke. It should be true when we say it,
      // or the ones that matter get lost among the ones that do not.
      return cors(text("something went wrong on our side", 500));
    }
  },
};

// ---------------------------------------------------------------- publishing

// ------------------------------------------------------------- ownership
//
// Who owns a listing, a channel, an attachment: a principal nobody can
// choose (K-1085). `github:<login>` when GitHub vouched for the login;
// `account:<user-id>` for a Krate session (email or Google sign-in), whose
// `login` is only the part of an address before the @ and so is never a
// name to own things by. Every ownership check compares these exactly.
//
// A record written before this has no `owner`; it reads as
// `github:<author_login>` -- the legacy default -- so only a GitHub-vouched
// identity can act on it. scripts/attribute-owners.mjs gives the rows that
// email accounts really published back to their accounts.

/// The owner principal a verified identity publishes as, or null.
function ownerOf(identity) {
  if (!identity) return null;
  if (identity.github === true && identity.login) return `github:${String(identity.login).toLowerCase()}`;
  if (identity.id) return `account:${identity.id}`;
  return null;
}

/// Who owns a stored listing, ownership record or channel.
function ownerOfRecord(record) {
  if (!record || typeof record !== "object") return null;
  if (typeof record.owner === "string" && record.owner) return record.owner;
  const login = record.author_login || record.login;
  return login ? `github:${String(login).toLowerCase()}` : null;
}

/// Whether a developer namespace (`<login>.krate.tech`, `/c/<login>/...`)
/// may show this record: only a GitHub-vouched owner of that login.
function ownedByGitHubLogin(record, login) {
  return ownerOfRecord(record) === `github:${String(login || "").toLowerCase()}`;
}

/// Whether `hash` is already published by an owner other than `owner`:
/// by its listing, or -- when the listing write failed -- by the ownership
/// record written before the bytes were stored (IC-387, IC-833 test 1838).
/// A read hiccup answers "no", which is the pre-fix behaviour for that one
/// request rather than a failed publish.
async function publishedBySomeoneElse(env, hash, owner) {
  try {
    const listing = await env.APPS.get(`app:${hash}`);
    if (listing) {
      const prior = JSON.parse(listing);
      const theirs = ownerOfRecord(prior);
      if (theirs) return theirs !== owner;
    }
    const record = await env.APPS.get(`owner:${hash}`);
    if (record) {
      const theirs = ownerOfRecord(JSON.parse(record));
      return Boolean(theirs) && theirs !== owner;
    }
  } catch (_) {}
  return false;
}

async function publish(request, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) {
    return text(
      "publishing needs a GitHub sign-in -- run `krate publish` and it will ask",
      401,
    );
  }

  // Refuse a declared length before reading a byte (IC-833).
  //
  // `await request.arrayBuffer()` materialises the whole body in memory and
  // only then is its length checked, so a client that announces 500 MB is
  // allocated 500 MB before being told 5 MiB is the limit. The ceiling was
  // real but it was enforced after the cost it exists to prevent.
  //
  // Content-Length is the client's claim, not a fact -- it can lie, be
  // absent, or arrive chunked. So this is a fast refusal for the honest
  // oversized upload and the cheap half of an attack, and the check below
  // still stands for everything else. Two checks, not one moved: a
  // Content-Length that undercounts must not buy a bigger body.
  const declared = request.headers.get("content-length");
  if (declared !== null) {
    const n = Number(declared);
    if (!Number.isFinite(n) || n < 0 || !Number.isInteger(n)) {
      return text("content-length is not a length", 400);
    }
    if (n > MAX_UPLOAD_BYTES) {
      return text("bundle too large (5 MiB max)", 413);
    }
  }

  // The body that actually arrives, read a chunk at a time and dropped the
  // moment it passes the ceiling (IC-833, test 1831). `arrayBuffer()`
  // materialised the whole body first and judged it after, so a chunked or
  // lying upload still cost its full size before the 413.
  const read = await readBounded(request, MAX_UPLOAD_BYTES);
  if (read.tooLarge) {
    return text("bundle too large (5 MiB max)", 413);
  }
  const body = read.body;
  if (body.length === 0) {
    return text("empty body", 400);
  }

  // It must actually be a .krate: the archive contract, applied at the door
  // (K-309, IC-833 test 1833). The central directory is judged whole before
  // any entry is read -- count, names, duplicates, ceilings -- so the store
  // does not fill with what `krate run` would refuse.
  const problem = archiveProblem(body);
  if (problem) {
    return text(`not a valid .krate bundle: ${problem}`, 422);
  }

  // Then the validator itself: crates/bundle compiled to WebAssembly,
  // judging the archive, the manifest, the component against its declared
  // world, every ceiling, the identities and the signature -- the same
  // code and the same words `krate run` uses (IC-833, tests 1832-1836).
  // The listing's permissions, identities and release come from this
  // judgement, never from a header (1835).
  const judged = judgeBundle(body);
  if (!judged.ok) {
    return text(`not a valid .krate bundle: ${judged.problem}`, 422);
  }
  const hash = await sha256Hex(body);
  const admitted = await admitJudged(env, {
    identity,
    judgement: judged.judgement,
    hash,
    size: body.length,
    fields: listingFields(request),
    store: () => storeBundle(env, hash, body),
  });
  // What a browser tab runs for this release, made once, now (IC-956).
  // Best effort: a component the transpiler refuses still publishes, and
  // opens in Krate; the Player says so.
  if (admitted.status === 200) await ensureWebPack(env, hash, body, judgedComponent);
  return admitted;
}

/// What a publisher says about a listing, from the publish door's headers.
function listingFields(request) {
  return {
    unlisted: header(request, "x-krate-unlisted") === "1",
    name: textHeader(request, "x-krate-name") || "",
    description: textHeader(request, "x-krate-description") || "",
    category: shelfOf(header(request, "x-krate-category")),
  };
}

/// A small fixed shelf list: free-text categories fragment a gallery.
function shelfOf(raw) {
  const c = (raw || "").toLowerCase();
  return ["games", "productivity", "tools", "media", "learning"].includes(c) ? c : "apps";
}

/// Everything after a bundle has been judged, for both doors (IC-945): the
/// publish door's one request and the big tier's parts judged on the build
/// service. One function, so the two cannot list an app differently: the
/// signature refusal, ownership before bytes, the listing read from the
/// judgement, the alias, the retired rows and the moving name.
///
/// `store` puts the bytes where /a/<hash> serves them and answers
/// `{ size, sha256 }` or `{ problem }`; `extra` adds fields to the listing.
async function admitJudged(env, { identity, judgement, hash, size, fields, store, extra }) {
  const marked = privateAppRefusal(judgement);
  if (marked) return marked;
  if (judgement.signature.state !== "absent" && judgement.signature.state !== "valid") {
    // A file that carries a signature that does not verify is not
    // published: to a recipient it would read as somebody's release that
    // was changed after they signed it.
    return text(
      `this app carries a signature that does not verify (${judgement.signature.state}): ` +
        `${judgement.signature.detail || ""}`.trim(),
      422,
    );
  }
  const manifestCapabilities = judgement.manifest.capabilities.map((c) => ({
    cap: c.cap,
    rationale: c.rationale || "",
    required: Boolean(c.required),
  }));
  // Who owns what this publish writes (K-1085): never a bare login.
  const owner = ownerOf(identity);
  if (!owner) return text("publishing needs a sign-in -- run `krate publish` and it will ask", 401);
  const vouched = owner.startsWith("github:");

  // Identical bytes are idempotent, and authorship does not transfer
  // (IC-387). Content addressing means anyone can re-POST bytes they
  // downloaded, and this write used to replace the listing's author with
  // theirs -- one authenticated request took over any app's public
  // identity. If these bytes are already published by someone else, the
  // existing listing stands and they get the same URLs back. Asked BEFORE
  // the store is touched: those bytes are already there, and nothing about
  // this request should write anything.
  // An organisation's private release is never given a public listing
  // (IC-946): the gallery, search and every developer page read listings,
  // so this one refusal keeps the bytes out of all of them.
  if ((await bytesHolder(env, hash)).org) {
    return text(
      "these exact bytes are an organisation's private release, so they cannot be published publicly",
      409,
    );
  }

  if (await publishedBySomeoneElse(env, hash, owner)) {
    const base2 = (env.PUBLIC_BASE || "").replace(/\/$/, "");
    return json({
      url: `${base2}/a/${hash}`,
      full_url: `${base2}/a/${hash}`,
      id: hash,
      note:
        "these exact bytes are already published; the existing listing " +
        "and its author stand",
    });
  }

  // Who published it, written BEFORE the bytes become public (IC-833,
  // test 1838). The bundle is served at /a/<hash> the moment the store
  // holds it, and the listing written after it was the only other record
  // of an author -- so a KV write that failed, a client that disconnected
  // or an isolate that died in between left an app anyone could download
  // and nobody could remove, its author included. This is the one write a
  // publish cannot do without: if it fails, nothing is stored and the
  // person is told to try again, which costs them a retry rather than
  // leaving an ownerless app on the internet.
  const ownerKey = `owner:${hash}`;
  try {
    await env.APPS.put(
      ownerKey,
      JSON.stringify({ login: identity.login, owner, at: Math.floor(Date.now() / 1000) }),
    );
  } catch (_) {
    return text(
      "could not record who is publishing this, so nothing was stored -- try again in a few minutes",
      503,
    );
  }

  // Content-addressed: republishing the same bytes is a no-op that returns the
  // same URL, so a person who publishes twice does not get two entries. The
  // store is told the digest and read back before anything is listed
  // (IC-833, tests 1837 and 1840).
  const stored = await store();
  if (stored.problem) {
    // Nothing public was created, so the ownership record owns nothing.
    // Best effort: a stray one names no bytes and is harmless.
    try {
      if (!(await env.BUNDLES.head(hash))) await env.APPS.delete(ownerKey);
    } catch (_) {}
    return text(stored.problem, 500);
  }

  const meta = {
    unlisted: Boolean(fields.unlisted),
    name: fields.name || "Untitled app",
    description: fields.description || "",
    category: fields.category || "apps",
    author: identity.name || identity.login,
    author_login: identity.login,
    // The owner the checks compare; author_login stays for display and
    // older clients. Only a GitHub-vouched owner links to a GitHub profile.
    owner,
    author_url: vouched ? `https://github.com/${identity.login}` : "",
    avatar_url: identity.avatar_url || "",
    published: Math.floor(Date.now() / 1000),
    size,
    // What the app declares it needs, so its page can show the permissions
    // BEFORE anyone downloads it. This is the one thing a Krate listing has
    // that an app store listing does not, and it was not being stored at
    // all -- the gallery could say an app was 36 KB but not that it asks for
    // nothing beyond a window.
    //
    // Sent by the CLI as a compact JSON array; capped and validated here
    // because it arrives in a header and a header is not to be trusted.
    // From the manifest INSIDE the bytes, never from a header (IC-669,
    // test 1310). The header the CLI sends is what an honest client
    // derived from the manifest; a dishonest one could list a microphone
    // app as asking for nothing, and the same author could re-POST the
    // identical bytes with a different header and rewrite the permissions
    // the page shows. The bytes are the claim; the listing reads them.
    capabilities: manifestCapabilities,
    // What the validator concluded, kept with the listing so a page and a
    // channel can show it (IC-389 tests 468 and 476): the identities, the
    // signature's state and key, the signed release when there is one,
    // the fork record, and which validator said so.
    identity: {
      archive: judgement.archive,
      execution: judgement.execution,
      project: judgement.project,
    },
    signature: { state: judgement.signature.state, public_key: judgement.signature.public_key },
    release: judgement.release,
    derived_from: judgement.derived_from,
    validator: { version: judgement.validator, wit: judgement.wit },
    ...(extra || {}),
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
      if (ownerOfRecord(other) === owner && other.name === meta.name) {
        await env.APPS.delete(key.name);
        // A retired release's bytes are anyone's to re-publish (see
        // `namable`), so its ownership record retires with its listing. The
        // record exists for one window -- a publish that never got its
        // listing -- and must not outlive the listing it stood in for.
        await env.APPS.delete(`owner:${otherHash}`);
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
  const result = {
    url: `${base}/a/${short}`,
    full_url: `${base}/a/${hash}`,
    id: hash,
    // The receipt: what the store holds, read back, not what was sent.
    stored: { size: stored.size, sha256: stored.sha256 },
  };
  // The moving name: this author's latest listed publish under this name
  // (IC-389, test 470). Only a listed publish moves it -- an unlisted one
  // is deliberately not the app's public face -- and only this author's,
  // because the key is their login and the login came from their session.
  // A moving name lives under a developer namespace, which only a
  // GitHub-vouched owner has (K-1085): `/c/bob/notes` is github:bob's.
  if (listed && !meta.unlisted && vouched) {
    const channel = await moveChannel(env, identity.login, meta.name, hash, meta.release ? meta.release.id : null);
    if (channel) {
      result.channel = `${base}/c/${channel.login}/${channel.slug}`;
    }
  }
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
/// The listing for a hash, but only when this caller owns it (IC-387).
///
/// A screenshot or icon is part of an app's public listing, and the listing
/// belongs to whoever published the app -- not to whoever is signed in.
/// Before this gate, any GitHub account could replace any app's images:
/// defacement of someone else's listing with one authenticated POST.
async function ownedApp(hash, identity, env) {
  const raw = await env.APPS.get(`app:${hash}`);
  if (!raw) {
    return { error: text("publish the app first -- images belong to a listing", 404) };
  }
  const meta = JSON.parse(raw);
  if (!ownerOf(identity) || ownerOfRecord(meta) !== ownerOf(identity)) {
    return { error: text("only the app's author can change its listing", 403) };
  }
  return { meta };
}

async function putShot(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const owned = await ownedApp(hash, identity, env);
  if (owned.error) return owned.error;

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
  // A bug report does NOT need a sign-in.
  //
  // It did, and that was backwards: the person whose product just broke was
  // asked to authenticate before they could say so. One outside user hit
  // exactly that -- the build failed, he pressed "Send to support", and got a
  // login page he could not get through with GitHub, Google or email. The
  // failure that made him want to report it is the same failure we then never
  // heard about (K-757).
  //
  // The identity bought nothing. `from` was written into the metadata and
  // read by no code anywhere -- not the admin list, not the detail view. We
  // need the zip, not the name.
  //
  // Still recorded when it happens to be there, because a report we can reply
  // to is better than an anonymous one. Optional, never a gate.

  // Refuse a declared length before reading a byte, the same way /publish
  // does (K-767). This used to be `new Uint8Array(await
  // request.arrayBuffer())` with the 12 MiB check after it, so a client
  // announcing 500 MB was allocated 500 MB before being told the ceiling.
  // The ceiling was real and enforced after the cost it exists to prevent,
  // and this endpoint is anonymous on purpose (K-757), so it is the
  // cheapest one to point a flood at.
  //
  // Content-Length is the client's claim, not a fact -- it can lie, be
  // absent, or arrive chunked. So this is a fast refusal for the honest
  // oversized report and the cheap half of an attack, and the bounded read
  // below still stands for everything else. Two checks, not one moved.
  const declared = request.headers.get("content-length");
  if (declared !== null) {
    const n = Number(declared);
    if (!Number.isFinite(n) || n < 0 || !Number.isInteger(n)) {
      return text("content-length is not a length", 400);
    }
    if (n > MAX_REPORT_BYTES) {
      return text("a report must be a zip under 12 MiB", 413);
    }
  }

  const identity = await verifyGitHub(request, env).catch(() => null);
  const read = await readBounded(request, MAX_REPORT_BYTES);
  if (read.tooLarge) {
    return text("a report must be a zip under 12 MiB", 413);
  }
  const body = read.body;
  if (body.length === 0) {
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
    from: identity ? identity.login : "anonymous",
    name: identity ? identity.name || identity.login : "anonymous",
    session: header(request, "x-krate-session") || "",
    krate: header(request, "x-krate-version") || "",
    os: header(request, "x-krate-os") || "",
    note: (textHeader(request, "x-krate-note") || "").slice(0, 400),
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
  // KRATE_ADMINS names GitHub logins, so only a login GitHub vouched for
  // can match it: an email or Google session whose address starts with an
  // admin's login is not that admin (K-1085).
  if (!identity || identity.github !== true) return null;
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
  // The listing names the author. When the listing write failed at
  // publish, the ownership record written before the bytes were stored
  // still does (IC-833, test 1838), so an author can always remove what
  // they published.
  const raw = await env.APPS.get(`app:${hash}`);
  const ownerRaw = raw ? null : await env.APPS.get(`owner:${hash}`);
  if (!raw && !ownerRaw) return text("not listed", 404);
  const meta = raw ? JSON.parse(raw) : null;
  const author = meta ? ownerOfRecord(meta) : ownerOfRecord(JSON.parse(ownerRaw));
  if (!ownerOf(identity) || author !== ownerOf(identity)) {
    return text("only the app's author can remove it", 403);
  }
  // Delisting is not deleting. The listing is one of four places an app
  // exists: the bundle blob (served by /a/<hash> forever), the short-link
  // alias that resolves to it, and the screenshot. Removing only the
  // listing left the app downloadable by anyone holding the URL -- which is
  // the opposite of what "remove it" means to the person clicking it.
  await deleteBigParts(env, hash);
  await env.APPS.delete(`app:${hash}`);
  await env.APPS.delete(`owner:${hash}`);
  await env.BUNDLES.delete(hash);
  await env.BUNDLES.delete(`shot:${hash}`);
  await env.BUNDLES.delete(`icon:${hash}`);
  // The alias is keyed by prefix, so clear every prefix length a short link
  // could have used.
  for (let length = 8; length <= 32; length += 1) {
    await env.APPS.delete(`alias:${hash.slice(0, length)}`);
  }
  // A channel pointing at the removed release falls back to the newest
  // earlier release of the same name that is still listed, and is removed
  // when there is none: a name must never resolve to bytes that are gone.
  // Only a listed publish ever moved a channel, so an app that never got
  // its listing has none to retreat.
  if (meta && ownerOf(identity).startsWith("github:")) await retreatChannel(env, identity.login, meta.name, hash);
  return json({ ok: true });
}

// ----------------------------------------------------------------- channel
//
// A channel is the one mutable name in the hub: `<publisher>/<app>`, moved
// by that publisher's own listed publishes and by nobody else (IC-389,
// test 470; K-308). Everything else the hub serves is a content address.
// Each move records what it replaced, so the channel's history is an
// audit trail rather than a pointer, and a client never runs what a
// channel says: `/c/<login>/<slug>?dl=1` redirects to the fixed address
// the channel currently names, and the client holds the bytes to THAT
// address (test 471). The JSON form is the visible resolution (test 476).

const CHANNEL_HISTORY = 50;

function slugOf(name) {
  return (name || "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
}

function channelKey(login, slug) {
  return `channel:${login}/${slug}`;
}

async function readChannel(env, login, slug) {
  const raw = await env.APPS.get(channelKey(login, slug));
  if (!raw) return null;
  try {
    return JSON.parse(raw);
  } catch (_) {
    return null;
  }
}

/// Point `login`'s channel for `name` at `hash`, recording the move.
async function moveChannel(env, login, name, hash, releaseId = null) {
  const slug = slugOf(name);
  if (!slug || !login) return null;
  const now = Math.floor(Date.now() / 1000);
  const existing = (await readChannel(env, login, slug)) || {
    login,
    name,
    slug,
    current: null,
    moved_at: null,
    history: [],
  };
  if (existing.current === hash) {
    return existing; // republishing the same bytes moves nothing
  }
  const move = { hash, at: now, previous: existing.current, release: releaseId };
  const channel = {
    login,
    name,
    slug,
    current: hash,
    moved_at: now,
    history: [...existing.history, move].slice(-CHANNEL_HISTORY),
  };
  try {
    await env.APPS.put(channelKey(login, slug), JSON.stringify(channel));
  } catch (_) {
    return null; // a KV quota day: the app is live at its address regardless
  }
  return channel;
}

/// Can the channel still name `hash`? Its bytes must exist, and if the
/// hash is listed, the listing must be this publisher's: a content
/// address is anyone's to re-publish once the original listing is retired,
/// and a channel must never resolve to bytes now listed under somebody
/// else's name.
async function namable(env, login, hash) {
  if (!(await env.BUNDLES.head(hash))) return false;
  const raw = await env.APPS.get(`app:${hash}`);
  if (!raw) return true;
  try {
    return ownedByGitHubLogin(JSON.parse(raw), login);
  } catch (_) {
    return false;
  }
}

/// Make the channel name something that still exists, or remove it.
///
/// Called after an unpublish and on every resolution, because bytes can
/// go by other doors too (an admin purge, a bytes-scope takedown): a name
/// that resolves to gone bytes is the failure this exists to prevent, so
/// the check sits on the path that answers, not only on the paths that
/// delete. Falls back to the newest earlier release of the channel's own
/// history that can still be named -- retiring an old listing keeps its
/// bytes and its links, so the previous release is what the publisher's
/// recipients already had.
async function settleChannel(env, channel, reason) {
  if (await namable(env, channel.login, channel.current)) return channel;
  const gone = channel.current;
  const earlier = [...channel.history].reverse().map((m) => m.hash).filter((h) => h !== gone);
  let fallback = null;
  for (const candidate of earlier) {
    if (await namable(env, channel.login, candidate)) {
      fallback = candidate;
      break;
    }
  }
  if (!fallback) {
    await env.APPS.delete(channelKey(channel.login, channel.slug));
    return null;
  }
  const now = Math.floor(Date.now() / 1000);
  channel.history = [...channel.history, { hash: fallback, at: now, previous: gone, reason }].slice(-CHANNEL_HISTORY);
  channel.current = fallback;
  channel.moved_at = now;
  await env.APPS.put(channelKey(channel.login, channel.slug), JSON.stringify(channel));
  return channel;
}

async function retreatChannel(env, login, name, hash) {
  const channel = await readChannel(env, login, slugOf(name));
  if (!channel || channel.current !== hash) return;
  await settleChannel(env, channel, "unpublished");
}

async function resolveChannel(request, url, rest, env) {
  const parts = rest.split("/").filter(Boolean);
  if (parts.length !== 2) return text("not found", 404);
  const [login, slug] = parts;
  if (!/^[A-Za-z0-9-]{1,39}$/.test(login) || !/^[a-z0-9-]{1,48}$/.test(slug)) {
    return text("not found", 404);
  }
  const stored = await readChannel(env, login, slug);
  const channel = stored && stored.current ? await settleChannel(env, stored, "gone") : null;
  if (!channel) return text("not found", 404);
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  if (url.searchParams.has("dl")) {
    // Never the bytes from here: the client lands on the fixed address and
    // checks the bytes against it, so a stale or wrong channel cannot run
    // anything the address does not name.
    return new Response(null, {
      status: 302,
      headers: { location: `${base}/a/${channel.current}?dl=1`, "cache-control": "no-store" },
    });
  }
  const takedown = await env.APPS.get(`takedown:${channel.current}`);
  return json({
    login: channel.login,
    name: channel.name,
    slug: channel.slug,
    current: channel.current,
    url: `${base}/a/${channel.current}`,
    moved_at: channel.moved_at,
    blocked: Boolean(takedown),
    history: channel.history,
  });
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
// ---------------------------------------------------------------- takedown
//
// A registry block is a RECORD with a notice, a reason, a scope and an
// appeal, never a silent 404 (IC-669, test 1313). The listing leaves the
// gallery and the hash answers 451 with the notice. The scope says what
// happened to the bytes: "listing" keeps them, so an upheld appeal restores
// the app as it was; "listing-and-bytes" removes them, for the case where
// serving one more copy is the harm. Neither touches anything on the
// developer's own machine (1314): a block is about this registry.
//
// Closing a takedown does not erase it. The record moves to
// takedown-closed:<hash> with the decision, so "was this ever blocked, and
// why" has an answer afterwards.

const TAKEDOWN_SCOPES = new Set(["listing", "listing-and-bytes"]);

async function readJson(request, max = 8192) {
  const raw = await request.text();
  if (raw.length > max) return null;
  try {
    const value = JSON.parse(raw || "{}");
    return value && typeof value === "object" ? value : null;
  } catch (_) {
    return null;
  }
}

function takedownResponse(rawRecord, env) {
  let record;
  try {
    record = JSON.parse(rawRecord);
  } catch (_) {
    return text("this app was removed from the registry", 451);
  }
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  return new Response(
    JSON.stringify({
      removed: true,
      reason: record.reason,
      scope: record.scope,
      at: record.at,
      emergency: record.emergency === true,
      appeal: { state: (record.appeal && record.appeal.state) || "none", how: `POST ${base}/takedown/${record.hash}/appeal as the author` },
      notice: `${base}/takedown/${record.hash}`,
    }),
    { status: 451, headers: { "content-type": "application/json", link: `<${base}/takedown/${record.hash}>; rel="blocked-by"` } },
  );
}

async function takedownPut(request, hash, env) {
  const admin = await isAdmin(request, env);
  if (!admin) return text("not found", 404);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const body = await readJson(request);
  if (!body) return text("a takedown is a JSON object", 400);
  const reason = String(body.reason || "").trim().slice(0, 500);
  if (!reason) return text("a takedown needs a reason; it is what the notice shows", 400);
  const scope = body.scope || "listing";
  if (!TAKEDOWN_SCOPES.has(scope)) return text(`scope must be one of ${[...TAKEDOWN_SCOPES].join(", ")}`, 400);
  const listed = await env.APPS.get(`app:${hash}`);
  const bytes = await env.BUNDLES.head(hash);
  if (!listed && !bytes) return text("nothing is published under that hash", 404);
  const record = {
    hash,
    reason,
    scope,
    emergency: body.emergency === true,
    notice: String(body.notice || "").slice(0, 500),
    by: admin.login,
    at: Math.floor(Date.now() / 1000),
    appeal: { state: "none" },
  };
  await env.APPS.put(`takedown:${hash}`, JSON.stringify(record));
  if (scope === "listing-and-bytes") {
    await env.BUNDLES.delete(hash);
    await env.BUNDLES.delete(`shot:${hash}`);
    await env.BUNDLES.delete(`icon:${hash}`);
  }
  return json({ ok: true, takedown: record });
}

/// Every open takedown, for clients to hold and honour offline (IC-669,
/// test 1311). Closed ones are not here: a restored app is not blocked.
async function takedownList(env) {
  const keys = await listAllKeys(env.APPS, "takedown:");
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const blocked = [];
  for (const key of keys) {
    const raw = await env.APPS.get(key);
    if (!raw) continue;
    try {
      const record = JSON.parse(raw);
      blocked.push({
        hash: record.hash,
        reason: record.reason,
        scope: record.scope,
        emergency: record.emergency === true,
        at: record.at,
        notice: `${base}/takedown/${record.hash}`,
      });
    } catch (_) {
      // an unreadable record blocks nothing it cannot name
    }
  }
  blocked.sort((a, b) => (a.hash < b.hash ? -1 : a.hash > b.hash ? 1 : 0));
  return new Response(
    JSON.stringify({ schema: "krate.hub-blocklist.v1", hub: base, issued_at: Math.floor(Date.now() / 1000), fetched_at: 0, blocked }),
    { headers: { "content-type": "application/json", "cache-control": "public, max-age=300" } },
  );
}

async function takedownNotice(hash, env) {
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`takedown:${hash}`);
  if (raw) return takedownResponse(raw, env);
  const closed = await env.APPS.get(`takedown-closed:${hash}`);
  if (closed) {
    const record = JSON.parse(closed);
    return json({ removed: false, was_removed: true, reason: record.reason, closed: record.closed });
  }
  return text("not found", 404);
}

async function takedownAppeal(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`takedown:${hash}`);
  if (!raw) return text("not found", 404);
  const record = JSON.parse(raw);
  const listed = await env.APPS.get(`app:${hash}`);
  const author = listed ? ownerOfRecord(JSON.parse(listed)) : null;
  if (!author || author !== ownerOf(identity)) return text("only the app's author can appeal its removal", 403);
  const body = await readJson(request);
  const appealText = String((body && body.text) || "").trim().slice(0, 2000);
  if (!appealText) return text("an appeal needs a sentence: what is wrong with the reason given", 400);
  record.appeal = { state: "appealed", text: appealText, by: identity.login, at: Math.floor(Date.now() / 1000) };
  await env.APPS.put(`takedown:${hash}`, JSON.stringify(record));
  return json({ ok: true, appeal: record.appeal });
}

async function takedownRestore(request, hash, env) {
  const admin = await isAdmin(request, env);
  if (!admin) return text("not found", 404);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`takedown:${hash}`);
  if (!raw) return text("not found", 404);
  const record = JSON.parse(raw);
  const body = await readJson(request);
  record.closed = { decision: "restored", by: admin.login, at: Math.floor(Date.now() / 1000), note: String((body && body.note) || "").slice(0, 500) };
  await env.APPS.put(`takedown-closed:${hash}`, JSON.stringify(record));
  await env.APPS.delete(`takedown:${hash}`);
  const bytes = await env.BUNDLES.head(hash);
  return json({
    ok: true,
    restored: Boolean(bytes),
    note: bytes ? "the listing is back as it was" : "the bytes were removed with the takedown; the author must publish again",
  });
}

async function takedownDeny(request, hash, env) {
  const admin = await isAdmin(request, env);
  if (!admin) return text("not found", 404);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`takedown:${hash}`);
  if (!raw) return text("not found", 404);
  const record = JSON.parse(raw);
  const body = await readJson(request);
  record.appeal = { ...(record.appeal || {}), state: "denied", decided_by: admin.login, decided_at: Math.floor(Date.now() / 1000), note: String((body && body.note) || "").slice(0, 500) };
  await env.APPS.put(`takedown:${hash}`, JSON.stringify(record));
  return json({ ok: true, appeal: record.appeal });
}

async function purgeBundle(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  const admins = (env.KRATE_ADMINS || "").split(",").map((name) => name.trim());
  // Only a GitHub-vouched login is an admin (K-1085).
  if (identity.github !== true || !admins.includes(identity.login)) return text("not found", 404);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  await deleteBigParts(env, hash);
  await env.APPS.delete(`app:${hash}`);
  await env.APPS.delete(`owner:${hash}`);
  await env.BUNDLES.delete(hash);
  await env.BUNDLES.delete(`shot:${hash}`);
  await env.BUNDLES.delete(`icon:${hash}`);
  for (let length = 8; length <= 32; length += 1) {
    await env.APPS.delete(`alias:${hash.slice(0, length)}`);
  }
  return json({ ok: true, purged: hash });
}

/// Store a small square logo for a published app. Same contract as the
/// screenshot: separate from the bundle, replaceable, and owned -- the old
/// comment here said "author-gated by the same sign-in the publish used",
/// which was true only in the sense that A sign-in was required. Whose was
/// never checked (IC-387).
async function putIcon(request, hash, env) {
  const identity = await verifyGitHub(request, env);
  if (!identity) return text("sign in first", 401);
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const owned = await ownedApp(hash, identity, env);
  if (owned.error) return owned.error;
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
  const takedown = await env.APPS.get(`takedown:${hash}`);
  if (takedown) {
    return takedownResponse(takedown, env);
  }
  let object = await env.BUNDLES.get(hash);
  // A big-tier app lives where its upload landed, named by the record its
  // passing verdict wrote (IC-945). Asked only on a miss, so an ordinary
  // download costs no extra read.
  let big = false;
  if (!object) {
    const blob = await env.APPS.get(`bigkey:${hash}`);
    if (blob) {
      object = await env.BUNDLES.get(blob);
      big = Boolean(object);
    }
  }
  if (!object) {
    return text("not found", 404);
  }

  // An organisation's private release (IC-946). The flag is on the R2
  // object itself, written with the bytes, so it is as consistent as the
  // bytes are: there is no moment at which the object exists and the
  // check does not. The organisation's store is asked on every fetch.
  const privateOrg = object.customMetadata && object.customMetadata.org;
  if (privateOrg) {
    return await fetchPrivate(request, url, hash, object, privateOrg, big, env);
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
  // A HEAD is a link checker or an unfurler looking, not a person or a
  // runtime fetching; it is answered but not counted as a hit.
  if (env.USAGE && request.method !== "HEAD") {
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

  const headers = {
    "content-type": "application/octet-stream",
    // Content-addressed, so a bundle at a given URL can never change and
    // may be cached forever. Vary, because the same URL serves a phone a
    // landing page instead.
    "cache-control": "public, max-age=31536000, immutable",
    vary: "accept, user-agent",
    "content-disposition": `attachment; filename="${filename}"`,
    // The size up front, so a download can show its progress; and the
    // tier, so `krate run` knows to save a big app to a file first.
    "content-length": String(object.size),
    ...(big ? { "x-krate-tier": "big" } : {}),
  };
  if (request.method === "HEAD") {
    // The size is the one thing a HEAD is usually asked for, and the
    // object knows it without the body being read.
    return new Response(null, { headers: { ...headers, "content-length": String(object.size) } });
  }
  return new Response(object.body, { headers });
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
  const takedown = await env.APPS.get(`takedown:${hash}`);
  if (takedown) return takedownResponse(takedown, env);
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
    meta: {
      name: m.name,
      description: m.description,
      author: m.author,
      avatar_url: m.avatar_url || "",
      size: m.size,
      published: m.published,
      // The permissions, so /app/?a=<hash> can list them before anyone
      // downloads anything. Empty for apps published before this shipped,
      // and the page says "not recorded" rather than "asks for nothing" --
      // an absent list and an empty one mean very different things.
      capabilities: Array.isArray(m.capabilities) ? m.capabilities : null,
    },
  });
}

/// The gallery.
///
/// Returns SHELVES, not one flat list. A grid of everything works at twenty
/// apps and fails at a thousand: nobody scrolls a gallery looking for
/// something good, they arrive with intent or they want to be shown a
/// shortlist. So the landing payload is a handful of short rows -- new, most
/// opened, and the busiest categories -- and everything else is reached by
/// searching, which is a query rather than a scroll.
///
/// `?q=` searches and returns matches instead of shelves. `?cat=` returns one
/// category. Both are capped, so no response is ever a thousand rows.
async function list(env, url) {
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const q = (url?.searchParams.get("q") || "").trim().toLowerCase().slice(0, 64);
  const cat = (url?.searchParams.get("cat") || "").trim().slice(0, 24);

  // Read every listed record. The KV gets are unavoidable -- the metadata IS
  // the value -- but they go out together rather than one at a time.
  const listing = await env.APPS.list({ prefix: "app:", limit: 1000 });
  const blocked = new Set(
    (await env.APPS.list({ prefix: "takedown:", limit: 1000 })).keys.map((k) => k.name.slice("takedown:".length)),
  );
  const records = await Promise.all(
    listing.keys.map(async (key) => {
      if (blocked.has(key.name.slice("app:".length))) return null;
      const raw = await env.APPS.get(key.name);
      if (!raw) return null;
      let meta;
      try {
        meta = JSON.parse(raw);
      } catch (_) {
        return null;
      }
      // Unlisted apps have working links and no gallery row.
      if (meta.unlisted) return null;
      return { id: key.name.slice("app:".length), meta };
    }),
  );

  let apps = records.filter(Boolean);

  // Search and category filtering happen HERE, before any R2 work, so a
  // search for one app does not cost a thousand head requests.
  if (q) {
    apps = apps.filter((a) => {
      const m = a.meta;
      return `${m.name || ""} ${m.description || ""} ${m.author || ""}`
        .toLowerCase()
        .includes(q);
    });
  }
  if (cat) apps = apps.filter((a) => (a.meta.category || "apps") === cat);

  const opens = await openCounts(env);
  const byNew = [...apps].sort((a, b) => (b.meta.published || 0) - (a.meta.published || 0));
  const byOpens = [...apps].sort(
    (a, b) => (opens.get(b.id) || 0) - (opens.get(a.id) || 0)
      || (b.meta.published || 0) - (a.meta.published || 0),
  );

  // Which apps actually need their images resolved. Only the ones about to
  // be sent -- this is what turns 2000 R2 head requests into a few dozen.
  const SHELF = 12;
  const chosen = new Map();
  const take = (arr, n) => arr.slice(0, n).map((a) => (chosen.set(a.id, a), a.id));

  let payload;
  if (q || cat) {
    // A query wants results, not shelves.
    payload = { query: q || null, category: cat || null, results: take(byNew, 60) };
  } else {
    const cats = {};
    for (const a of apps) {
      const c = a.meta.category || "apps";
      (cats[c] ||= []).push(a);
    }
    const shelves = [
      { id: "new", label: "New this week", apps: take(byNew, SHELF) },
    ];
    // Only when there is something to rank. Before any opens are recorded,
    // "most opened" would be an arbitrary order wearing a confident label.
    if (opens.size) {
      shelves.push({ id: "opened", label: "Most opened", apps: take(byOpens, SHELF) });
    }
    for (const [id, list_] of Object.entries(cats)) {
      if (list_.length < 3) continue;   // a shelf of one is not a shelf
      list_.sort((a, b) => (b.meta.published || 0) - (a.meta.published || 0));
      shelves.push({ id, label: id, apps: take(list_, SHELF) });
    }
    payload = { shelves, total: apps.length };
  }

  // Which categories have anything in them, so the client can offer exactly
  // those chips. It cannot work this out itself any more: it only receives
  // the shelves, and a category with fifty apps might have none of them on
  // this week's rows.
  payload.cats = [...new Set(apps.map((a) => a.meta.category || "apps"))];

  // Now, and only now, ask R2 what pictures exist -- for the few dozen apps
  // being returned rather than for everything published.
  const ids = [...chosen.keys()];
  const images = new Map(
    await Promise.all(
      ids.map(async (id) => {
        const [shot, icon] = await Promise.all([
          env.BUNDLES.head(`shot:${id}`),
          env.BUNDLES.head(`icon:${id}`),
        ]);
        return [id, {
          shot: shot ? `${base}/shot/${id}` : null,
          icon: icon ? `${base}/icon/${id}` : null,
        }];
      }),
    ),
  );

  const hydrate = (id) => {
    const a = chosen.get(id);
    return {
      id,
      url: `${base}/a/${id}`,
      ...images.get(id),
      opens: opens.get(id) || 0,
      // The owner principal is for the hub's checks, not the gallery: an
      // account owner is a random account id that is nobody's business.
      meta: (({ owner, ...rest }) => rest)(a.meta),
    };
  };

  if (payload.results) payload.results = payload.results.map(hydrate);
  else for (const s of payload.shelves) s.apps = s.apps.map(hydrate);

  // `apps` stays for older Studios, which read a flat list and know nothing
  // about shelves. Dropping it would break every copy already installed.
  // De-duplicated by id, because one app can sit on both "new" and its
  // category shelf and an old client would draw it twice.
  payload.apps = payload.results
    || [...new Map(payload.shelves.flatMap((s) => s.apps).map((a) => [a.id, a])).values()];
  return json(payload);
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

/// A trimmed, cached copy of the two GitHub answers the website reads. The
/// Cache API holds it for ten minutes (no KV, no budget spent), which keeps
/// the hub far inside GitHub's unauthenticated limit.
async function githubMirror(pathname, request) {
  const cache = caches.default;
  const key = new Request(new URL(pathname, request.url).toString(), { method: "GET" });
  const hit = await cache.match(key);
  if (hit) return hit;
  const upstream = pathname === "/repo"
    ? "https://api.github.com/repos/incyashraj/krate"
    : "https://api.github.com/repos/incyashraj/krate/releases/latest";
  const r = await fetch(upstream, { headers: { accept: "application/vnd.github+json", "user-agent": "krate-hub" } });
  if (!r.ok) return text("GitHub did not answer", 502);
  const d = await r.json();
  const body = pathname === "/repo"
    ? { stargazers_count: d.stargazers_count || 0 }
    : {
        tag_name: d.tag_name,
        assets: (d.assets || []).map((a) => ({ name: a.name, browser_download_url: a.browser_download_url, size: a.size })),
      };
  const res = new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json", "cache-control": "public, max-age=600" },
  });
  await cache.put(key, res.clone());
  return res;
}

// ------------------------------------------------------------ who is real
//
// The numbers a founder reads to know whether real people came, signed up
// and tried Krate. Everything here goes to Analytics Engine (env.USAGE) and
// NEVER to KV: counting must not be able to spend the budget sign-ups need
// (K-952, K-912).

/// A crawler, a link preview, a monitor or a script -- not a person.
function isBot(request) {
  const ua = request.headers.get("user-agent") || "";
  if (!ua) return true;
  if (request.cf && request.cf.verifiedBotCategory) return true;
  return /bot|crawl|spider|slurp|preview|facebookexternalhit|embedly|headless|lighthouse|pingdom|uptime|monitor|curl\/|wget|python|go-http|node-fetch|axios|java\/|okhttp|scrapy|puppeteer|playwright/i.test(ua);
}

/// Where a visit came from, as a bare host ("google.com"), never a URL.
/// Our own pages are "-": moving around krate.tech is not a source.
function refHost(ref) {
  try {
    const host = new URL(String(ref || "")).hostname.toLowerCase().replace(/^www\./, "");
    if (!host || host === "krate.tech" || host.endsWith(".krate.tech")) return "-";
    return host.slice(0, 64);
  } catch {
    return "-";
  }
}

/// A home or office connection, or a cloud network (CI, build farms, our
/// own Fly builder). Cloudflare names the network's owner on every request.
function netOf(request) {
  const org = String((request.cf && request.cf.asOrganization) || "");
  return /microsoft|azure|amazon|aws|google|digitalocean|hetzner|ovh|linode|akamai|oracle|alibaba|tencent|fly\.io|flyio|cloudflare|github|macstadium|scaleway|vultr|contabo|leaseweb|choopa/i.test(org)
    ? "cloud"
    : "home";
}

/// One visitor for one day: a salted hash of the connection and browser,
/// with the salt and the day inside it. No cookie, nothing stored on the
/// device, and it cannot follow anyone past midnight UTC -- the way
/// privacy-first analytics count uniques. The raw address is never kept.
async function visitorHash(request, env) {
  const day = new Date().toISOString().slice(0, 10);
  const ip = request.headers.get("cf-connecting-ip") || "";
  const ua = request.headers.get("user-agent") || "";
  const data = new TextEncoder().encode(`${env.VISITOR_SALT || "krate"}|${day}|${ip}|${ua}`);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", data));
  return [...digest.slice(0, 12)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// A page view or a download click from krate.tech.
async function noteView(request, env, body) {
  if (!env.USAGE) return;
  const page = String(body.page || "/").slice(0, 64);
  const event = body.event === "download" ? "download" : "view";
  const os = ["mac", "windows", "linux"].includes(body.os) ? body.os : "-";
  env.USAGE.writeDataPoint({
    // blob1 kind, blob2 page, blob3 source host, blob4 country,
    // blob5 human|bot, blob6 view|download, blob7 download os
    blobs: [
      "view",
      page,
      refHost(body.ref),
      String((request.cf && request.cf.country) || "-"),
      isBot(request) ? "bot" : "human",
      event,
      os,
    ],
    doubles: [1],
    indexes: [await visitorHash(request, env)],
  });
}

/// Record how a sign-in step or a make attempt ended, then hand the
/// response on untouched. A step that throws is recorded and re-thrown.
/// This is how the desk can say "5 people tried to sign in, 2 finished"
/// -- the failed attempts of 2026-09-29 left no trace anywhere.
async function noteStep(env, kind, step, pending) {
  const record = (outcome, status) => {
    try {
      if (env.USAGE) {
        env.USAGE.writeDataPoint({ blobs: [kind, step, outcome, String(status)], doubles: [1], indexes: [kind] });
      }
    } catch {}
  };
  let res;
  try {
    res = await pending;
  } catch (e) {
    record("error", 500);
    throw e;
  }
  const failed = res.headers && typeof res.headers.get === "function" ? res.headers.get("x-krate-signin") : null;
  if (failed) record(failed === "failed" ? "error" : "refused", failed === "failed" ? 502 : 400);
  else record(res.status >= 500 ? "error" : res.status >= 400 ? "refused" : "ok", res.status);
  return res;
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
        // Which published app was opened, when it was one from the gallery.
        // A hash, never a name, and only for apps that are public already --
        // it says "this listing was opened", which is the same fact the
        // listing itself makes public. Absent for everything else, so
        // opening a file somebody sent you privately is not counted here.
        String(event.app || "").match(/^[0-9a-f]{6,64}$/) ? event.app : "-",
        // Where it came from: a home or office connection, or a cloud
        // network (CI runners, build farms, our own Fly builder). CI replays
        // opened 17,345 apps on a single day, which buried every real person
        // in the counts; this is how the dashboard tells them apart.
        netOf(request),
      ],
      doubles: [1],
      indexes: [id],
    });
  }
  return text("ok");
}

/// How often each published app was opened, for the gallery's shelves.
///
/// Analytics Engine, not KV, and that is not a preference: the first version
/// of `usage` did a read-modify-write per ping and one busy day exhausted the
/// free tier's puts, which took publishing down with it because they share a
/// namespace (see the note in `usage`). A counter that can break the product
/// is worse than no counter.
///
/// Returns a Map of app id -> opens in the window. Empty on any failure --
/// the gallery must still render when the analytics API is unreachable, and
/// a shelf that quietly falls back to "newest" is better than an error page.
async function openCounts(env, days = 30) {
  if (!env.ACCOUNT_ID || !env.ANALYTICS_TOKEN) return new Map();
  const since = new Date(Date.now() - days * 86400_000).toISOString().slice(0, 10);
  // blob1 is the action and blob8 is the app id; see writeDataPoint above.
  const sql = `
    SELECT blob8 AS app, SUM(_sample_interval) AS opens
    FROM krate_usage
    WHERE blob1 = 'open' AND blob8 != '-' AND timestamp >= toDateTime('${since} 00:00:00')
    GROUP BY app ORDER BY opens DESC LIMIT 200`;
  try {
    const r = await fetch(
      `https://api.cloudflare.com/client/v4/accounts/${env.ACCOUNT_ID}/analytics_engine/sql`,
      { method: "POST", headers: { Authorization: `Bearer ${env.ANALYTICS_TOKEN}` }, body: sql },
    );
    if (!r.ok) return new Map();
    const out = await r.json();
    return new Map((out.data || []).map((row) => [row.app, Number(row.opens) || 0]));
  } catch (_) {
    return new Map();
  }
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

  // How many machines were active each day. This needs its own query: the
  // one above groups by day, action and outcome, so its DISTINCT is per
  // group and summing those would count a machine once per action it took.
  //
  // Without this number every count above is a numerator with no
  // denominator, and "38,356 opens" cannot be told apart from one CI runner
  // in a loop. It is the same dataset and the same index -- no new
  // collection, just a count nobody was asking for (K-243). The old
  // active_installs_by_day came from KV keys that stopped being written on
  // 2026-08-10 and has been frozen ever since.
  const activeSql =
    "SELECT toDate(timestamp) AS day, count(DISTINCT index1) AS machines " +
    "FROM krate_usage " +
    `WHERE timestamp > now() - INTERVAL '${days}' DAY ` +
    "GROUP BY day ORDER BY day";
  const activeRes = await fetch(
    `https://api.cloudflare.com/client/v4/accounts/${env.CF_ACCOUNT_ID}/analytics_engine/sql`,
    {
      method: "POST",
      headers: { Authorization: `Bearer ${env.CF_ANALYTICS_TOKEN}` },
      body: activeSql,
    },
  );
  const activeByDay = {};
  if (activeRes.ok) {
    const activeBody = await activeRes.json();
    for (const row of activeBody.data || []) {
      activeByDay[row.day] = Number(row.machines);
    }
  }

  return {
    actions_by_day: byDay,
    distinct_installs: installs,
    active_machines_by_day: activeByDay,
  };
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
  // Writes only what changed (K-952). This runs on EVERY request that
  // carries a GitHub token -- Studio's session sync sends one on each save
  // and list -- and it used to PUT the user, the ident and the email ident
  // every time, changed or not. A day of one person working in Studio spent
  // the free plan's 1,000 KV writes, and every sign-in after that failed
  // before an account could exist. A returning, unchanged account now costs
  // zero writes; KV budget belongs to new people.
  const identKey = `ident:${provider}:${stableId}`;
  const emailKey = profile.email ? `ident:email:${profile.email.toLowerCase()}` : null;
  const identHeld = await env.APPS.get(identKey);
  let userId = identHeld;
  let emailHeld = null;
  if (emailKey) {
    emailHeld = await env.APPS.get(emailKey);
    // Same inbox, same person: unify across providers by verified email.
    if (!userId) userId = emailHeld;
  }
  let record;
  if (!userId) {
    userId = crypto.randomUUID();
    const shownEmail = profile.email || profile.unverified_email || "";
    record = {
      id: userId,
      created: Date.now(),
      name: profile.name || "",
      login: profile.login || (shownEmail ? shownEmail.split("@")[0] : ""),
      email: profile.email || "",
      avatar_url: profile.avatar_url || "",
      providers: [provider],
    };
    if (profile.verified_email) record.verified_emails = [profile.verified_email.toLowerCase()];
    if (profile.unverified_email && !profile.email) record.unverified_email = profile.unverified_email.toLowerCase();
    await env.APPS.put(`user:${userId}`, JSON.stringify(record));
    // A new person: the founder hears at once, with his welcome ready to
    // send from his own Gmail in one click (he writes each one himself:
    // the automatic note landed in Promotions). Never in the way of the
    // sign-in itself.
    await notifyFounderOfSignup(env, record).catch((error) =>
      console.error(`could not tell the founder about a new account: ${error}`),
    );
    // The automatic welcome, only when switched on (WELCOME_AUTO = "1"):
    // ten to twenty minutes from now, from the next ten-minute run.
    if (env.WELCOME_AUTO === "1" && record.email && (env.RESEND_API_KEY || env.GMAIL_APP_PASSWORD)) {
      await env.APPS.put(`welcome-due:${userId}`, String(record.created), {
        expirationTtl: 7 * 24 * 3600,
      });
    }
  } else {
    record = JSON.parse((await env.APPS.get(`user:${userId}`)) || "{}");
    const before = JSON.stringify(record);
    if (!(record.providers || []).includes(provider)) {
      record.providers = [...(record.providers || []), provider];
    }
    record.name = record.name || profile.name || "";
    record.avatar_url = record.avatar_url || profile.avatar_url || "";
    record.email = record.email || profile.email || "";
    // An inbox proved by this sign-in becomes one of the account's
    // organisation principals (IC-947). Written once, then free.
    if (profile.verified_email) {
      const proved = profile.verified_email.toLowerCase();
      const held = Array.isArray(record.verified_emails) ? record.verified_emails : [];
      if (!held.includes(proved)) record.verified_emails = [...held, proved];
    }
    if (JSON.stringify(record) !== before) {
      await env.APPS.put(`user:${userId}`, JSON.stringify(record));
    }
  }
  if (identHeld !== userId) await env.APPS.put(identKey, userId);
  if (emailKey && emailHeld !== userId) await env.APPS.put(emailKey, userId);
  // Which account a GitHub login signs in to, so an organisation notice
  // for github:<login> can reach that account's address (IC-948 D). Only a
  // GitHub sign-in writes it, and only when it changed.
  if (provider === "github" && profile.login) {
    const loginKey = `ghlogin:${String(profile.login).toLowerCase()}`;
    if ((await env.APPS.get(loginKey)) !== userId) await env.APPS.put(loginKey, userId);
  }
  return record;
}

// --------------------------------------------------------- founder welcome
//
// One plain-text note from the founder to each new person, ten to twenty
// minutes after they sign up. Plain on purpose: a founder's note that looks
// like a real email gets replies; a designed one reads as marketing.
//
// From Yashraj's own krate.tech address, which Resend can send for; replies
// go to his Gmail. It cannot be sent "from" the Gmail address itself: Gmail
// publishes a DMARC policy of reject, so mail claiming to be from gmail.com
// that Gmail did not send is refused or binned.

const WELCOME_CRON = "*/10 * * * *";
const WELCOME_AFTER_MS = 10 * 60 * 1000;
const WELCOME_FROM = "Yashraj Pardeshi <yashraj@krate.tech>";
const WELCOME_REPLY_TO = "incyashraj@gmail.com";
const WELCOME_SUBJECT = "You\u2019re in. One question from me";

/// The note, in the founder's words. `name` is the first word of the name
/// the provider gave; none (an email sign-in knows no name) is "there".
function welcomeText(name) {
  const first = (name || "").trim().split(/\s+/)[0] || "there";
  return `Hi ${first},

I\u2019m Yashraj, the person who builds Krate. Thanks for signing up.

I\u2019ll keep this short. Krate exists for one reason: making an app and sharing it should be as easy as sending a document. If it doesn\u2019t feel that way to you yet, I want to know.

So I have one question:

What\u2019s the first thing you want to make with Krate?

Just hit reply. It comes straight to me, not a support queue, and I read every reply myself.

If something breaks, confuses you, or feels slower than it should, tell me that too. Even the small stuff. Krate is still early, and what you tell me now decides what I build next. Real changes have already come from people like you.

If you\u2019d rather show me than write it, I\u2019m happy to jump on a quick call: https://calendly.com/incyashraj/30min

Thanks for trying something new with me.

Yashraj
Founder, Krate
krate.tech

P.S. If you get stuck, here\u2019s a 3-minute demo of making an app from scratch: https://youtu.be/No0Cu8dAewg
`;
}

/// Gmail's compose window, from the founder's own account, with the
/// welcome to `user` written and the krate.tech copy in Cc: one click, read,
/// change anything, Send.
function welcomeComposeUrl(user) {
  const q = new URLSearchParams({
    view: "cm",
    fs: "1",
    authuser: WELCOME_GMAIL,
    to: user.email || "",
    cc: WELCOME_CC,
    su: WELCOME_SUBJECT,
    body: welcomeText(user.name),
  });
  return `https://mail.google.com/mail/?${q.toString()}`;
}

/// Who to tell when someone signs up: FOUNDER_NOTIFY (comma-separated),
/// else both of the founder's inboxes.
function founderInboxes(env) {
  return (env.FOUNDER_NOTIFY || `${WELCOME_GMAIL},${WELCOME_CC}`)
    .split(",")
    .map((a) => a.trim())
    .filter(Boolean);
}

/// The moment an account is made: who, how, when, and the welcome ready
/// to send. Plain text so it reads at a glance on a phone.
async function notifyFounderOfSignup(env, user) {
  if (!env.RESEND_API_KEY) return;
  const who = user.name || user.login || user.email || "Someone";
  const how = (user.providers || []).join(", ") || "unknown";
  const at = new Date(user.created || Date.now()).toLocaleString("en-GB", {
    timeZone: "Asia/Singapore",
    dateStyle: "medium",
    timeStyle: "short",
  });
  const lines = [
    `${who} just made a Krate account.`,
    "",
    `Name:   ${user.name || "(none given)"}`,
    `Email:  ${user.email || "(none)"}`,
    `Signed in with: ${how}`,
    `When:   ${at} Singapore time`,
    "",
  ];
  if (user.email) {
    lines.push("Write them the welcome (opens Gmail, ready to send):", welcomeComposeUrl(user), "");
  }
  lines.push("Everyone, welcomed or not: https://hub.krate.tech/admin#welcome");
  await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.RESEND_API_KEY}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      from: "Krate <hello@krate.tech>",
      to: founderInboxes(env),
      subject: `New on Krate: ${who}`,
      text: lines.join("\n"),
    }),
  });
}

/// Send one welcome; null when it went, else why not.
///
/// Through Gmail itself when the founder's app password is set
/// (GMAIL_APP_PASSWORD): From his own Gmail, Cc his krate.tech address.
/// Sent through Resend, the same note was filed under Promotions -- the
/// founder's friend found it there -- because a bulk sender's servers sent
/// it; Gmail's own servers sending a person's own mail land it in Primary.
/// Otherwise through Resend, from his krate.tech address (it cannot claim
/// the Gmail address: Gmail's DMARC policy is reject).
async function sendWelcome(env, user) {
  const text = welcomeText(user.name);
  if (env.GMAIL_APP_PASSWORD) {
    const connect = env.SMTP_CONNECT || (await import("cloudflare:sockets")).connect;
    return smtpSend(connect, {
      user: WELCOME_GMAIL,
      password: env.GMAIL_APP_PASSWORD,
      from: `Yashraj Pardeshi <${WELCOME_GMAIL}>`,
      to: user.email,
      cc: WELCOME_CC,
      subject: WELCOME_SUBJECT,
      text,
    });
  }
  const res = await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      authorization: `Bearer ${env.RESEND_API_KEY}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      from: WELCOME_FROM,
      to: [user.email],
      reply_to: WELCOME_REPLY_TO,
      subject: WELCOME_SUBJECT,
      text,
    }),
  });
  if (res.ok) return null;
  return `resend ${res.status} ${await res.text().catch(() => "")}`;
}

const WELCOME_GMAIL = "incyashraj@gmail.com";
const WELCOME_CC = "yashraj@krate.tech";

/// One message through Gmail's SMTP server (smtp.gmail.com:465, TLS from
/// the first byte), AUTH PLAIN with an app password; null when Gmail took
/// it, else the step and Gmail's words. `connect` is cloudflare:sockets'
/// (a test passes its own).
async function smtpSend(connect, msg) {
  const socket = connect({ hostname: "smtp.gmail.com", port: 465 }, { secureTransport: "on" });
  const writer = socket.writable.getWriter();
  const reader = socket.readable.getReader();
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  let buffered = "";
  // One reply: lines until one whose fourth character is a space.
  const reply = async () => {
    for (;;) {
      const lines = buffered.split("\r\n");
      for (let i = 0; i < lines.length - 1; i++) {
        if (/^\d{3} /.test(lines[i])) {
          const code = Number(lines[i].slice(0, 3));
          const said = lines.slice(0, i + 1).join(" ");
          buffered = lines.slice(i + 1).join("\r\n");
          return { code, said };
        }
      }
      const { value, done } = await reader.read();
      if (done) return { code: 0, said: "the connection closed" };
      buffered += decoder.decode(value, { stream: true });
    }
  };
  const say = (line) => writer.write(encoder.encode(`${line}\r\n`));
  const step = async (line, want, name) => {
    if (line !== null) await say(line);
    const r = await reply();
    if (r.code !== want) throw new Error(`${name}: ${r.said}`);
  };
  const b64 = (t) => btoa(String.fromCharCode(...encoder.encode(t)));
  try {
    await step(null, 220, "greeting");
    await step("EHLO hub.krate.tech", 250, "hello");
    await step(`AUTH PLAIN ${b64(`\0${msg.user}\0${msg.password}`)}`, 235, "sign-in");
    await step(`MAIL FROM:<${msg.user}>`, 250, "sender");
    await step(`RCPT TO:<${msg.to}>`, 250, "recipient");
    if (msg.cc) await step(`RCPT TO:<${msg.cc}>`, 250, "copy");
    await step("DATA", 354, "data");
    // Base64 body lines of 76: the note has curly quotes, and 8-bit text
    // over SMTP is not something to count on.
    const body = b64(msg.text).replace(/.{1,76}/g, (m) => `${m}\r\n`);
    const date = new Date().toUTCString().replace("GMT", "+0000");
    const id = `<${crypto.randomUUID()}@hub.krate.tech>`;
    const headers = [
      `From: ${msg.from}`,
      `To: <${msg.to}>`,
      msg.cc ? `Cc: <${msg.cc}>` : null,
      `Subject: =?UTF-8?B?${b64(msg.subject)}?=`,
      `Date: ${date}`,
      `Message-ID: ${id}`,
      "MIME-Version: 1.0",
      "Content-Type: text/plain; charset=UTF-8",
      "Content-Transfer-Encoding: base64",
    ].filter(Boolean);
    await say(`${headers.join("\r\n")}\r\n\r\n${body}.`);
    const accepted = await reply();
    if (accepted.code !== 250) throw new Error(`send: ${accepted.said}`);
    await say("QUIT");
    return null;
  } catch (error) {
    return `gmail ${error.message}`;
  } finally {
    try {
      await socket.close();
    } catch {
      // Already closed by the server.
    }
  }
}

// For scripts/send-welcome-test.mjs: the exact note, sent by hand.
export { welcomeText, sendWelcome };

/// Send the welcomes that are due: each at most once (the account records
/// that it was welcomed before the due mark is dropped), twenty a run.
async function sendDueWelcomes(env, now = Date.now()) {
  if (!env.RESEND_API_KEY && !env.GMAIL_APP_PASSWORD) return { sent: 0 };
  const listed = await env.APPS.list({ prefix: "welcome-due:" });
  let sent = 0;
  for (const { name: key } of listed.keys || []) {
    if (sent >= 20) break;
    const created = Number((await env.APPS.get(key)) || "0");
    if (!created || now - created < WELCOME_AFTER_MS) continue;
    const userId = key.slice("welcome-due:".length);
    const user = JSON.parse((await env.APPS.get(`user:${userId}`)) || "null");
    if (!user || user.welcomed || !user.email) {
      await env.APPS.delete(key);
      continue;
    }
    const failed = await sendWelcome(env, user);
    if (failed) {
      // Left due: the next run tries again, until the mark expires in a week.
      console.error(`the welcome to a new account did not go: ${failed}`);
      continue;
    }
    user.welcomed = now;
    await env.APPS.put(`user:${userId}`, JSON.stringify(user));
    await env.APPS.delete(key);
    sent++;
  }
  return { sent };
}

async function newSession(env, userId, meta = {}) {
  const token = `krs_${crypto.randomUUID().replaceAll("-", "")}`;
  await env.APPS.put(`session:${token}`, userId, { expirationTtl: 90 * 24 * 3600 });
  // Indexed per user so support can revoke a person's sessions, and so the
  // person can see and sign out their own (account-sessions.js): the entry
  // holds the kind and the browser it came from, in the same write.
  await recordSession(env, userId, token, meta);
  return token;
}

/// Where a sign-in started, and the proof the browser that finishes it is
/// the one that began it (K-909).
///
/// A finished sign-in is a session handed to a page. Nothing used to tie it
/// to the browser that asked, so a link carrying SOMEONE ELSE's finished
/// sign-in -- the attacker's own, started and completed on their machine --
/// signed the visitor into the attacker's account, and everything they then
/// made landed where the attacker could read it.
///
/// So the page that starts a sign-in keeps a random `nonce` in its own
/// storage and sends it here; the hub carries it through the provider round
/// trip untouched and hands it back beside the session; /login/done accepts
/// the session only when the nonce is one this browser is holding. The
/// desktop does the same with `app_nonce` on its side of the krate:// hop.
///
/// Carried in the state record the start already writes -- `login:<state>`
/// or `email:<token>` -- so this costs no KV write of its own (K-912). The
/// hub does not insist on a nonce: the check that matters is the page's,
/// and an old page mid-deploy must still get a person signed in.
function signInStart(from, nonce, appNonce) {
  const ok = (v) => (/^[A-Za-z0-9_-]{16,64}$/.test(v || "") ? v : "");
  // `player:<hash>`: the browser Player asking for an organisation's app it
  // is about to open in this tab (IC-956). It comes back to that app's page
  // on this hub, never anywhere a caller names.
  // `player:org/<slug>/<app-id>`: the same for an organisation's app as it
  // is now (the address a phone keeps on its home screen).
  const player = /^player:(?:[0-9a-f]{64}|org\/[a-z][a-z0-9-]{1,38}\/[a-z0-9][a-z0-9._-]{0,127})$/.test(from || "") ? from : null;
  return { from: from === "app" ? "app" : player || "web", nonce: ok(nonce), app_nonce: ok(appNonce) };
}

/// What the company sign-in routes (org-sso.js) borrow from here: the same
/// accounts, sessions and K-909 delivery every other sign-in uses.
function signInDeps(request) {
  const label = sessionLabel(request && request.headers.get("user-agent"));
  // Company sign-in's sessions say so in the session list.
  const companySession = (env, userId, meta) => newSession(env, userId, meta || { kind: "company", label });
  return { SLUG, signInStart, deliver, ensureUser, newSession: companySession, orgAsk, noteOrgOf };
}

/// Read a stored start. Before K-909 the record was the bare word "app" or
/// "web"; a sign-in in flight across the deploy still has one of those.
function readSignInStart(raw) {
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object") return signInStart(parsed.from, parsed.nonce, parsed.app_nonce);
  } catch (e) { /* the old bare word */ }
  return signInStart(raw, "", "");
}

/// Deliver a finished sign-in to wherever it started. Everything rides in
/// the fragment, which never leaves the browser.
function deliver(start, user, token) {
  const hand = new URLSearchParams({
    token,
    login: user.login || "",
    name: user.name || "",
    avatar_url: user.avatar_url || "",
  });
  if (start.nonce) hand.set("nonce", start.nonce);
  if (start.app_nonce) hand.set("app_nonce", start.app_nonce);
  // The Player's own page on this hub (IC-956): a path, so it can only ever
  // be this hub's page for that app. The page takes the session only when
  // its own nonce comes back, as krate.tech/login/done does (K-909).
  if (typeof start.from === "string" && start.from.startsWith("player:")) {
    return new Response(null, {
      status: 302,
      headers: { location: `/run/${start.from.slice("player:".length)}#${hand.toString()}`, "cache-control": "no-store" },
    });
  }
  const suffix = start.from === "app" ? "?app=1" : "";
  return Response.redirect(`https://krate.tech/login/done/${suffix}#${hand.toString()}`, 302);
}

// ------------------------------------------------------------------- google

async function googleStart(url, env) {
  if (!env.GOOGLE_CLIENT_ID || !env.GOOGLE_CLIENT_SECRET) {
    return text("Google sign-in is not configured on this hub yet.", 503);
  }
  const start = signInStart(
    url.searchParams.get("from"),
    url.searchParams.get("nonce"),
    url.searchParams.get("app_nonce"),
  );
  const state = crypto.randomUUID();
  await env.APPS.put(`login:${state}`, JSON.stringify(start), { expirationTtl: 600 });
  const auth = new URL("https://accounts.google.com/o/oauth2/v2/auth");
  auth.searchParams.set("client_id", env.GOOGLE_CLIENT_ID);
  auth.searchParams.set("redirect_uri", `${env.PUBLIC_BASE}/login/google/callback`);
  auth.searchParams.set("response_type", "code");
  auth.searchParams.set("scope", "openid email profile");
  auth.searchParams.set("state", state);
  return Response.redirect(auth.toString(), 302);
}

async function googleCallback(url, env, request) {
  const state = url.searchParams.get("state") || "";
  const stored = await env.APPS.get(`login:${state}`);
  if (!stored) return signInFailed(url, "expired");
  await env.APPS.delete(`login:${state}`);
  const start = readSignInStart(stored);
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
  if (!result.id_token) return signInFailed(url, "failed");
  // tokeninfo validates the signature and audience for us.
  const info = await fetch(
    `https://oauth2.googleapis.com/tokeninfo?id_token=${encodeURIComponent(result.id_token)}`,
  );
  if (!info.ok) return text("Google's answer could not be verified.", 502);
  const claims = await info.json();
  if (claims.aud !== env.GOOGLE_CLIENT_ID) return text("Wrong audience.", 400);
  // Only an address Google says it verified may find or merge an account
  // (K-1085, IC-947). Before this, any Google account claiming an address
  // signed in as whoever already held it. tokeninfo answers the claim as
  // the string "true"; the ID token itself carries a boolean.
  const verified = claims.email_verified === true || claims.email_verified === "true";
  const email = claims.email ? String(claims.email).toLowerCase() : "";
  const user = await ensureUser(env, "google", claims.sub, {
    email: verified ? email : undefined,
    verified_email: verified ? email : undefined,
    unverified_email: verified ? undefined : email || undefined,
    name: claims.name,
    avatar_url: claims.picture,
  });
  return deliver(start, user, await newSession(env, user.id, { kind: "google", label: sessionLabel(request && request.headers.get("user-agent"), start.from) }));
}

// -------------------------------------------------------------------- email

/// The sign-in email, as HTML.
///
/// It used to be one line of plain text with a bare URL, which reads as
/// something a script sent rather than something a product did -- and for
/// somebody signing in by email it is the FIRST thing Krate ever shows them,
/// before the app, before the site.
///
/// A terminal, because that is what Krate is: the site's own hero is a
/// terminal window, and the people receiving this are developers. The
/// palette is lifted from docs/landing/site.css rather than invented --
/// --bg #0a0a0a, --panel #0f1012, --line #1f2228, --good #4ade80,
/// --accent #6291ff, --warn #f4dc74, and the same traffic-light dots.
///
/// Written for mail clients, not browsers, which is why it looks like 2005:
/// nested tables for layout, every style inline, no flexbox, no grid, no
/// external stylesheet. Gmail strips <style> blocks, Outlook renders through
/// Word, and anything clever silently collapses.
///
/// Dark mail has one extra trap on top of that: a client that forces its own
/// light theme will repaint backgrounds and leave the text colours alone. So
/// nothing here depends on the dark background for legibility -- every text
/// colour is readable on white too, and the button's label is white on a
/// solid cell that carries `bgcolor` as well as a CSS background, which is
/// what survives Outlook.
///
/// Three things it must do even when images are blocked, which is the
/// default in most clients:
///   - the button still reads as a button (colour on the anchor itself),
///   - the full URL is visible as text, so it can be copied,
///   - the logo has alt text that says the product's name.
function signInEmailHtml(link) {
  const BG = "#0a0a0a";
  const PANEL = "#0f1012";
  const LINE = "#1f2228";
  const GOOD = "#4ade80";
  const ACCENT = "#6291ff";
  const WARN = "#f4dc74";
  const DIM = "#8b8f98";
  const TEXT = "#e8eaed";
  const MONO =
    "SFMono-Regular,'Cascadia Code',Menlo,Consolas,'Liberation Mono',monospace";
  // Absolute: a mail client has no origin to resolve against.
  const LOGO = "https://krate.tech/krate-logo.png";
  const esc = (s) =>
    String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const href = esc(link);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="dark light">
<title>Sign in to Krate</title>
</head>
<body style="margin:0;padding:0;background:${BG};">
<!-- Preheader: the grey line a client shows beside the subject. Hidden in
     the body, or it prints twice. -->
<div style="display:none;max-height:0;overflow:hidden;opacity:0;">krate account login &mdash; your link is good for 15 minutes.</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:${BG};">
<tr><td align="center" style="padding:32px 16px;">
  <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:600px;">

    <!-- Wordmark above the window, so the sender is clear before anything
         else renders. -->
    <tr><td style="padding:0 4px 14px 4px;">
      <img src="${LOGO}" width="26" height="26" alt="Krate"
           style="display:inline-block;vertical-align:middle;border:0;">
      <span style="display:inline-block;vertical-align:middle;padding-left:9px;font:600 16px/1 ${MONO};color:${TEXT};">krate</span>
    </td></tr>

    <tr><td style="background:${PANEL};border:1px solid ${LINE};border-radius:12px;">

      <!-- Title bar: the same three dots as the site's terminal. -->
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
        <tr><td style="padding:11px 14px;border-bottom:1px solid ${LINE};">
          <table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr>
            <!-- The dots are CELLS, not spans.
                 As spans they came out as elongated ovals on Gmail mobile:
                 the &nbsp; inside inherits the body's 1.75 line-height, and
                 a mobile client ignores width/height on an inline-block, so
                 the box grew taller than it was wide. A td with width,
                 height and line-height:0 is the shape email clients
                 actually honour. Photographed on a phone before and after.

                 The hexes are the real macOS traffic lights and were never
                 wrong -- #FF5F57, #FEBC2E, #28C840. They read as dull in a
                 dark-mode client because it repaints around them, which is
                 why bgcolor is set as well as the CSS background: the
                 attribute is what survives that. -->
            <td width="12" height="12" bgcolor="#FF5F57" style="width:12px;height:12px;line-height:0;font-size:0;border-radius:6px;background:#FF5F57;">&#8203;</td>
            <td width="6" style="width:6px;line-height:0;font-size:0;">&#8203;</td>
            <td width="12" height="12" bgcolor="#FEBC2E" style="width:12px;height:12px;line-height:0;font-size:0;border-radius:6px;background:#FEBC2E;">&#8203;</td>
            <td width="6" style="width:6px;line-height:0;font-size:0;">&#8203;</td>
            <td width="12" height="12" bgcolor="#28C840" style="width:12px;height:12px;line-height:0;font-size:0;border-radius:6px;background:#28C840;">&#8203;</td>
            <td style="padding-left:14px;font:400 11.5px/1 ${MONO};color:${DIM};">krate &mdash; login</td>
          </tr></table>
        </td></tr>
      </table>

      <!-- Body: a session, read top to bottom. -->
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">
        <tr><td style="padding:20px 18px 0 18px;font:400 14px/1.75 ${MONO};color:${TEXT};">
          <span style="color:${GOOD};">$</span> <span style="color:${TEXT};">krate account login</span>
        </td></tr>

        <tr><td style="padding:6px 18px 0 18px;font:400 14px/1.75 ${MONO};color:${DIM};">
          <span style="color:${GOOD};">&#10003;</span> request received<br>
          <span style="color:${GOOD};">&#10003;</span> link signed, valid for <span style="color:${WARN};">15 minutes</span><br>
          <span style="color:${DIM};">&#8230;</span> waiting for you to confirm
        </td></tr>

        <!-- The button. bgcolor AND a CSS background: Outlook honours the
             attribute, everything else the style. -->
        <tr><td style="padding:22px 18px 0 18px;">
          <table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr>
            <td align="center" bgcolor="${ACCENT}" style="border-radius:8px;">
              <!-- A DARK label on the blue, and a border in the same blue.
                   Email clients that force a light theme repaint backgrounds
                   and leave text colours alone, so no single label colour is
                   safe on its own: white vanished when the cell was
                   repainted white, and dark-on-dark would vanish the other
                   way. Dark ink reads on the blue AND on a repainted light
                   cell, and the border keeps the shape either way. Checked
                   by rendering with every background forced to #ffffff. -->
              <a href="${href}"
                 style="display:inline-block;padding:13px 26px;border-radius:8px;background:${ACCENT};border:1px solid ${ACCENT};color:#0a0a0a;text-decoration:none;font:700 14px/1 ${MONO};">
                Sign in to Krate &rarr;
              </a>
            </td>
          </tr></table>
        </td></tr>

        <tr><td style="padding:20px 18px 0 18px;font:400 12.5px/1.6 ${MONO};color:${DIM};">
          <span style="color:${DIM};"># or paste this into your browser</span>
        </td></tr>

        <!-- word-break, or a long token pushes the card wider than a phone. -->
        <tr><td style="padding:8px 18px 0 18px;">
          <div style="padding:12px 13px;background:${BG};border:1px solid ${LINE};border-radius:8px;font:400 12.5px/1.7 ${MONO};color:${ACCENT};word-break:break-all;">
            ${href}
          </div>
        </td></tr>

        <tr><td style="padding:18px 18px 22px 18px;font:400 12.5px/1.7 ${MONO};color:${DIM};">
          <span style="color:${DIM};"># the link works once. if you did not ask to sign in,</span><br>
          <span style="color:${DIM};"># nothing has happened and you can ignore this.</span>
        </td></tr>
      </table>

    </td></tr>

    <tr><td style="padding:16px 4px 0 4px;">
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0"><tr>
        <td align="left" style="font:400 11.5px/1.5 ${MONO};color:${DIM};">
          build once. run anywhere.
        </td>
        <td align="right" style="font:400 11.5px/1.5 ${MONO};">
          <a href="https://krate.tech" style="color:${DIM};text-decoration:none;">krate.tech</a>
          <span style="color:${LINE};">&nbsp;&middot;&nbsp;</span>
          <a href="https://krate.tech/docs/" style="color:${DIM};text-decoration:none;">docs</a>
        </td>
      </tr></table>
    </td></tr>

  </table>
</td></tr>
</table>
</body>
</html>`;
}

async function emailStart(request, env) {
  if (!env.RESEND_API_KEY) {
    return text("Email sign-in is not configured on this hub yet.", 503);
  }
  const { email, from, nonce, app_nonce: appNonce } = await request.json().catch(() => ({}));
  if (!email || !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
    return text("That does not look like an email address.", 400);
  }
  const token = crypto.randomUUID().replaceAll("-", "");
  await env.APPS.put(
    `email:${token}`,
    JSON.stringify({ email: email.toLowerCase(), ...signInStart(from, nonce, appNonce) }),
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
      // Both parts, always. `text` is not a fallback nobody sees: plenty of
      // clients are set to plain text, and a mail with only HTML is the one
      // that lands in spam. The link is spelled out in both, so the mail
      // works even when images and styles are stripped.
      text: `Click to sign in to Krate:\n\n${link}\n\nThe link works once and expires in 15 minutes. If you did not ask for this, ignore it.`,
      html: signInEmailHtml(link),
    }),
  });
  if (!sent.ok) {
    // Say what the mail provider actually said.
    //
    // This threw the reason away and returned "Try again", which is advice
    // for a blip and useless for a misconfiguration -- and the failure here
    // is almost always a misconfiguration, because a send either works every
    // time or never. It never worked: SPF for krate.tech authorises
    // Hostinger and not Resend, so every send was refused and the only
    // person who could ever see why was reading Cloudflare's logs.
    //
    // An outside user hit exactly this. He could not sign in with GitHub,
    // Google or email, gave up, and told the founder by hand. The email
    // button was the one that was genuinely broken, and it said nothing.
    const why = await sent.text().catch(() => "");
    // The reason goes to the LOG, in full. That is where it is useful and
    // where it is safe.
    console.error(`resend refused the sign-in email: ${sent.status} ${why}`);
    // The PERSON gets one sentence they can act on.
    //
    // This used to pass the vendor's body straight through, and adversarial
    // testing showed what that means: an over-long address made the page
    // print `{"statusCode":422,"name":"validation_error","message":"The
    // email address length is more than 320 characters long."}` -- raw JSON,
    // naming our mail provider, to somebody who only wanted to sign in. It
    // also broke the layout at 375px, because that blob contains a 58
    // character unbreakable token.
    //
    // Naming the provider is the part that is not ours to give away. A
    // status code is enough for us to find the log line.
    return text(
      `The sign-in email could not be sent. Use Continue with GitHub or ` +
        `Google, which do not depend on mail. (Reference: ${sent.status})`,
      502,
    );
  }
  return json({ sent: true });
}

async function emailVerify(url, env, request) {
  const token = url.searchParams.get("t") || "";
  const stored = await env.APPS.get(`email:${token}`);
  if (!stored) return signInFailed(url, "used");
  await env.APPS.delete(`email:${token}`);
  const record = JSON.parse(stored);
  // The link reached this inbox, so the inbox is proved (IC-947).
  const user = await ensureUser(env, "email", record.email, { email: record.email, verified_email: record.email });
  const start = signInStart(record.from, record.nonce, record.app_nonce);
  return deliver(start, user, await newSession(env, user.id, { kind: "email", label: sessionLabel(request && request.headers.get("user-agent"), start.from) }));
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
  // The page's nonce rides along in the same record (K-909).
  const start = signInStart(
    url.searchParams.get("from"),
    url.searchParams.get("nonce"),
    url.searchParams.get("app_nonce"),
  );
  // The state ties the callback to this start. Ten minutes is enough to type
  // a password and approve; an unused state simply expires.
  const state = crypto.randomUUID();
  await env.APPS.put(`login:${state}`, JSON.stringify(start), { expirationTtl: 600 });

  const auth = new URL("https://github.com/login/oauth/authorize");
  auth.searchParams.set("client_id", GITHUB_CLIENT_ID);
  auth.searchParams.set("redirect_uri", `${env.PUBLIC_BASE}/login/callback`);
  auth.searchParams.set("scope", "read:user");
  auth.searchParams.set("state", state);
  return Response.redirect(auth.toString(), 302);
}

async function loginCallback(url, env, request) {
  const state = url.searchParams.get("state") || "";
  const code = url.searchParams.get("code") || "";
  const stored = await env.APPS.get(`login:${state}`);
  if (!stored) return signInFailed(url, "expired");
  // One shot: a replayed callback with the same state gets the line above.
  await env.APPS.delete(`login:${state}`);
  const start = readSignInStart(stored);

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
  if (!result.access_token) return signInFailed(url, "failed");

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
  // same address lands on this same user. Indexed once, so the person can
  // see this sign-in and sign it out (account-sessions.js): the one write a
  // GitHub sign-in adds.
  try {
    await recordSession(env, account.id, result.access_token, {
      kind: "github",
      label: sessionLabel(request && request.headers.get("user-agent"), start.from),
      ttl: GITHUB_INDEX_TTL_SECONDS,
    });
  } catch (e) {
    // The list is a convenience; the sign-in must not fail on it.
  }
  return deliver(start, { ...account, login: profile.login }, result.access_token);
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
  // A caller's bad body is a 400, never a 500.
  //
  // This was a bare `await request.json()`: an empty or malformed body threw
  // a SyntaxError that escaped to the top-level handler and answered "500
  // something went wrong on our side". It was not our side -- and it is
  // reachable by anyone, with no auth and no cost. Proved live:
  //
  //   curl -X POST https://hub.krate.tech/auth/poll           -> 500
  //   curl -X POST -d 'notjson' https://hub.krate.tech/auth/poll -> 500
  //
  // The same `.catch(() => ({}))` every other handler in this file already
  // uses. A 500 is a claim that WE broke; it should be true when we say it.
  const body = await request.json().catch(() => null);
  const device_code = body && body.device_code;
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
  // Indexed for the person's session list when their account is known
  // (one write per sign-in; account-sessions.js). A first-ever sign-in from
  // the desktop has no account yet, and is listed only while it is in use.
  try {
    const userId = profile.login ? await env.APPS.get(`ghlogin:${String(profile.login).toLowerCase()}`) : null;
    if (userId) {
      await recordSession(env, userId, result.access_token, {
        kind: "github",
        label: sessionLabel(request.headers.get("user-agent"), "app"),
        ttl: GITHUB_INDEX_TTL_SECONDS,
      });
    }
  } catch (e) {
    // The list is a convenience; the sign-in must not fail on it.
  }

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
/// The login a test token stands for, or null (IC-946).
///
/// The organisation proof needs two or three people signed in at once
/// without spending real GitHub accounts, so `wrangler dev` can be given
/// KRATE_TEST_IDENTITIES as `token=login,token=login`. It is honoured only
/// when that variable is set AND the hub's own address is this machine's
/// loopback -- the production hub is https://hub.krate.tech, so even a
/// variable set there by mistake grants nothing. Absent, every token goes
/// to GitHub as before, which refuses one it did not issue.
function testIdentity(token, env) {
  const map = env.KRATE_TEST_IDENTITIES;
  if (!map) return null;
  if (!/^http:\/\/(127\.0\.0\.1|localhost|\[::1\])(:\d+)?\/?$/.test(String(env.PUBLIC_BASE || ""))) return null;
  for (const pair of String(map).split(",")) {
    const at = pair.indexOf("=");
    if (at <= 0) continue;
    if (pair.slice(0, at).trim() === token) {
      const who = pair.slice(at + 1).trim().toLowerCase();
      // `email:<address>` stands for an account that proved that inbox, as
      // an email-link sign-in does: its `login` is the part before the @,
      // exactly like a real one, so the proof can show that part is never
      // taken for a GitHub login (IC-947).
      if (who.startsWith("email:")) {
        const email = who.slice("email:".length);
        if (!EMAIL.test(email)) return null;
        return { login: email.split("@")[0], name: "", avatar_url: "", github: false, id: `test-${email}`, emails: [email] };
      }
      const login = who.startsWith("github:") ? who.slice("github:".length) : who;
      return /^[a-z0-9](?:[a-z0-9-]{0,38})$/.test(login)
        ? { login, name: "", avatar_url: "", github: true, emails: [] }
        : null;
    }
  }
  return null;
}

/// Every principal a verified identity holds (IC-947): `github:<login>`
/// only when GitHub vouched for the login, `email:<address>` for each inbox
/// the account proved. A Krate session's `login` -- the part of an address
/// before the @ -- is never one of them (K-1085).
function principalsOf(identity) {
  if (!identity) return [];
  const out = [];
  if (identity.github === true && identity.login) out.push(`github:${String(identity.login).toLowerCase()}`);
  for (const email of identity.emails || []) {
    const e = String(email).toLowerCase();
    if (EMAIL.test(e)) out.push(`email:${e}`);
  }
  // `sso:<slug>` when this session came through that organisation's
  // company sign-in. Not a principal: a store reads it only to answer
  // `sso-required` (org-sso.js).
  if (out.length) out.push(...ssoMarks(identity));
  return [...new Set(out)];
}

async function verifyGitHub(request, env) {
  const auth = request.headers.get("authorization") || "";
  const token = auth.replace(/^Bearer\s+/i, "").trim();
  if (!token) return null;

  // Stubbed identities for a local `wrangler dev` proof only (IC-946). They
  // stand in for GitHub logins, so they count as vouched.
  const stubbed = testIdentity(token, env);
  if (stubbed) return stubbed;

  // A Krate session (Google or email sign-in) is as good as a GitHub token
  // for the account it belongs to -- but its `login` is only the part of an
  // email address before the @, which anyone can choose. So it is never a
  // GitHub login: `github` is false, and nothing that trusts a GitHub login
  // (KRATE_ADMINS above all) may accept it (K-1085).
  if (token.startsWith("krs_")) {
    const userId = await env.APPS.get(`session:${token}`);
    if (!userId) return null;
    const record = JSON.parse((await env.APPS.get(`user:${userId}`)) || "null");
    if (!record) return null;
    // The inboxes this account proved (an email link, or Google with
    // email_verified): its organisation principals (IC-947). `id` is the
    // account's random id: what it owns public listings by (K-1085).
    return {
      login: record.login,
      name: record.name,
      avatar_url: record.avatar_url,
      github: false,
      id: userId,
      emails: Array.isArray(record.verified_emails) ? record.verified_emails : [],
      // The organisations whose company sign-in made this session (IC-948).
      sso: await ssoSlugsFor(record, token),
    };
  }

  const cacheKey = `token:${await sha256Hex(new TextEncoder().encode(token))}`;
  const cached = await env.APPS.get(cacheKey);
  // Only this function writes the cache, and only after GitHub vouched.
  if (cached) return { ...JSON.parse(cached), github: true };

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
  return { ...identity, github: true };
}

// ------------------------------------------------------------- big files

/* The big-file path (IC-945): an app too large for the publish door's one
 * 5 MiB request -- Veloren is 370 MB -- uploaded in parts and judged on the
 * build service, because a Worker cannot hold it.
 *
 *   POST /publish/big/start            sign-in, allowlist, quota; opens an
 *                                      R2 multipart upload; answers a token
 *   PUT  /publish/big/<id>/part/<n>    one part, streamed into R2
 *   POST /publish/big/<id>/finish      completes it; the build service judges
 *   GET  /publish/big/<id>             where it is: uploading, judging,
 *                                      listed (with its URL) or refused
 *   GET  /internal/big/<id>/bytes      the build service reads the upload
 *   POST /internal/big/<id>/verdict    and answers with the hub's judgement
 *
 * Nothing is listed or served before a passing verdict: the upload lives at
 * big/<id>, which no public route names, and /a/<hash> reaches it only
 * through the `bigkey:<hash>` record the verdict writes. The token carries
 * everything a part needs, so a part costs no KV operation at all.
 */
const BIG_PART_BYTES = 64 * 1024 * 1024;
const BIG_MAX_BYTES = {
  bundle: 1024 * 1024 * 1024,
  source: 512 * 1024 * 1024,
  notices: 1024 * 1024,
};
const BIG_PER_LOGIN_PER_DAY = 3;
const BIG_ALL_PER_DAY = 10;
const BIG_LIVE_PER_LOGIN = 5;
const BIG_KEEP_VERSIONS = 2;
// R2 aborts an unfinished multipart upload after seven days; the record
// outlives it by one so the status can still say what happened.
const BIG_RECORD_TTL_SECONDS = 8 * 24 * 60 * 60;
const BIG_TOKEN_SECONDS = 2 * 24 * 60 * 60;

function bigOn(env) {
  return Boolean(env.KRATE_BUILDER_SECRET);
}

/// Who may use the big tier while it is new: GitHub logins named in
/// BIG_PUBLISHERS (comma-separated). Empty means nobody. The name is a
/// GitHub login, so only a login GitHub vouched for matches it: an email
/// account called incyashraj@anything is not incyashraj (K-1085).
function bigAllowed(env, identity) {
  if (!identity || identity.github !== true) return false;
  const login = identity.login;
  return String(env.BIG_PUBLISHERS || "")
    .split(",")
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean)
    .includes(String(login || "").toLowerCase());
}

function b64url(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function unb64url(text) {
  const s = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(s, (c) => c.charCodeAt(0));
}

/// The upload token's key: derived from the builder secret, labelled, so
/// it needs no secret of its own and cannot be confused with the builder's.
async function bigTokenKey(env) {
  const base = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(env.KRATE_BUILDER_SECRET || ""),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const derived = await crypto.subtle.sign("HMAC", base, new TextEncoder().encode("krate big upload v1"));
  return crypto.subtle.importKey("raw", derived, { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
    "verify",
  ]);
}

async function signBigToken(env, claims) {
  const body = b64url(new TextEncoder().encode(JSON.stringify(claims)));
  const sig = await crypto.subtle.sign("HMAC", await bigTokenKey(env), new TextEncoder().encode(body));
  return `${body}.${b64url(new Uint8Array(sig))}`;
}

/// The claims of a valid, unexpired token for upload `id`, or null.
async function readBigToken(request, env, id) {
  const raw = (request.headers.get("x-krate-upload") || "").trim();
  const dot = raw.indexOf(".");
  if (dot <= 0) return null;
  const body = raw.slice(0, dot);
  let sig;
  try {
    sig = unb64url(raw.slice(dot + 1));
  } catch (_) {
    return null;
  }
  const ok = await crypto.subtle.verify(
    "HMAC",
    await bigTokenKey(env),
    sig,
    new TextEncoder().encode(body),
  );
  if (!ok) return null;
  let claims;
  try {
    claims = JSON.parse(new TextDecoder().decode(unb64url(body)));
  } catch (_) {
    return null;
  }
  if (!claims || claims.id !== id) return null;
  if (!(claims.exp > Math.floor(Date.now() / 1000))) return null;
  return claims;
}

function partLength(claims, n) {
  return n < claims.parts ? claims.part_size : claims.size - claims.part_size * (claims.parts - 1);
}

function today() {
  return new Date().toISOString().slice(0, 10);
}

async function readCount(env, key) {
  const raw = await env.APPS.get(key);
  const n = Number(raw || 0);
  return Number.isFinite(n) ? n : 0;
}

/// How many big listings this login has live: the kept versions of each
/// of their big apps.
async function bigLive(env, login) {
  const listing = await env.APPS.list({ prefix: `bigversions:${login}:` });
  let n = 0;
  for (const key of listing.keys || []) {
    try {
      n += JSON.parse((await env.APPS.get(key.name)) || "[]").length;
    } catch (_) {}
  }
  return n;
}

async function bigStart(request, env) {
  if (!bigOn(env)) return text("the big-file path is not switched on here", 503);
  const identity = await verifyGitHub(request, env);
  if (!identity) {
    return text("publishing needs a GitHub sign-in -- run `krate publish` and it will ask", 401);
  }
  if (!bigAllowed(env, identity)) {
    return text(
      "apps over 5 MiB go through the big-file path, which is open to a few publishers " +
        "while it is new -- write to the Krate team to be added",
      403,
    );
  }
  const ask = await readJson(request, 16384);
  if (!ask) return text("a JSON body, please", 400);
  const kind = String(ask.kind || "bundle");
  if (!(kind in BIG_MAX_BYTES)) return text("kind is bundle, source or notices", 400);
  const size = Number(ask.size);
  if (!Number.isInteger(size) || size <= 0) return text("size is a whole number of bytes", 400);
  if (size > BIG_MAX_BYTES[kind]) {
    return text(
      kind === "bundle"
        ? `this app is ${size} bytes; Krate Cloud takes apps up to ${BIG_MAX_BYTES.bundle} bytes. ` +
            "It still opens from the file on a desktop."
        : `a ${kind} file is at most ${BIG_MAX_BYTES[kind]} bytes`,
      413,
    );
  }
  const sha256 = String(ask.sha256 || "").toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(sha256)) return text("sha256 is 64 hex characters", 400);

  // An organisation candidate over 5 MiB (IC-946): the same parts and the
  // same judging, landing as a candidate rather than a listing. Who may
  // publish there is asked before a part is accepted; the signature is
  // asked again with the judgement.
  let org = null;
  if (ask.org !== undefined && ask.org !== null && ask.org !== "") {
    org = String(ask.org).toLowerCase();
    if (!SLUG.test(org)) return orgError(400, "slug-invalid", org);
    if (kind !== "bundle") return text("an organisation takes the app itself, not a source or notices file", 400);
    const check = await orgAsk(env, org, {
      op: "check-candidate",
      principals: principalsOf(identity),
      args: { digest: sha256 },
    });
    if (check.status !== 200) return json(check.body, check.status);
  }

  let forHash = null;
  if (kind === "bundle") {
    if (org) {
      const holder = await bytesHolder(env, sha256);
      if (holder.exists && holder.org !== org) {
        return orgError(409, "bytes-held-elsewhere", org, "these exact bytes are already held outside this organisation");
      }
    } else {
      // These exact bytes are already listed: nothing to upload.
      const listed = await env.APPS.get(`app:${sha256}`);
      if (listed) {
        const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
        return json({ already: true, url: `${base}/a/${sha256}`, id: sha256 });
      }
    }
    // The tier's daily ceilings hold for an organisation's uploads too.
    const mine = await readCount(env, `bigday:${identity.login}:${today()}`);
    if (mine >= BIG_PER_LOGIN_PER_DAY) {
      return text(`${BIG_PER_LOGIN_PER_DAY} big apps a day per publisher -- try again tomorrow`, 429);
    }
    const all = await readCount(env, `bigday:all:${today()}`);
    if (all >= BIG_ALL_PER_DAY) {
      return text("Krate Cloud has taken its big uploads for today -- try again tomorrow", 429);
    }
    if ((await bigLive(env, identity.login)) >= BIG_LIVE_PER_LOGIN) {
      return text(
        `${BIG_LIVE_PER_LOGIN} big apps live per publisher -- unpublish one to add another`,
        429,
      );
    }
  } else {
    // Source and notices belong to a listing this person published.
    forHash = String(ask.for || "").toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(forHash)) return text("`for` names the app's 64-hex id", 400);
    const owner = await env.APPS.get(`app:${forHash}`);
    let prior = null;
    try {
      prior = owner ? JSON.parse(owner) : null;
    } catch (_) {}
    if (!prior || ownerOfRecord(prior) !== ownerOf(identity)) {
      return text("that app is not one you published", 403);
    }
  }

  const id = b64url(crypto.getRandomValues(new Uint8Array(16))).replace(/[-_]/g, "x");
  const key = `big/${id}`;
  let upload;
  try {
    upload = await env.BUNDLES.createMultipartUpload(key, {
      httpMetadata: { contentType: "application/octet-stream" },
      ...(org ? { customMetadata: { org } } : {}),
    });
  } catch (err) {
    return text("the store could not open an upload -- try again in a few minutes", 503);
  }
  // BIG_PART_BYTES in the environment only for tests; never under R2's
  // 5 MiB minimum or over what one request may carry.
  const partSize = Math.min(
    BIG_PART_BYTES,
    Math.max(5 * 1024 * 1024, Number(env.BIG_PART_BYTES) || BIG_PART_BYTES),
  );
  const parts = Math.max(1, Math.ceil(size / partSize));
  const record = {
    id,
    kind,
    for: forHash,
    org,
    principals: org ? principalsOf(identity) : undefined,
    login: identity.login,
    owner: ownerOf(identity),
    identity: {
      login: identity.login,
      name: identity.name || "",
      avatar_url: identity.avatar_url || "",
      github: identity.github === true,
      id: identity.id || undefined,
    },
    size,
    sha256,
    key,
    upload_id: upload.uploadId,
    parts,
    part_size: partSize,
    fields: {
      unlisted: Boolean(ask.unlisted),
      name: String(ask.name || "").slice(0, 80),
      description: String(ask.description || "").slice(0, 280),
      category: shelfOf(ask.category),
    },
    status: "uploading",
    created: Math.floor(Date.now() / 1000),
  };
  try {
    await env.APPS.put(`bigup:${id}`, JSON.stringify(record), {
      expirationTtl: BIG_RECORD_TTL_SECONDS,
    });
    if (kind === "bundle") {
      const mine = await readCount(env, `bigday:${identity.login}:${today()}`);
      await env.APPS.put(`bigday:${identity.login}:${today()}`, String(mine + 1), {
        expirationTtl: 2 * 24 * 60 * 60,
      });
      const all = await readCount(env, `bigday:all:${today()}`);
      await env.APPS.put(`bigday:all:${today()}`, String(all + 1), {
        expirationTtl: 2 * 24 * 60 * 60,
      });
    }
  } catch (_) {
    try {
      await upload.abort();
    } catch (_) {}
    return text("could not record the upload -- try again in a few minutes", 503);
  }
  const token = await signBigToken(env, {
    id,
    login: identity.login,
    key,
    upload_id: upload.uploadId,
    size,
    sha256,
    parts,
    part_size: partSize,
    exp: Math.floor(Date.now() / 1000) + BIG_TOKEN_SECONDS,
  });
  return json({ id, parts, part_size: partSize, token });
}

async function bigPart(request, env, id, n) {
  const claims = await readBigToken(request, env, id);
  if (!claims) return text("this upload's token is missing, wrong or expired -- start again", 401);
  if (!Number.isInteger(n) || n < 1 || n > claims.parts) {
    return text(`part numbers run from 1 to ${claims.parts}`, 400);
  }
  const want = partLength(claims, n);
  const declared = request.headers.get("content-length");
  if (declared === null) return text("each part says its length", 411);
  if (Number(declared) !== want) {
    return text(`part ${n} is ${want} bytes, not ${declared}`, 400);
  }
  if (!request.body) return text("an empty part", 400);
  try {
    const upload = env.BUNDLES.resumeMultipartUpload(claims.key, claims.upload_id);
    const part = await upload.uploadPart(n, request.body);
    return json({ part: n, etag: part.etag });
  } catch (err) {
    return text(`the store did not take part ${n} -- send it again`, 503);
  }
}

async function readBigRecord(env, id) {
  if (!/^[0-9a-zA-Z]{8,40}$/.test(id)) return null;
  try {
    return JSON.parse((await env.APPS.get(`bigup:${id}`)) || "null");
  } catch (_) {
    return null;
  }
}

async function writeBigRecord(env, record) {
  try {
    await env.APPS.put(`bigup:${record.id}`, JSON.stringify(record), {
      expirationTtl: BIG_RECORD_TTL_SECONDS,
    });
    return true;
  } catch (_) {
    return false;
  }
}

async function bigFinish(request, env, id) {
  const claims = await readBigToken(request, env, id);
  if (!claims) return text("this upload's token is missing, wrong or expired -- start again", 401);
  const record = await readBigRecord(env, id);
  if (!record) return text("no such upload", 404);
  if (record.status !== "uploading") return json(bigStatus(record, env));
  const ask = await readJson(request, 65536);
  const parts = ask && Array.isArray(ask.parts) ? ask.parts : null;
  if (!parts || parts.length !== claims.parts) {
    return text(`finish names all ${claims.parts} parts with their etags`, 400);
  }
  const seen = new Set();
  for (const p of parts) {
    if (!Number.isInteger(p.part) || p.part < 1 || p.part > claims.parts || seen.has(p.part)) {
      return text("each part once, numbered from 1", 400);
    }
    if (typeof p.etag !== "string" || !p.etag) return text("each part carries its etag", 400);
    seen.add(p.part);
  }
  let object;
  try {
    const upload = env.BUNDLES.resumeMultipartUpload(claims.key, claims.upload_id);
    object = await upload.complete(
      parts
        .slice()
        .sort((a, b) => a.part - b.part)
        .map((p) => ({ partNumber: p.part, etag: p.etag })),
    );
  } catch (err) {
    return text("the store could not put the parts together -- send any missing part and finish again", 409);
  }
  if (!object || object.size !== record.size) {
    try {
      await env.BUNDLES.delete(record.key);
    } catch (_) {}
    record.status = "refused";
    record.problem = `${object ? object.size : 0} bytes arrived where ${record.size} were declared`;
    await writeBigRecord(env, record);
    return json(bigStatus(record, env), 422);
  }
  record.status = "judging";
  record.finished = Math.floor(Date.now() / 1000);
  await writeBigRecord(env, record);
  await askBuilderToJudge(env, record);
  return json(bigStatus(record, env), 202);
}

/// Ask the build service to judge an upload. Best effort: the cron asks
/// again for anything still judging, so a sleeping or busy builder only
/// delays a listing, never loses it.
async function askBuilderToJudge(env, record) {
  const builder = (env.KRATE_BUILDER_URL || "").replace(/\/$/, "");
  if (!builder) return false;
  try {
    const res = await fetch(`${builder}/judge`, {
      method: "POST",
      headers: { "content-type": "application/json", "x-krate-builder": env.KRATE_BUILDER_SECRET || "" },
      body: JSON.stringify({ id: record.id, kind: record.kind, size: record.size, sha256: record.sha256 }),
      signal: AbortSignal.timeout(10_000),
    });
    return res.ok;
  } catch (_) {
    return false;
  }
}

function bigStatus(record, env) {
  const out = { id: record.id, kind: record.kind, status: record.status };
  if (record.org) {
    out.org = record.org;
    if (record.digest) out.digest = record.digest;
    if (record.state) out.state = record.state;
  }
  if (record.problem) out.problem = record.problem;
  if (record.url) out.url = record.url;
  if (record.status === "uploading") out.parts = record.parts;
  return out;
}

async function bigGetStatus(request, env, id) {
  const claims = await readBigToken(request, env, id);
  if (!claims) return text("this upload's token is missing, wrong or expired", 401);
  const record = await readBigRecord(env, id);
  if (!record) return text("no such upload", 404);
  return json(bigStatus(record, env));
}

/// The build service reads an upload it was asked to judge.
async function bigBytes(request, env, id) {
  if (!(await fromBuilder(request, env))) return text("not found", 404);
  const record = await readBigRecord(env, id);
  if (!record || record.status !== "judging") return text("nothing to judge", 404);
  const object = await env.BUNDLES.get(record.key);
  if (!object) return text("the upload is gone", 404);
  return new Response(object.body, {
    headers: { "content-type": "application/octet-stream", "content-length": String(object.size) },
  });
}

/// The build service's answer. On a pass the upload is listed through the
/// same admitJudged the publish door uses; on a refusal it is removed.
async function bigVerdict(request, env, id) {
  if (!(await fromBuilder(request, env))) return text("not found", 404);
  const record = await readBigRecord(env, id);
  if (!record) return text("no such upload", 404);
  if (record.status !== "judging") return json(bigStatus(record, env));
  const verdict = await readJson(request, 4 * 1024 * 1024);
  if (!verdict) return text("a JSON verdict, please", 400);

  const refuse = async (problem) => {
    try {
      await env.BUNDLES.delete(record.key);
    } catch (_) {}
    record.status = "refused";
    record.problem = problem;
    await writeBigRecord(env, record);
    return json(bigStatus(record, env));
  };
  if (String(verdict.sha256 || "") !== record.sha256) {
    return refuse("the bytes that arrived are not the ones the upload declared");
  }
  if (!verdict.ok) {
    // A bundle's problem is the validator's own sentence, which already
    // says what was wrong; a source or notices file is named.
    const problem = verdict.problem || "refused";
    return refuse(record.kind === "bundle" ? problem : `not a valid ${record.kind} file: ${problem}`);
  }

  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  if (record.kind === "bundle" && record.org) {
    // An organisation candidate (IC-946): never a listing. The address
    // record goes first, so a candidate never names bytes /a/ cannot find;
    // if the organisation refuses it, both go.
    if (!verdict.judgement) return refuse("the judgement was missing");
    const holder = await bytesHolder(env, record.sha256);
    if (holder.exists && holder.org !== record.org) {
      return refuse("these exact bytes are already held outside this organisation");
    }
    const head = await env.BUNDLES.head(record.key);
    if (!head || head.size !== record.size) return refuse("the upload could not be found in the store");
    const ours = !holder.exists;
    if (ours) {
      try {
        await env.APPS.put(`bigkey:${record.sha256}`, record.key);
      } catch (_) {
        return text("could not record where the app lives -- the build service will be asked again", 503);
      }
    }
    const added = await orgAsk(env, record.org, {
      op: "add-candidate",
      principals: record.principals || [],
      args: { ...candidateArgs(verdict.judgement, record.sha256, record.size, base), big: true },
    });
    if (added.status >= 300) {
      if (ours) {
        try {
          await env.APPS.delete(`bigkey:${record.sha256}`);
        } catch (_) {}
      }
      return refuse(`${added.body.error}: ${added.body.message}`);
    }
    if (!ours) {
      // The same bytes were already this organisation's; the copy that
      // just arrived is not needed.
      try {
        await env.BUNDLES.delete(record.key);
      } catch (_) {}
    }
    record.status = "candidate";
    record.digest = record.sha256;
    record.state = added.body.state;
    await writeBigRecord(env, record);
    return json(bigStatus(record, env));
  }
  if (record.kind === "bundle") {
    if (!verdict.judgement) return refuse("the judgement was missing");
    const response = await admitJudged(env, {
      // An upload from before owners were recorded was a BIG_PUBLISHERS GitHub login.
      identity: { github: true, ...record.identity },
      judgement: verdict.judgement,
      hash: record.sha256,
      size: record.size,
      fields: record.fields,
      extra: { big: true, blob: record.key },
      store: async () => {
        const head = await env.BUNDLES.head(record.key);
        if (!head || head.size !== record.size) {
          return { problem: "the upload could not be found in the store; nothing was published" };
        }
        try {
          await env.APPS.put(`bigkey:${record.sha256}`, record.key);
        } catch (_) {
          return { problem: "could not record where the app lives -- try again in a few minutes" };
        }
        return { size: head.size, sha256: record.sha256 };
      },
    });
    const answer = await response.clone().json().catch(() => null);
    if (!response.ok || !answer) {
      // A refusal in the contract's shape is said in its own words.
      if (answer && answer.error && answer.message) return refuse(answer.message);
      return refuse((await response.text().catch(() => "")) || "the listing could not be made");
    }
    if (answer.note && /already published/.test(answer.note)) {
      // Somebody else's listing of the same bytes stands; ours is not needed.
      try {
        await env.BUNDLES.delete(record.key);
      } catch (_) {}
    } else {
      await keepLatestBig(env, record.login, record.fields.name, record.sha256);
    }
    record.status = "listed";
    record.url = answer.url;
    await writeBigRecord(env, record);
    return json(bigStatus(record, env));
  }

  // Source or notices: attached to the listing they were declared for.
  const raw = await env.APPS.get(`app:${record.for}`);
  let meta = null;
  try {
    meta = raw ? JSON.parse(raw) : null;
  } catch (_) {}
  // An upload started before owners were recorded carries none; it was a
  // BIG_PUBLISHERS login, which is a GitHub login (the legacy default).
  const uploader = record.owner || `github:${String(record.login || "").toLowerCase()}`;
  if (!meta || ownerOfRecord(meta) !== uploader) return refuse("that app is no longer listed");
  const previous = meta[record.kind];
  meta[record.kind] = { key: record.key, size: record.size, sha256: record.sha256 };
  try {
    await env.APPS.put(`app:${record.for}`, JSON.stringify(meta));
  } catch (_) {
    return text("could not attach it -- the build service will be asked again", 503);
  }
  if (previous && previous.key && previous.key !== record.key) {
    try {
      await env.BUNDLES.delete(previous.key);
    } catch (_) {}
  }
  record.status = "listed";
  record.url = `${base}/a/${record.for}/${record.kind}`;
  await writeBigRecord(env, record);
  return json(bigStatus(record, env));
}

/// Keep the newest BIG_KEEP_VERSIONS versions of a big app; older ones'
/// bytes, address record and source go (a 370 MB version kept forever per
/// publish is the cost that matters here).
async function keepLatestBig(env, login, name, hash) {
  const key = `bigversions:${login}:${slugOf(name || "untitled")}`;
  let versions = [];
  try {
    versions = JSON.parse((await env.APPS.get(key)) || "[]");
  } catch (_) {}
  versions = [hash, ...versions.filter((h) => h !== hash)];
  const dropped = versions.slice(BIG_KEEP_VERSIONS);
  versions = versions.slice(0, BIG_KEEP_VERSIONS);
  try {
    await env.APPS.put(key, JSON.stringify(versions));
  } catch (_) {
    return;
  }
  for (const old of dropped) {
    try {
      const blob = await env.APPS.get(`bigkey:${old}`);
      if (blob) await env.BUNDLES.delete(blob);
      await env.APPS.delete(`bigkey:${old}`);
      const raw = await env.APPS.get(`app:${old}`);
      if (raw) {
        const meta = JSON.parse(raw);
        for (const kind of ["source", "notices"]) {
          if (meta[kind] && meta[kind].key) await env.BUNDLES.delete(meta[kind].key);
        }
        await env.APPS.delete(`app:${old}`);
      }
      await env.APPS.delete(`owner:${old}`);
    } catch (_) {}
  }
}

/// Remove what a big-tier listing keeps outside its content address: the
/// bytes at big/<id>, the record naming them, and any attached source or
/// notices (IC-945). A listing that is not big has none of these.
async function deleteBigParts(env, hash) {
  try {
    const blob = await env.APPS.get(`bigkey:${hash}`);
    if (blob) {
      await env.BUNDLES.delete(blob);
      await env.APPS.delete(`bigkey:${hash}`);
    }
    const raw = await env.APPS.get(`app:${hash}`);
    const meta = raw ? JSON.parse(raw) : null;
    for (const kind of ["source", "notices"]) {
      if (meta && meta[kind] && meta[kind].key) await env.BUNDLES.delete(meta[kind].key);
    }
    // No longer one of its publisher's live big apps.
    if (meta && meta.big) {
      const key = `bigversions:${meta.author_login}:${slugOf(meta.name || "untitled")}`;
      const versions = JSON.parse((await env.APPS.get(key)) || "[]").filter((h) => h !== hash);
      if (versions.length) await env.APPS.put(key, JSON.stringify(versions));
      else await env.APPS.delete(key);
    }
  } catch (_) {}
}

/// A listing's attached source or notices, served as a download.
async function fetchAttachment(hash, kind, env) {
  if (!/^[0-9a-f]{64}$/.test(hash)) return text("not found", 404);
  const raw = await env.APPS.get(`app:${hash}`);
  let meta = null;
  try {
    meta = raw ? JSON.parse(raw) : null;
  } catch (_) {}
  const attached = meta && meta[kind];
  if (!attached || !attached.key) return text("not found", 404);
  const object = await env.BUNDLES.get(attached.key);
  if (!object) return text("not found", 404);
  const slug = slugOf(meta.name || "app");
  const headers =
    kind === "source"
      ? {
          "content-type": "application/zstd",
          "content-disposition": `attachment; filename="${slug}-source.tar.zst"`,
        }
      : { "content-type": "text/plain; charset=utf-8" };
  return new Response(object.body, {
    headers: { ...headers, "content-length": String(object.size), "cache-control": "public, max-age=3600" },
  });
}

/// The cron's part: ask again about anything still judging after ten
/// minutes, and give up on anything judging for a day.
async function retryBigJudging(env) {
  if (!bigOn(env)) return;
  let listing;
  try {
    listing = await env.APPS.list({ prefix: "bigup:" });
  } catch (_) {
    return;
  }
  const now = Math.floor(Date.now() / 1000);
  for (const key of listing.keys || []) {
    const record = await readBigRecord(env, key.name.slice("bigup:".length));
    if (!record || record.status !== "judging") continue;
    const waited = now - (record.finished || record.created || now);
    if (waited > 24 * 60 * 60) {
      try {
        await env.BUNDLES.delete(record.key);
      } catch (_) {}
      record.status = "refused";
      record.problem = "the build service did not judge it within a day -- publish it again";
      await writeBigRecord(env, record);
    } else if (waited > 10 * 60) {
      await askBuilderToJudge(env, record);
    }
  }
}

// ------------------------------------------------------------------- helpers

/// A .krate is a zip carrying manifest.toml and code.wasm. This checks the
/// shape without unzipping: the local file headers name their entries in
/// plain bytes near the start, which is enough to reject something that is
/// not a bundle at all.
/// Read a request body a chunk at a time, giving up the moment it passes
/// `max` (IC-833, test 1831). Returns `{ body }` or `{ tooLarge: true }`.
/// Nothing past the ceiling is kept, and the stream is cancelled so the
/// rest of an oversized upload is never pulled.
async function readBounded(request, max) {
  if (!request.body) return { body: new Uint8Array(0) };
  const reader = request.body.getReader();
  const chunks = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      try {
        await reader.cancel();
      } catch (_) {
        // Cancelling is a courtesy to the sender; the refusal stands.
      }
      return { tooLarge: true };
    }
    chunks.push(value);
  }
  const body = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    body.set(chunk, at);
    at += chunk.byteLength;
  }
  return { body };
}

/// Why these bytes are not an archive the hub will hold, or null (K-309,
/// IC-833 test 1833). Judged from the central directory before any entry
/// is inflated, in the order a reader would meet the problems.
function archiveProblem(bytes) {
  if (bytes.length < 4) return "too short to be a zip";
  // "PK\x03\x04"
  if (!(bytes[0] === 0x50 && bytes[1] === 0x4b && bytes[2] === 0x03 && bytes[3] === 0x04)) {
    return "not a zip archive";
  }
  const { declared, entries } = zipDirectory(bytes);
  if (declared === null) {
    return "no central directory, so no manifest.toml or code.wasm can be found";
  }
  if (entries.length < declared) {
    // The directory claims more records than can be read. A real writer
    // that names one path twice lands here too, since the reader keys by
    // name (the record-count check crates/bundle makes for the same reason).
    return `declares ${declared} entries but only ${entries.length} can be read`;
  }
  if (entries.length > MAX_ENTRY_COUNT) {
    return `more than ${MAX_ENTRY_COUNT} entries`;
  }
  const seen = new Set();
  let declaredTotal = 0;
  for (const entry of entries) {
    const { name } = entry;
    if (!/^[\x20-\x7e]+$/.test(name)) {
      return `entry name is not plain ASCII: ${shortName(name)}`;
    }
    if (
      name.includes("\\") ||
      name.startsWith("/") ||
      /^[A-Za-z]:/.test(name) ||
      name.split("/").some((part, i, parts) => (part === "" && i !== parts.length - 1) || part === "." || part === "..")
    ) {
      return `unsafe entry path: ${shortName(name)}`;
    }
    if (name.split("/").length - 1 > MAX_PATH_DEPTH) {
      return `entry path nests deeper than ${MAX_PATH_DEPTH}: ${shortName(name)}`;
    }
    if (name.length > MAX_PATH_BYTES) {
      return `entry path longer than ${MAX_PATH_BYTES} bytes: ${shortName(name)}`;
    }
    if (entry.flags & 0x1) {
      return `encrypted entry: ${shortName(name)}`;
    }
    if (name.endsWith("/")) continue; // a directory record carries no content
    const logical = name.toLowerCase();
    if (seen.has(logical)) {
      return `names the same file twice: ${shortName(name)}`;
    }
    seen.add(logical);
    if (entry.size > MAX_ENTRY_BYTES) {
      return `entry ${shortName(name)} declares more than ${MAX_ENTRY_BYTES} bytes`;
    }
    declaredTotal += entry.size;
    if (declaredTotal > MAX_DECLARED_TOTAL_BYTES) {
      return `entries declare more than ${MAX_DECLARED_TOTAL_BYTES} bytes together`;
    }
  }
  return null;
}

function shortName(name) {
  return name.length > 80 ? `${name.slice(0, 40)}...${name.slice(-37)}` : name;
}

/// Put the bytes at their own digest and read them back before saying so
/// (IC-833, tests 1837 and 1840).
///
/// R2 is told the SHA-256 it must see, so bytes that do not hash to the
/// key are refused by the store itself. An object already at the key is
/// compared, never replaced: matching means the publish is idempotent, a
/// mismatch is a store that holds the wrong bytes and is reported rather
/// than papered over. An object stored before the digest was recorded is
/// written again under the same guard, which upgrades it. What the store
/// reports on read-back is the receipt; a receipt that disagrees with what
/// was sent removes the object and fails the publish.
async function storeBundle(env, hash, body, customMetadata) {
  const options = {
    httpMetadata: { contentType: "application/octet-stream" },
    sha256: hash,
    // An organisation's candidate carries `org` here, and /a/<hash> asks
    // that organisation before serving a byte (IC-946).
    ...(customMetadata ? { customMetadata } : {}),
  };
  const existing = await env.BUNDLES.head(hash);
  const existingSha = existing ? checksumHex(existing) : null;
  if (existingSha && existingSha !== hash) {
    return {
      problem:
        "the store holds bytes at this address that do not match it; not overwriting " +
        "-- report this",
    };
  }
  if (!existing || !existingSha) {
    try {
      await env.BUNDLES.put(hash, body, options);
    } catch (err) {
      return { problem: `the store refused the bytes: ${err && err.message ? err.message : err}` };
    }
  }
  const stored = await env.BUNDLES.head(hash);
  const storedSha = stored ? checksumHex(stored) : null;
  if (!stored || stored.size !== body.length || storedSha !== hash) {
    try {
      await env.BUNDLES.delete(hash);
    } catch (_) {
      // Best effort: an unverified object must not be listed either way.
    }
    return {
      problem:
        "the stored copy could not be verified (size or digest differs from what was " +
        "sent); nothing was published",
    };
  }
  return { size: stored.size, sha256: storedSha };
}

function checksumHex(object) {
  const raw = object.checksums && object.checksums.sha256;
  if (!raw) return null;
  return [...new Uint8Array(raw)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// ---------------------------------------------------------- reading the zip
//
// Enough of the zip format to find one named entry and read it: the end of
// central directory record at the tail, the central directory it points
// at, the local header each entry points at. Deflate is inflated with the
// runtime's own DecompressionStream, so there is no dependency and no
// hand-written inflater. Bounded: an entry larger than `max` bytes is
// refused rather than read, and a directory that points outside the body
// is treated as absent.

function u16(b, i) { return b[i] | (b[i + 1] << 8); }
function u32(b, i) { return (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0; }


function zipEntries(bytes) {
  return zipDirectory(bytes).entries;
}

/// The central directory as the archive declares it: `declared` is the
/// EOCD's record count (null when there is no EOCD at all), `entries` the
/// records that could actually be read, keyed by nothing -- duplicates are
/// the caller's to judge.
function zipDirectory(bytes) {
  // EOCD: signature 06054b50, at least 22 bytes, comment up to 65535.
  const floor = Math.max(0, bytes.length - 22 - 65535);
  let eocd = -1;
  for (let i = bytes.length - 22; i >= floor; i -= 1) {
    if (bytes[i] === 0x50 && bytes[i + 1] === 0x4b && bytes[i + 2] === 0x05 && bytes[i + 3] === 0x06) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) return { declared: null, entries: [] };
  const count = u16(bytes, eocd + 10);
  let at = u32(bytes, eocd + 16);
  const out = [];
  const dec = new TextDecoder();
  for (let n = 0; n < count && at + 46 <= bytes.length; n += 1) {
    if (u32(bytes, at) !== 0x02014b50) break;
    const flags = u16(bytes, at + 8);
    const method = u16(bytes, at + 10);
    const compressed = u32(bytes, at + 20);
    const size = u32(bytes, at + 24);
    const nameLen = u16(bytes, at + 28);
    const extraLen = u16(bytes, at + 30);
    const commentLen = u16(bytes, at + 32);
    const local = u32(bytes, at + 42);
    const name = dec.decode(bytes.subarray(at + 46, at + 46 + nameLen));
    out.push({ name, flags, method, compressed, size, local });
    at += 46 + nameLen + extraLen + commentLen;
  }
  return { declared: count, entries: out };
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

/* Free text a person typed -- an app's name, its description, a support
 * note -- in a header. A header carries ISO-8859-1 at most: a browser
 * refuses to send anything else ("String contains non ISO-8859-1 code
 * point"), and a CLI's raw UTF-8 arrives garbled. So a name like "Mom’s"
 * or "番茄钟" could not be published from the web at all (K-891). Senders
 * now percent-encode the value and say so with `x-krate-encoding: uri`;
 * without that header the value is taken as sent, which is what every
 * older client does. */
function textHeader(request, name) {
  const value = header(request, name);
  if (value && (request.headers.get("x-krate-encoding") || "").trim() === "uri") {
    try { return decodeURIComponent(value).trim(); } catch (e) { return value; }
  }
  return value;
}

/* ================= a developer's own name: alice.krate.tech ==============
 *
 * The hub has always kept a channel per published app --
 * `channel:<login>/<slug>`, mutable, 50 moves of history, rollback, moved
 * by that publisher and nobody else. What it never had was a face: nothing
 * linked it, there was no page for a person, and the gallery was one
 * global shelf. This serves that machinery on a name a developer can put
 * in their own README (K-849).
 *
 * One label only, deliberately. Cloudflare's free Universal SSL covers the
 * apex and exactly one level of subdomain, so `alice.krate.tech` is
 * covered and `beta.alice.krate.tech` is not. Environments belong in the
 * path, never in a second label.
 */

/// Which developer this hostname belongs to, or null for the hub itself.
///
/// Names that are part of the service can never be a person: a developer
/// called "hub" must not be able to answer for hub.krate.tech. Checked
/// against the same shape GitHub logins use, which is what the publisher
/// identity already is.
function developerHost(url, env) {
  const host = url.hostname.toLowerCase();
  const root = ((env.PUBLIC_BASE || "").replace(/^https?:\/\//, "").replace(/\/$/, "") || "hub.krate.tech")
    .split(".")
    .slice(-2)
    .join(".");
  if (!host.endsWith(`.${root}`)) return null;
  const label = host.slice(0, -(root.length + 1));
  // One label. A dotted prefix is not a developer and has no certificate
  // under the free wildcard, which covers exactly one level.
  //
  // Belt and braces: the charset test below already rejects a dot, so
  // removing this line changes no behaviour -- checked by sabotage. It
  // stays because the RULE is worth stating where a reader looks for it,
  // and because a future edit that widens the charset would otherwise
  // silently start serving pages on hostnames with no certificate.
  if (label.includes(".")) return null;
  const RESERVED = new Set([
    "hub", "www", "api", "app", "studio", "docs", "cloud", "builder",
    "admin", "mail", "smtp", "imap", "ns1", "ns2", "cdn", "assets",
    "status", "blog", "help", "support", "play", "open", "make", "login",
  ]);
  if (RESERVED.has(label)) return null;
  if (!/^[a-z0-9-]{1,39}$/.test(label) || label.startsWith("-") || label.endsWith("-")) {
    return null;
  }
  return label;
}

/// Serve one developer's namespace.
///
/// The same URL answers two callers, which is the trick the hub already
/// uses for bundles (classifyClient): a browser gets a page, a runtime
/// gets the bytes. So `krate run alice.krate.tech/notes` works and the
/// same address pasted into a browser explains itself.
async function developerRoute(request, url, login, env) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return text("not found", 404);
  }
  const parts = url.pathname.split("/").filter(Boolean);
  const wantsPage = classifyClient(request) !== "tool";

  if (parts.length === 0) {
    const apps = await developerApps(env, login);
    // A name nobody has published under is not a developer (K-856).
    //
    // The wildcard record means EVERY name reaches this worker, and this
    // used to render the empty state for all of them: nobody-here.krate.tech
    // answered 200 with "Apps by nobody-here" for a person who had never
    // signed in. "No apps yet" and "no such person" were the same answer,
    // so a real developer's page was indistinguishable from a stranger's --
    // and with robots.txt allowing everything, the set of indexable pages
    // asserting somebody is on Krate was infinite.
    //
    // Publishing is what creates a `channel:<login>/` key, so an empty
    // listing means nothing has ever been published under this name. That
    // is the test, because it needs no index: users are stored under an
    // internal id and there is no login -> user record to ask. The cost is
    // that somebody signed in who has not published yet has no page, which
    // is the right answer anyway -- there is nothing to show.
    if (!apps.length) {
      return wantsPage
        ? unknownDeveloperPage(login)
        : json({ error: "no such developer", login }, 404);
    }
    return wantsPage
      ? developerPage(login, apps, env)
      : json({
          login,
          // Names a tool can ask this host for: `<login>.krate.tech/<slug>`
          // only resolves through a channel. An app with no channel is
          // still listed, at the content address that does answer, rather
          // than under a slug that would 404.
          apps: apps.filter((a) => !a.href).map((a) => a.slug),
          published: apps.map((a) => ({
            name: a.name,
            id: a.current,
            url: a.href || `/${a.slug}`,
          })),
        });
  }
  if (parts.length !== 1) return text("not found", 404);

  const slug = parts[0].toLowerCase();
  if (!/^[a-z0-9-]{1,48}$/.test(slug)) return text("not found", 404);
  const stored = await readChannel(env, login, slug);
  const channel = stored && stored.current ? await settleChannel(env, stored, "gone") : null;
  if (!channel) {
    return wantsPage
      ? htmlPage(
          `Nothing here yet`,
          // A heading on the PAGE, not only in the tab. Verified live: the
          // body opened mid-sentence with nothing above it, so a person
          // following a dead link landed on a stray line of text.
          `<h1>Nothing here yet</h1>
           <p class="sub">${escapeForHtml(login)} has no app called <b>${escapeForHtml(slug)}</b>.</p>
           <p><a href="/">See what ${escapeForHtml(login)} has made</a></p>`,
          404,
        )
      : text("not found", 404);
  }
  const takedown = await env.APPS.get(`takedown:${channel.current}`);
  if (takedown) return takedownResponse(takedown, env);

  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  // A runtime asked for the app: hand it the fixed content address, never
  // the bytes from here, so what runs is what the address names.
  if (!wantsPage) {
    return new Response(null, {
      status: 302,
      headers: { location: `${base}/a/${channel.current}?dl=1`, "cache-control": "no-store" },
    });
  }
  const meta = await appMeta(env, channel.current);
  return appPage(login, channel, meta, env);
}

/// Every app this developer has published, newest first.
///
/// Channels first, then the apps themselves. A channel is the moving name
/// -- `channel:<login>/<slug>`, rollback, history -- and it is what the
/// page prefers, because its URL survives a new version. But a channel is
/// only written when a LISTED publish happens (IC-389), and it was added
/// long after publishing was, so every app published before it has none.
///
/// Reading channels alone was wrong and shipped: of 20 published apps
/// across four people, exactly ONE had a channel, so three of the four
/// developers' pages were empty -- and once K-856 turned an empty page
/// into a 404, three real developers' names stopped resolving. The app
/// records are the ground truth about who published what; the channel is
/// a nicer name on top of it.
async function developerApps(env, login) {
  const channelRows = await developerChannelApps(env, login);
  const seen = new Set(channelRows.map((row) => row.current));
  const direct = await developerDirectApps(env, login, seen);
  return [...channelRows, ...direct].sort(
    (a, b) => (b.moved_at || 0) - (a.moved_at || 0),
  );
}

/// The apps this developer published that no channel covers.
///
/// Keyed on `author_login`, which the publish door writes from the
/// session -- so it is the publisher's own login, not anything a caller
/// can set. Unlisted apps are left out for the same reason they never
/// move a channel: an unlisted publish is deliberately not the app's
/// public face.
async function developerDirectApps(env, login, seen) {
  const listing = await env.APPS.list({ prefix: "app:", limit: 1000 });
  const rows = await Promise.all(
    listing.keys.map(async (key) => {
      const hash = key.name.slice("app:".length);
      if (seen.has(hash)) return null;
      let meta;
      try {
        meta = JSON.parse((await env.APPS.get(key.name)) || "null");
      } catch (_) {
        return null;
      }
      // <login>.krate.tech shows only what github:<login> owns (K-1085).
      if (!meta || !ownedByGitHubLogin(meta, login)) return null;
      if (meta.unlisted) return null;
      if (await env.APPS.get(`takedown:${hash}`)) return null;
      return {
        // No channel, so the app answers at its content address. The
        // page's own links go through /<slug>, which only a channel can
        // serve, so these carry an absolute URL instead.
        slug: hash,
        href: `${(env.PUBLIC_BASE || "").replace(/\/$/, "")}/a/${hash}`,
        name: meta.name || hash.slice(0, 12),
        current: hash,
        moved_at: (meta.published || 0) * 1000,
        meta,
      };
    }),
  );
  return rows.filter(Boolean);
}

/// Every app this developer has a channel for, newest move first.
async function developerChannelApps(env, login) {
  const listing = await env.APPS.list({ prefix: `channel:${login}/`, limit: 200 });
  const rows = await Promise.all(
    listing.keys.map(async (key) => {
      const raw = await env.APPS.get(key.name);
      if (!raw) return null;
      let channel;
      try {
        channel = JSON.parse(raw);
      } catch (_) {
        return null;
      }
      if (!channel.current) return null;
      if (await env.APPS.get(`takedown:${channel.current}`)) return null;
      const meta = await appMeta(env, channel.current);
      // A channel shows only its own GitHub owner's listing (K-1085).
      if ((meta.owner || meta.author_login) && !ownedByGitHubLogin(meta, login)) return null;
      return {
        slug: channel.slug,
        name: channel.name || channel.slug,
        current: channel.current,
        moved_at: channel.moved_at || 0,
        meta,
      };
    }),
  );
  return rows.filter(Boolean).sort((a, b) => (b.moved_at || 0) - (a.moved_at || 0));
}

/// One app's stored metadata, or an empty object. Never throws: a page
/// that cannot read a size is still a page worth serving.
async function appMeta(env, hash) {
  try {
    const raw = await env.APPS.get(`app:${hash}`);
    return raw ? JSON.parse(raw) : {};
  } catch (_) {
    return {};
  }
}

/// Everything that reaches these pages is publisher-supplied -- an app's
/// name, its description, a login. Escaped on the way out, every time.
function escapeForHtml(value) {
  return String(value == null ? "" : value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/// The shell every page here shares. Deliberately one file with inline
/// styles: a page that depends on a stylesheet on another host is a page
/// that breaks when that host does, and this one has to work when
/// somebody has pasted the URL into a README and a stranger clicked it.
function htmlPage(title, body, status = 200) {
  // A page that says "there is nothing here" must not be indexed as if
  // there were (K-856). krate.tech/robots.txt is `Allow: /` with a
  // sitemap, and the wildcard record means every made-up name reaches
  // this worker, so without this a crawler can mint an unbounded set of
  // Krate pages for people and apps that do not exist.
  //
  // Keyed on the STATUS rather than passed in by each caller: a refusal is
  // exactly the thing not to index, and a caller that forgets the flag is
  // how this comes back. A 200 is untouched, so a real developer's page
  // is never hidden by it.
  const robots =
    status === 200 ? "" : '\n<meta name="robots" content="noindex, follow" />';
  return new Response(
    `<!doctype html><html lang="en"><head><meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />${robots}
<title>${escapeForHtml(title)}</title>
<link rel="icon" href="https://krate.tech/krate-logo.png" />
<style>
:root { color-scheme: dark; --bg:#0b0d12; --text:#fff; --muted:rgba(255,255,255,0.62);
  --faint:rgba(255,255,255,0.42); --line:rgba(255,255,255,0.12); --blue:#6c8cff; }
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--text);
  font:15px/1.6 ui-sans-serif,-apple-system,"Segoe UI",Roboto,sans-serif;
  padding:48px 20px; }
.wrap { max-width:720px; margin:0 auto; }
a { color:var(--blue); text-decoration:none; }
a:hover { text-decoration:underline; }
h1 { font-size:26px; margin:0 0 6px; letter-spacing:-0.01em; }
.sub { color:var(--muted); margin:0 0 30px; }
.app { display:flex; gap:16px; align-items:flex-start; padding:18px 0;
  border-bottom:1px solid var(--line); }
.app:last-of-type { border-bottom:0; }
.shot { width:132px; height:88px; flex:none; border-radius:10px;
  border:1px solid var(--line); object-fit:cover; object-position:top center;
  background:rgba(255,255,255,0.04); }
.app h2 { font-size:16px; margin:0 0 4px; font-weight:560; }
.app p { margin:0 0 6px; color:var(--muted); font-size:13.5px; }
.meta { color:var(--faint); font-size:12px; }
.run { font:12.5px/1.7 ui-monospace,SFMono-Regular,Menlo,monospace;
  background:rgba(255,255,255,0.05); border:1px solid var(--line);
  border-radius:8px; padding:10px 12px; margin:14px 0 0; overflow-x:auto;
  white-space:nowrap; color:var(--muted); }
.btn { display:inline-block; background:#fff; color:#0b0d12; font-weight:560;
  font-size:14px; padding:9px 18px; border-radius:999px; margin-top:14px; }
.btn:hover { text-decoration:none; opacity:0.9; }
.asks { color:var(--faint); font-size:12px; margin-top:10px; }
footer { margin-top:44px; color:var(--faint); font-size:12.5px; }
</style></head><body><div class="wrap">${body}</div></body></html>`,
    { status, headers: { "content-type": "text/html; charset=utf-8", "cache-control": "public, max-age=60" } },
  );
}

/// A developer's own page: who they are, and everything they have made.
/// A name nobody has published under (K-856).
///
/// Worded for the two people who actually land here: somebody who
/// mistyped a developer's name, and somebody who guessed at a name to see
/// what happens. It says plainly that there is nothing under this one,
/// without asserting that the person does or does not exist -- the hub
/// cannot tell, and claiming either would be inventing a fact.
function unknownDeveloperPage(login) {
  // The site's gallery, NOT the hub's /apps -- that one answers JSON to a
  // browser (checked live: content-type application/json on an Accept:
  // text/html request), so linking a person there would hand them raw
  // JSON from a page written to help them.
  // Written out, the way the other pages in this file link to the site
  // (see the takedown and share pages). KRATE_ORIGIN is the BUILDER's
  // variable and does not exist here, so reading it would have silently
  // fallen through to this same default for ever.
  return htmlPage(
    "Nothing published here",
    `<h1>Nothing published here</h1>
     <p class="sub">No apps have been published under
       <b>${escapeForHtml(login)}</b>.</p>
     <p><a href="https://krate.tech/cloud">See what people have made</a>
       &middot; <a href="https://krate.tech">What is Krate?</a></p>`,
    404,
  );
}

/// What a stranger reads about an app, or nothing.
///
/// Studio's publish sheet used to prefill the description with the first
/// thing the person typed, so apps published before 7dfbd7714 carry the
/// request that made them: "make a weather app", "make a calculator app
/// for me for simple calculations". That is an instruction to a builder,
/// not a description of the thing built, and a developer's own page showed
/// it under their name (K-865). A request reads as nothing; no line is
/// better than that line. docs/cloud/app/index.html keeps a copy of this
/// rule, and first-contact-test.mjs runs both on the same sentences.
function plainDescription(text) {
  const s = String(text || "").replace(/\s+/g, " ").trim();
  if (!s) return "";
  if (/^(?:(?:hey|hi|hello)\b[,!]?\s*)?(?:please\s+)?(?:can|could|would|will)\s+you\b/i.test(s)) return "";
  if (/^(?:please\s+)?(?:i\s+(?:want|need|would like)|i'd like|help me|give me|let'?s)\b/i.test(s)) return "";
  if (/^(?:please\s+)?(?:make|build|create|write|generate|code|design|develop)\s+(?:me\s+|us\s+)?(?:a|an|the|my|some|simple|small|basic)\b/i.test(s)) return "";
  return s;
}

function developerPage(login, apps, env) {
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const host = `${login}.${((env.PUBLIC_BASE || "").replace(/^https?:\/\//, "").split(".").slice(-2).join(".")) || "krate.tech"}`;
  const rows = apps.length
    ? apps
        .map((app) => {
          const size = app.meta && app.meta.size ? `${Math.round(app.meta.size / 1024)} KB` : "";
          const desc = escapeForHtml(plainDescription(app.meta && app.meta.description));
          const shot = app.meta && app.meta.shot
            ? `<img class="shot" src="${base}/shot/${escapeForHtml(app.current)}" alt="${escapeForHtml(app.name)}" loading="lazy" />`
            : `<div class="shot"></div>`;
          return `<div class="app">${shot}<div>
            <h2><a href="${escapeForHtml(app.href || `/${app.slug}`)}">${escapeForHtml(app.name)}</a></h2>
            ${desc ? `<p>${desc}</p>` : ""}
            <div class="meta">${size}${size ? " &middot; " : ""}runs on Mac, Windows and Linux</div>
          </div></div>`;
        })
        .join("")
    : `<p class="sub">No apps published yet.</p>`;
  return htmlPage(
    `${login} on Krate`,
    `<h1>${escapeForHtml(login)}</h1>
     <p class="sub">Apps by ${escapeForHtml(login)}, each one file that runs on Mac, Windows and Linux.</p>
     ${rows}
     <footer>Every app here is a single sandboxed file. It reaches only what it
     declares, and you see that before it runs. <a href="https://krate.tech">What is Krate?</a></footer>`,
  );
}

/// One app's page: what it is, what it asks for, and how to run it.
function appPage(login, channel, meta, env) {
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const root = ((env.PUBLIC_BASE || "").replace(/^https?:\/\//, "").split(".").slice(-2).join(".")) || "krate.tech";
  const host = `${login}.${root}`;
  const size = meta && meta.size ? `${Math.round(meta.size / 1024)} KB` : "";
  const asks = Array.isArray(meta && meta.asks) && meta.asks.length
    ? `<p class="asks">Asks for: ${meta.asks.map((a) => escapeForHtml(a)).join(", ")}</p>`
    : "";
  const shot = meta && meta.shot
    ? `<img class="shot" style="width:100%;height:auto;max-height:360px" src="${base}/shot/${escapeForHtml(channel.current)}" alt="${escapeForHtml(channel.name || channel.slug)}" />`
    : "";
  const desc = plainDescription(meta && meta.description);
  return htmlPage(
    `${channel.name || channel.slug} by ${login}`,
    `<h1>${escapeForHtml(channel.name || channel.slug)}</h1>
     <p class="sub">by <a href="/">${escapeForHtml(login)}</a>${size ? ` &middot; ${size}` : ""} &middot; runs on Mac, Windows and Linux</p>
     ${shot}
     ${desc ? `<p>${escapeForHtml(desc)}</p>` : ""}
     ${asks}
     <a class="btn" href="${base}/a/${escapeForHtml(channel.current)}?dl=1">Download the app</a>
     <p class="run">krate run ${escapeForHtml(host)}/${escapeForHtml(channel.slug)}</p>
     <footer>One sandboxed file. It reaches only what it declares, and Krate
     asks you before it does. <a href="https://krate.tech/open/">Get Krate</a></footer>`,
  );
}

/* A sign-in that did not finish goes back to the sign-in page with one
 * line saying why, never to a bare text page with no way out (K-972).
 * Cancelling at GitHub or Google arrives as ?error=access_denied. */
function signInFailed(url, why) {
  const err = url && url.searchParams && url.searchParams.get("error");
  const code = err === "access_denied" ? "cancelled" : why;
  // Tagged, so the sign-in counts still record it as the failure it is
  // (noteStep reads the tag; a plain 302 would count as a success).
  return new Response(null, {
    status: 302,
    headers: { location: `https://krate.tech/login/?error=${encodeURIComponent(code)}`, "x-krate-signin": code },
  });
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
  headers.set("access-control-allow-methods", "GET, POST, PUT, DELETE, OPTIONS");
  headers.set(
    "access-control-allow-headers",
    "authorization, content-type, x-krate-name, x-krate-description, x-krate-category, x-krate-session, x-krate-version, x-krate-os, x-krate-note, x-krate-unlisted, x-krate-encoding, idempotency-key",
  );
  // A page script cannot read a response header it was not shown. The 429
  // from the anonymous wall (K-767) carries `retry-after`, and without this
  // the browser hides it and the page can only guess how long to wait.
  headers.set("access-control-expose-headers", "retry-after");
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

// Personal pricing is frozen (IC-640, IC-402).
//
// The founder's decision is that individual use stays free during adoption,
// and that no price is set before buyer, usage and cost are measured. The
// live surface said otherwise: `/billing/config` returned live:true with
// $12/month, $96/year and $79/year, and a checkout that took money for them.
// A policy that the product contradicts is not a policy.
//
// So this is an explicit switch rather than deleted code. The Stripe
// integration is real work and will be needed when a price is actually
// decided -- CP8 revisits it with measured data. Removing it would mean
// rebuilding it; leaving it live would mean charging people against the
// stated policy. `KRATE_BILLING_UNFROZEN` is the deliberate act that turns
// it back on, and it is not set anywhere.
function billingFrozen(env) {
  return env.KRATE_BILLING_UNFROZEN !== "1";
}

function billingLive(env) {
  if (billingFrozen(env)) return false;
  return Boolean(env.STRIPE_SECRET_KEY && env.STRIPE_PRICE_MONTHLY && env.STRIPE_PRICE_YEARLY);
}

const FOUNDING_SEATS = 200;

async function foundingSold(env) {
  return parseInt((await env.APPS.get("foundingSold")) || "0", 10);
}

async function billingConfig(env) {
  // While frozen, say so plainly and quote no price. A page that renders
  // "$12/month" next to a button that cannot charge is worse than a page
  // with no prices on it.
  if (billingFrozen(env)) {
    return json({ live: false, founding: false, frozen: true, prices: {} });
  }
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
  //
  // The marker is written AFTER the work, not before it (IC-409).
  //
  // It used to be written here, first. If the entitlement write then failed,
  // Stripe's retry found the event already marked processed and returned
  // success -- so Stripe believed delivery had worked while a paying
  // customer never received access, permanently and silently. Reproduced in
  // a controlled test before this change.
  //
  // Marking afterwards means a crash mid-way leaves the event unmarked and
  // Stripe retries it. That can process an event twice, which is the safe
  // direction: every write below is idempotent -- entitlements are derived
  // from the subscription's current state, not incremented -- so a repeat
  // lands on the same answer. Losing a payment is not recoverable; repeating
  // one is.
  if (await env.APPS.get(`evt:${event.id}`)) return json({ ok: true });

  // An organisation's event (IC-948 E): its store decides, failing closed.
  if (await orgBillingEvent(env, event, orgAsk)) {
    await env.APPS.put(`evt:${event.id}`, "1", { expirationTtl: 7 * 24 * 3600 });
    return json({ ok: true });
  }

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

  // Only now: the work above completed without throwing, so this event is
  // genuinely done and Stripe need not send it again.
  await env.APPS.put(`evt:${event.id}`, "1", { expirationTtl: 7 * 24 * 3600 });
  return json({ ok: true });
}


// ================================================================ free tier

// ================================================== the funded-case ledger
//
// One case per app somebody asked for. The old mechanism was a pair of bare
// counters (mkacct:, mkdev:) bumped when a make produced a file -- a number
// with no story, which could not tell a retry from a new app, could not
// refund a failure that was our fault, and lost one of every two
// simultaneous increments to a read-modify-write race.
//
// A case is its own KV key, written under every identity the caller
// presented (account and/or device -- the same two-key rule as the
// counters, so switching accounts does not hand back an allowance):
//
//   case:acct:<userId>:<caseId>
//   case:dev:<deviceHash>:<caseId>
//
// Its lifecycle: opened (funded, consuming nothing yet) -> attempts append
// (made / provider-failed / infra-failed / krate-failed / not-as-asked /
// stopped / revision) -> closed (accepted or abandoned). ONLY a case with a
// "made" attempt counts against the allowance: a failure the person did not
// cause retries inside its case for free, and an abandoned case that never
// produced a file never cost anything. A revision of the same app is an
// attempt on its case; a different app is a new case.
//
// The race two devices used to lose: each case is a distinct key, so
// nothing is overwritten. Two simultaneous opens at 2-of-3 both succeed and
// the ledger briefly holds four cases; the wall refuses the next open. An
// overspend of one in the worst race, never a lost record -- the opposite
// trade from the counters, and the right one, because a lost count was
// silent and this is visible in the person's own ledger.

// The free allowance, per the 2026-09-19 ruling: ONE app, ever.
//
// Counted in cases that PRODUCED A FILE, so somebody whose attempts all
// failed for our reasons has spent nothing and can try again. That is the
// whole reason the ledger counts cases rather than button presses.
const CASE_LIMIT_FREE = 1;

// And one free EDIT of that app, afterwards.
//
// Making an app and then being unable to change it is not a trial of
// anything -- the first version is never the one somebody wants. So a person
// who has used their app gets one more funded case to change it, and that is
// where the free tier ends. Separate from the make allowance rather than
// "two makes", because the two are different promises: the second is only
// for an app that already exists.
const EDIT_LIMIT_FREE = 1;

function caseKeys(user, device) {
  const keys = [];
  if (user) keys.push(`case:acct:${user.id}:`);
  if (/^[0-9a-f]{64}$/.test(device)) keys.push(`case:dev:${device}:`);
  return keys;
}

/// Every case under these identities, one entry per case id.
async function loadCases(env, prefixes) {
  const byId = new Map();
  for (const prefix of prefixes) {
    const listing = await env.APPS.list({ prefix, limit: 1000 });
    for (const key of listing.keys) {
      const id = key.name.slice(prefix.length);
      if (byId.has(id)) continue;
      const record = JSON.parse((await env.APPS.get(key.name)) || "null");
      if (record) byId.set(id, record);
    }
  }
  return byId;
}

/// The number that gates the wall: cases that produced a file.
function madeCount(byId) {
  let n = 0;
  for (const record of byId.values()) if (record.made) n += 1;
  return n;
}

/// The consumed count, read the only way KV answers immediately.
///
/// KV `list` is eventually consistent -- measured against production, a case
/// written now is invisible to a listing for up to ~30 seconds. Deriving the
/// wall's number from `list` therefore let somebody make a fourth free app
/// simply by being quick, which is the one thing the wall exists to stop.
///
/// `get` on a single key IS read-your-writes, which is why the counters this
/// replaced never had the problem. So the counters stay as the authoritative
/// tally -- every consumption writes them -- and the cases carry the history.
/// The higher of the two wins: a listing that has caught up cannot be argued
/// down by a counter, and a counter that is ahead is believed at once.
/// How many funded EDIT cases produced a file.
///
/// Counted off the same ledger as makes, filtered on the flag the case was
/// opened with. A legacy case has no flag and is a make, which is right:
/// every case before this ruling was.
function editCount(byId) {
  // `byId` is a Map, like madeCount's. Object.values on a Map returns an
  // empty array, so the first version of this counted zero edits forever and
  // the allowance never bit -- caught by the test asserting the SECOND edit
  // is refused, which is why that assertion exists rather than only one that
  // the first is allowed.
  let n = 0;
  for (const record of byId.values()) if (record.edit && record.made) n += 1;
  return n;
}

async function consumedCount(env, user, device, byId) {
  const keys = [];
  if (user) keys.push(`mkacct:${user.id}`);
  if (/^[0-9a-f]{64}$/.test(device)) keys.push(`mkdev:${device}`);
  const raw = await Promise.all(keys.map((k) => env.APPS.get(k)));
  const counter = raw.reduce((most, v) => Math.max(most, parseInt(v || "0", 10)), 0);
  return Math.max(counter, madeCount(byId));
}

/// How much of the count belongs to the MACHINE rather than to the person
/// (K-807).
///
/// A fresh account on a second-hand or shared Mac starts with its free app
/// already spent, because the device hash is the second key against a
/// new-email reset. The mechanism is right and stays; what was missing is
/// that nothing ever said so, so the person met a wall with no explanation.
///
/// This answers the only question that makes the wall explicable: would the
/// count be lower if this were a different machine? It reads the device's
/// own counter and its own cases, and says nothing about the account.
async function deviceOnlyCount(env, device) {
  if (!/^[0-9a-f]{64}$/.test(device)) return 0;
  // The counter first, because `get` is read-your-writes and `list` is not.
  // Same reasoning as consumedCount: a listing that has not caught up must
  // never argue the number down.
  const counter = parseInt((env.APPS.get ? await env.APPS.get(`mkdev:${device}`) : null) || "0", 10) || 0;
  // A case record carries no device field -- the identity is the KV PREFIX
  // it is stored under. Filtering on `record.device` matches nothing and
  // silently reports the counter alone, which is why this loads by prefix.
  const owned = await loadCases(env, [`case:dev:${device}:`]);
  return Math.max(counter, madeCount(owned));
}

async function writeCase(env, prefixes, record) {
  await Promise.all(
    prefixes.map((prefix) => env.APPS.put(`${prefix}${record.id}`, JSON.stringify(record))),
  );
}

/// The old counters become legacy cases, once, lazily.
///
/// Anyone who made apps before the ledger existed has a number in mkacct:/
/// mkdev: and no cases. If the counter says more than the ledger does, the
/// difference is minted as closed, accepted, made cases marked "migrated" --
/// so the allowance they spent stays spent, and it is now READABLE: every
/// consumed slot is a record, not a bare integer.
async function migrateCounters(env, user, device, prefixes, byId, { write = true } = {}) {
  const counterKeys = [];
  if (user) counterKeys.push(`mkacct:${user.id}`);
  if (/^[0-9a-f]{64}$/.test(device)) counterKeys.push(`mkdev:${device}`);
  if (!counterKeys.length) return byId;

  const counts = await Promise.all(counterKeys.map((k) => env.APPS.get(k)));
  const counter = counts.reduce((most, raw) => Math.max(most, parseInt(raw || "0", 10)), 0);
  const short = counter - madeCount(byId);

  // A READ must not write. `/plan/get` is called on every Studio session,
  // and minting here put KV puts on a read path -- on the namespace that
  // publishes and sign-ins share. Worse, a dropped put (quota exhausted)
  // leaves the cases unstored, so the next read computes the same shortfall
  // and mints again: unbounded writes exactly when the budget is gone. That
  // is how counting took the product down on 2026-08-10, and the note above
  // planCount says never to do it again.
  //
  // So a read reports the counter's number without materialising it. The
  // wall is still correct -- the count is the same -- and the records get
  // written by the next call that was going to write anyway.
  if (!write) {
    for (let i = 0; i < short; i++) {
      byId.set(`unmigrated-${i}`, { id: `unmigrated-${i}`, made: true, phantom: true });
    }
    return byId;
  }

  for (let i = 0; i < short; i++) {
    const record = {
      id: `legacy-${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`,
      opened: new Date().toISOString(),
      state: "accepted",
      made: true,
      request: "",
      attempts: [
        { at: new Date().toISOString(), outcome: "made", note: "migrated from the counter" },
      ],
    };
    await writeCase(env, prefixes, record);
    byId.set(record.id, record);
  }
  return byId;
}

async function caseIdentity(request, env, body) {
  const device = String(body.device || "");
  const user = await authedUser(request, env);
  const prefixes = caseKeys(user, device);
  return { user, device, prefixes };
}

async function caseOpen(request, env) {
  const body = await request.json().catch(() => ({}));
  const { user, device, prefixes } = await caseIdentity(request, env, body);
  if (!prefixes.length) return text("Sign in first, or send a device id.", 400);

  const byId = await migrateCounters(env, user, device, prefixes, await loadCases(env, prefixes));

  // Is this a change to an app they already have, or a new one?
  //
  // The two have separate allowances because they are separate promises:
  // one app free, and one change to it. A person who could make an app and
  // never alter it has not really tried the product -- the first version is
  // never the one they want.
  const isEdit = Boolean(body.edit);

  // The wall. A paid plan lifts it; the free allowance is counted in cases
  // that produced a file, so a person whose attempts all failed for our
  // reasons has spent nothing.
  const made = await consumedCount(env, user, device, byId);
  const edits = editCount(byId);
  const spent = isEdit ? edits : made;
  const limit = isEdit ? EDIT_LIMIT_FREE : CASE_LIMIT_FREE;
  if (spent >= limit) {
    const ent = user ? JSON.parse((await env.APPS.get(`ent:${user.id}`)) || "null") : null;
    if (!entitlementActive(ent)) {
      // No price: Krate quotes none anywhere (the pricing decision), and
      // this said "Studio is $12 a month" and "Studio is free" in one
      // sentence (K-907). The two ways on are both free.
      const message = isEdit
        ? "The free change is used. Krate Studio on your own machine is free and unlimited with your own AI, and this session is waiting there -- or add your own API key in Settings and keep going here."
        : "The free app is used. Krate Studio on your own machine is free and unlimited with your own AI, and this session is waiting there -- or add your own API key in Settings and keep going here.";
      return json({ wall: true, n: spent, limit, edit: isEdit, message }, 402);
    }
  }

  // What a free person's tries may cost us, whatever they produced
  // (2026-10-06). The allowance counts only "made", so failed tries were
  // unlimited, and one person's builds took about $5 of the Krate key in
  // half an hour without an app. Two limits, both lifted by a paid plan and
  // neither touching a build on the person's own key (those open no case):
  //   - FREE_TRIES_PER_DAY cases opened in 24 hours that made nothing
  //     (5: four failures on a bad provider afternoon must still leave room
  //     to make the app, which failure-keeps-the-free-app.test.mjs holds);
  //   - FREE_SPEND_CEILING_USD of Krate-paid spend on the account in all,
  //     enough for the free app and its free change at a normal cost.
  // Both are settings, because they are the founder's money to decide.
  {
    const ent = user ? JSON.parse((await env.APPS.get(`ent:${user.id}`)) || "null") : null;
    if (!entitlementActive(ent)) {
      const dayAgo = Date.now() - 24 * 60 * 60 * 1000;
      const unmade = [...byId.values()].filter((c) => c && !c.made && Date.parse(c.opened || "") > dayAgo).length;
      const triesPerDay = Number(env.FREE_TRIES_PER_DAY || 5);
      if (unmade >= triesPerDay) {
        return json({
          wall: true, tries: true, n: unmade, limit: triesPerDay, edit: isEdit,
          message: "Today's free tries are used. Try again tomorrow, or keep going now: add your own API key in Settings, or use Krate Studio on your computer, free with your own AI.",
        }, 429);
      }
      if (user) {
        const log = JSON.parse((await env.APPS.get(`spend:${user.id}`)) || "[]");
        const ours = log.filter((r) => r.paid_by !== "own").reduce((t, r) => t + (r.usd || 0), 0);
        const ceiling = Number(env.FREE_SPEND_CEILING_USD || 6);
        if (ours >= ceiling) {
          return json({
            wall: true, spend: true, edit: isEdit,
            message: "The free building on this account is used up. Add your own API key in Settings to keep going here, or use Krate Studio on your computer, free with your own AI.",
          }, 402);
        }
      }
    }
  }

  // An EDIT with no app to edit is a make, whatever the caller said. A
  // client that sent `edit: true` first would otherwise get a free app out
  // of the edit allowance and keep its make allowance untouched.
  if (isEdit && made < 1) {
    return json(
      { wall: true, n: 0, limit: EDIT_LIMIT_FREE, edit: true, message: "There is no app to change yet. Make one first." },
      409,
    );
  }

  const record = {
    id: crypto.randomUUID().replaceAll("-", "").slice(0, 16),
    opened: new Date().toISOString(),
    state: "open",
    made: false,
    edit: isEdit,
    request: String(body.request || "").slice(0, 200),
    attempts: [],
  };
  await writeCase(env, prefixes, record);
  return json({ id: record.id, n: made });
}

const CASE_OUTCOMES = new Set([
  "made", // a file was produced -- this is the one that consumes the case's funding
  "provider-failed", // the AI vendor failed; not the person's fault
  "infra-failed", // our machine failed; not the person's fault
  "krate-failed", // the engine failed; not the person's fault
  "not-as-asked", // built, but the wrong thing -- exit 6 in the CLI's vocabulary
  "stopped", // the person stopped it
  "revision", // more work on the same app, inside the same case
]);

async function loadOwnCase(env, prefixes, id) {
  if (!/^[a-z0-9-]{1,40}$/.test(id)) return null;
  for (const prefix of prefixes) {
    const record = JSON.parse((await env.APPS.get(`${prefix}${id}`)) || "null");
    if (record) return record;
  }
  return null;
}

async function caseAttempt(request, env) {
  const body = await request.json().catch(() => ({}));
  const { prefixes } = await caseIdentity(request, env, body);
  if (!prefixes.length) return text("Sign in first, or send a device id.", 400);

  const outcome = String(body.outcome || "");
  if (!CASE_OUTCOMES.has(outcome)) {
    return text(`say what happened: one of ${[...CASE_OUTCOMES].join(", ")}`, 400);
  }
  // Only a case you can name under your own identity: the id space is per
  // account/device, so someone else's case id simply is not found.
  const record = await loadOwnCase(env, prefixes, String(body.id || ""));
  if (!record) return text("no such case", 404);
  if (record.state !== "open") {
    return text(`that case is ${record.state}; new work is a new case`, 409);
  }

  record.attempts.push({
    at: new Date().toISOString(),
    outcome,
    note: String(body.note || "").slice(0, 200) || undefined,
  });
  if (outcome === "made") record.made = true;
  await writeCase(env, prefixes, record);

  // The legacy counters mirror every consumption, whichever route consumed
  // it. planCount mirrors its own writes; without this, a made-attempt
  // through the case route left the counters behind, and a rolled-back
  // worker -- which reads only counters -- would hand the allowance out
  // again.
  if (outcome === "made") {
    const { user, device } = await caseIdentity(request, env, body);
    const counterKeys = [];
    if (user) counterKeys.push(`mkacct:${user.id}`);
    if (/^[0-9a-f]{64}$/.test(device)) counterKeys.push(`mkdev:${device}`);
    if (counterKeys.length) {
      // The counter is INCREMENTED from its own strongly-consistent value,
      // never recomputed from a listing. Recomputing wrote the stale number
      // straight back, so the counter never moved and the wall never saw
      // the consumption -- measured against production: n stayed 0 for
      // ~30 seconds after a made attempt, which is long enough to take a
      // fourth free app.
      const raw = await Promise.all(counterKeys.map((k) => env.APPS.get(k)));
      const current = raw.reduce((most, v) => Math.max(most, parseInt(v || "0", 10)), 0);
      const n = current + 1;
      await Promise.all(counterKeys.map((k) => env.APPS.put(k, String(n))));
    }
  }
  return json({ id: record.id, state: record.state, made: record.made });
}

async function caseClose(request, env) {
  const body = await request.json().catch(() => ({}));
  const { prefixes } = await caseIdentity(request, env, body);
  if (!prefixes.length) return text("Sign in first, or send a device id.", 400);

  const verdict = String(body.verdict || "");
  if (verdict !== "accepted" && verdict !== "abandoned") {
    return text("verdict is accepted or abandoned", 400);
  }
  const record = await loadOwnCase(env, prefixes, String(body.id || ""));
  if (!record) return text("no such case", 404);

  // Closing twice with the same verdict is idempotent; changing a verdict
  // is not a thing -- the record is the history.
  if (record.state === "open") {
    record.state = verdict;
    record.closed = new Date().toISOString();
    await writeCase(env, prefixes, record);
  }
  // An abandoned case that never made a file was never counted; nothing to
  // refund because nothing was taken. One that made a file stays counted:
  // the file exists.
  return json({ id: record.id, state: record.state, made: record.made });
}

async function caseList(request, env) {
  const body = await request.json().catch(() => ({}));
  const { user, device, prefixes } = await caseIdentity(request, env, body);
  if (!prefixes.length) return text("Sign in first, or send a device id.", 400);
  // A listing is a read: it reports the counter's spend without spending
  // puts to materialise it. The phantom rows carry no history, which is
  // honest -- the counter never had any.
  const byId = await migrateCounters(
    env, user, device, prefixes, await loadCases(env, prefixes), { write: false },
  );
  const cases = [...byId.values()]
    .filter((c) => !c.phantom)
    .sort((a, b) => (a.opened < b.opened ? -1 : 1));
  // The counter, not the listing: a case made seconds ago is not in the
  // listing yet, and reporting a number the wall disagrees with would read
  // as the allowance resetting.
  const n = await consumedCount(env, user, device, byId);
  return json({ n, limit: CASE_LIMIT_FREE, cases });
}

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

  // The wire contract is unchanged -- {n, keys} in, {device, n} out -- but
  // the number now comes from the funded-case ledger, with the old counters
  // as a migration floor. No month anywhere. ONE app EVER, plus one change
  // to it, per the 2026-09-19 ruling -- see CASE_LIMIT_FREE and
  // EDIT_LIMIT_FREE above, which are what actually enforce it. This comment
  // said "three ever" after that ruling landed, which is the third time the
  // number has drifted between a comment, the UI and the wall. The wall is
  // the policy; the words must not promise more than it honours.
  const prefixes = caseKeys(user, device);
  // Materialise the old counters only when this call was going to write
  // anyway. A plain read reports the same number without spending puts.
  const byId = await migrateCounters(
    env, user, device, prefixes, await loadCases(env, prefixes),
    { write: Boolean(increment) },
  );

  // A device that made apps offline reports the higher local number; the
  // mirror never goes backward, so an offline make is still counted --
  // minted as cases, so even those slots have a readable record.
  const local = Number.isFinite(body.n) ? Math.max(0, Math.floor(body.n)) : 0;
  let short = local - madeCount(byId);
  // Bounded, and only where a write was already happening. Studio sends its
  // local count on EVERY session sync, so minting here on a read turned one
  // status check into up to three KV puts, forever, on the namespace
  // publishes share. The cap is the free allowance: a client claiming a
  // thousand offline makes cannot make us write a thousand records.
  if (!increment) short = 0;
  short = Math.min(short, CASE_LIMIT_FREE);
  while (short > 0) {
    const record = {
      id: `offline-${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`,
      opened: new Date().toISOString(),
      state: "accepted",
      made: true,
      request: "",
      attempts: [
        { at: new Date().toISOString(), outcome: "made", note: "made offline, mirrored later" },
      ],
    };
    await writeCase(env, prefixes, record);
    byId.set(record.id, record);
    short -= 1;
  }

  if (increment) {
    // The legacy increment is one whole case: opened, made, accepted, in a
    // single call. Older clients (Studio's plan_count_make, the builder's
    // countTheMake) speak this route and get exact ledger semantics without
    // knowing the ledger exists.
    const record = {
      id: crypto.randomUUID().replaceAll("-", "").slice(0, 16),
      opened: new Date().toISOString(),
      state: "accepted",
      made: true,
      request: String(body.request || "").slice(0, 200),
      attempts: [{ at: new Date().toISOString(), outcome: "made" }],
    };
    await writeCase(env, prefixes, record);
    byId.set(record.id, record);
  }

  // The answer never goes backward. On a read the offline makes were not
  // materialised, so the client's own number still counts toward what is
  // reported -- the wall must not under-count just because this call
  // declined to spend writes.
  const n = Math.max(await consumedCount(env, user, device, byId), local);
  // The counters stay mirrored, but only on a call that was already
  // writing. Nothing reads them for truth any more; they exist so a
  // rolled-back worker cannot hand the allowance out again, and a rollback
  // that gives away free apps is the expensive kind of surprise.
  if (increment) {
    await Promise.all(keys.map((k) => env.APPS.put(k, String(n))));
  }
  // Say where the count came from, so a wall can be explained (K-807).
  //
  // `machine` is how much of `n` this DEVICE already carried. When a person
  // signs in on a second-hand or shared Mac their free app can be spent
  // before they have made anything, and the number alone cannot tell them
  // why. Reported, not enforced: the wall is still `n`, and a client that
  // ignores this field behaves exactly as before.
  const machine = await deviceOnlyCount(env, device);
  return json({ n, keys: keys.length, machine });
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

/* ---- a person's own API key -------------------------------------------
 *
 * Krate pays for the first app in a browser and no more than that: after
 * it, somebody who wants to keep building in a tab brings their own key.
 * The desktop already works this way (`krate api-key set`), storing the key
 * in the OS keychain -- a browser has no keychain, so the hub holds it.
 *
 * Held ENCRYPTED, not merely private. KV is not a secret store: anything
 * with read access to the namespace would otherwise see every customer's
 * API key in plain text. AES-GCM under a server-held secret means a KV dump
 * is useless on its own, and the key is decrypted only in the moment a
 * build needs it.
 *
 * It never travels back to the browser. `GET /keys` answers whether one is
 * set and the last four characters, which is enough for a person to
 * recognise which key they pasted and useless to anybody else.
 */
const KEY_VENDORS = { anthropic: "Anthropic (Claude)", openai: "OpenAI" };

/// The wrapping key, derived from a server secret. Absent in a dev worker
/// with no secret set, and every key route then refuses rather than storing
/// something it cannot protect.
async function wrappingKey(env) {
  const secret = env.KEY_WRAP_SECRET || "";
  if (!secret) return null;
  const material = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(secret));
  return crypto.subtle.importKey("raw", material, { name: "AES-GCM" }, false, [
    "encrypt",
    "decrypt",
  ]);
}

async function sealKey(env, plaintext) {
  const wrap = await wrappingKey(env);
  if (!wrap) return null;
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const sealed = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv },
    wrap,
    new TextEncoder().encode(plaintext),
  );
  const b64 = (bytes) => btoa(String.fromCharCode(...new Uint8Array(bytes)));
  return { iv: b64(iv), body: b64(sealed) };
}

/// Decrypt one stored key. Used by the build service through /keys/use, and
/// nowhere that answers a browser.
async function openKey(env, record) {
  const wrap = await wrappingKey(env);
  if (!wrap || !record || !record.body) return null;
  const bytes = (b64) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  try {
    const out = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: bytes(record.iv) },
      wrap,
      bytes(record.body),
    );
    return new TextDecoder().decode(out);
  } catch (e) {
    return null;
  }
}

async function listKeys(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const held = JSON.parse((await env.APPS.get(`keys:${user.id}`)) || "{}");
  return json({
    keys: Object.entries(KEY_VENDORS).map(([vendor, label]) => {
      const record = held[vendor];
      return {
        vendor,
        label,
        set: Boolean(record),
        // Enough to recognise the key, useless to anyone else.
        tail: record ? record.tail || "" : "",
        added: record ? record.added || 0 : 0,
      };
    }),
  });
}

async function putKey(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  if (!(await wrappingKey(env))) {
    return text("This Krate cannot store keys safely yet; ask us to finish setting it up.", 503);
  }
  let body;
  try {
    body = await request.json();
  } catch (e) {
    return text("expected JSON", 400);
  }
  const vendor = String(body.vendor || "");
  const key = String(body.key || "").trim();
  if (!KEY_VENDORS[vendor]) return text("unknown vendor", 400);
  // Shape only -- whether it WORKS is the build's business, and a key that
  // looks wrong is worth refusing before it is stored and puzzled over.
  if (key.length < 20 || key.length > 400 || /\s/.test(key)) {
    return text("that does not look like an API key", 422);
  }
  if (vendor === "anthropic" && !key.startsWith("sk-ant-")) {
    return text("an Anthropic key starts with sk-ant-", 422);
  }
  if (vendor === "openai" && !key.startsWith("sk-")) {
    return text("an OpenAI key starts with sk-", 422);
  }
  const sealed = await sealKey(env, key);
  if (!sealed) return text("could not store that key", 500);
  const held = JSON.parse((await env.APPS.get(`keys:${user.id}`)) || "{}");
  held[vendor] = { ...sealed, tail: key.slice(-4), added: Math.floor(Date.now() / 1000) };
  await env.APPS.put(`keys:${user.id}`, JSON.stringify(held));
  return json({ ok: true, vendor, tail: key.slice(-4) });
}

async function forgetKey(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  let body;
  try {
    body = await request.json();
  } catch (e) {
    return text("expected JSON", 400);
  }
  const vendor = String(body.vendor || "");
  if (!KEY_VENDORS[vendor]) return text("unknown vendor", 400);
  const held = JSON.parse((await env.APPS.get(`keys:${user.id}`)) || "{}");
  delete held[vendor];
  await env.APPS.put(`keys:${user.id}`, JSON.stringify(held));
  return json({ ok: true });
}

/// Is this request from the build service? It carries a secret only the
/// builder and this worker hold (`KRATE_BUILDER_SECRET`, set on both).
/// Compared by digest, so the time taken says nothing about how much of a
/// guess was right. No secret configured means nobody qualifies.
async function fromBuilder(request, env) {
  const expected = env.KRATE_BUILDER_SECRET || "";
  const offered = request.headers.get("x-krate-builder") || "";
  if (!expected || !offered) return false;
  const digest = async (s) =>
    new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
  const [a, b] = await Promise.all([digest(expected), digest(offered)]);
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

/// Hand one decrypted key to the build service, for a build this very
/// person started.
///
/// TWO credentials, because either alone is not enough. The person's
/// session says WHOSE key: the builder can only ever read the key of the
/// account whose build it is running. The builder's secret says WHO is
/// asking: this used to take the session alone, on a CORS-enabled route,
/// so anything holding a person's session -- a script injected into a
/// krate.tech page, say -- could read their API key in plain text, while
/// the comment above promised it never travels back to a browser (K-861).
/// Answers 404 when they have not set one, so the caller falls back to
/// whatever it has.
async function useKey(request, env) {
  if (!(await fromBuilder(request, env))) {
    return text("Only the build service may ask for this.", 403);
  }
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  let body;
  try {
    body = await request.json();
  } catch (e) {
    return text("expected JSON", 400);
  }
  const vendor = String(body.vendor || "");
  if (!KEY_VENDORS[vendor]) return text("unknown vendor", 400);
  const held = JSON.parse((await env.APPS.get(`keys:${user.id}`)) || "{}");
  if (!held[vendor]) return text("no key set", 404);
  const key = await openKey(env, held[vendor]);
  if (!key) return text("that key could not be read", 500);
  return json({ vendor, key });
}

/* ---- what the builds cost ----------------------------------------------
 *
 * Anybody spending their own money on inference is owed the number, per
 * build, not a monthly total they cannot check. The engine already prices
 * each run from the API's own token counts; the build service forwards it
 * here and this keeps the ledger.
 *
 * Two buckets, because they are different money: `krate` is the funded
 * first app, which we paid for, and `own` is spend on the person's own key.
 */
async function recordSpend(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  let body;
  try {
    body = await request.json();
  } catch (e) {
    return text("expected JSON", 400);
  }
  const usd = Number(body.usd);
  if (!Number.isFinite(usd) || usd < 0 || usd > 1000) return text("bad amount", 400);
  const entry = {
    at: Math.floor(Date.now() / 1000),
    usd: Math.round(usd * 10000) / 10000,
    model: String(body.model || "").slice(0, 60),
    rounds: Number(body.rounds) || 0,
    app: String(body.app || "").slice(0, 80),
    paid_by: body.paid_by === "own" ? "own" : "krate",
  };
  const log = JSON.parse((await env.APPS.get(`spend:${user.id}`)) || "[]");
  log.unshift(entry);
  // A person's own history, not an audit trail: the last 200 builds is more
  // than anyone scrolls, and an unbounded list in one KV value eventually
  // stops being writable at all.
  await env.APPS.put(`spend:${user.id}`, JSON.stringify(log.slice(0, 200)));
  return json({ ok: true });
}

async function spendReport(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const log = JSON.parse((await env.APPS.get(`spend:${user.id}`)) || "[]");
  const now = Math.floor(Date.now() / 1000);
  const since = (days) => now - days * 86400;
  const sum = (rows) => Math.round(rows.reduce((t, r) => t + (r.usd || 0), 0) * 10000) / 10000;
  const own = log.filter((r) => r.paid_by === "own");
  const krate = log.filter((r) => r.paid_by !== "own");
  return json({
    builds: log.length,
    total: { own: sum(own), krate: sum(krate) },
    month: { own: sum(own.filter((r) => r.at >= since(30))), krate: sum(krate.filter((r) => r.at >= since(30))) },
    week: { own: sum(own.filter((r) => r.at >= since(7))), krate: sum(krate.filter((r) => r.at >= since(7))) },
    recent: log.slice(0, 50),
  });
}

/// The welcome is shown once per person, not once per browser: a new
/// browser, a private window or cleared site data showed it again. One write,
/// the first time only (KV writes are a daily budget, K-952).
async function markAccountOnboarded(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  if (!user.onboarded) {
    user.onboarded = true;
    await env.APPS.put(`user:${user.id}`, JSON.stringify(user));
  }
  return json({ ok: true });
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
      // Studio's welcome has been seen on this account, on any device.
      onboarded: Boolean(user.onboarded),
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

/* ---- build sessions, per account ---------------------------------------
 *
 * A session is the conversation that made an app: what was asked for, what
 * the agent said back, what came out. Studio already keeps them -- as files
 * on the desktop, in localStorage in the browser -- and neither was tied to
 * an account, so a person who started in a tab and then downloaded Studio
 * arrived to an empty room.
 *
 * The record is stored opaquely. Studio's own Session struct carries a
 * `#[serde(flatten)]` catch-all precisely because whitelisting its fields
 * silently dropped everything else (K-203), and a hub that re-shaped the
 * JSON here would reintroduce that bug from the other side. So: validate the
 * envelope, cap the size, store the body untouched.
 */

/// Keys are `sess:<user>:<id>`. One KV entry per session rather than one
/// bucket per account: a bucket means every save rewrites every session, and
/// two machines saving at once lose one of them.
const SESSION_MAX_BYTES = 256 * 1024;
const SESSION_MAX_PER_USER = 100;

function sessionOwner(user) {
  // The account id first, and only then the login.
  //
  // Not login-first: a Krate account with no GitHub gets its login derived
  // from the local part of an email, so two people at different domains --
  // sam@a.com and sam@b.io -- would land on the same prefix and read each
  // other's sessions. The id is minted per account and cannot collide.
  //
  // Never the email itself: that would put an address into a key name, which
  // shows up in listings and logs.
  const key = String(user.id || user.login || "");
  return /^[A-Za-z0-9_.-]{1,64}$/.test(key) ? key : "";
}

async function sessionsList(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const owner = sessionOwner(user);
  if (!owner) return json({ sessions: [] });

  const listing = await env.APPS.list({ prefix: `sess:${owner}:`, limit: 200 });
  const sessions = [];
  for (const key of listing.keys) {
    const raw = await env.APPS.get(key.name);
    if (!raw) continue;
    try {
      sessions.push(JSON.parse(raw));
    } catch (_) {
      // A single unreadable record must not take the whole list down: the
      // person's other sessions are still theirs.
    }
  }
  // Newest first, the order Studio's own list expects.
  sessions.sort((a, b) => (b.updated || 0) - (a.updated || 0));
  return json({ sessions });
}

async function sessionsPut(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const owner = sessionOwner(user);
  if (!owner) return text("no account", 401);

  let session;
  try {
    session = await request.json();
  } catch (_) {
    return text("bad request", 400);
  }
  const id = String(session && session.id ? session.id : "");
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(id)) {
    return text("a session needs an id", 400);
  }
  const body = JSON.stringify(session);
  if (body.length > SESSION_MAX_BYTES) {
    // A refusal that names the number, because the caller can act on it:
    // the transcript is what grows, and Studio can trim it.
    return text(`session too large (${SESSION_MAX_BYTES} bytes max)`, 413);
  }

  // Last write wins, but only if it is actually later. Two machines editing
  // one session should not let a stale save from the slower one overwrite
  // the newer work -- the same rule store.shared uses for its keys.
  const existing = await env.APPS.get(`sess:${owner}:${id}`);
  if (existing) {
    try {
      const prev = JSON.parse(existing);
      if ((prev.updated || 0) > (session.updated || 0)) {
        return json({ ok: true, kept: "newer", session: prev });
      }
    } catch (_) {
      // Unreadable previous value: the new one is strictly better.
    }
  } else {
    // Only count on create, and only enough to stop a runaway client from
    // filling the namespace publishes and sign-ins share.
    const listing = await env.APPS.list({ prefix: `sess:${owner}:`, limit: SESSION_MAX_PER_USER + 1 });
    if (listing.keys.length >= SESSION_MAX_PER_USER) {
      return text(`too many sessions (${SESSION_MAX_PER_USER} max)`, 409);
    }
  }

  await env.APPS.put(`sess:${owner}:${id}`, body);
  return json({ ok: true });
}

async function sessionsDelete(request, id, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  const owner = sessionOwner(user);
  if (!owner || !/^[A-Za-z0-9_-]{1,64}$/.test(id)) return text("not found", 404);
  await env.APPS.delete(`sess:${owner}:${id}`);
  return json({ ok: true });
}

async function myApps(request, env) {
  const user = await authedUser(request, env);
  if (!user) return text("Sign in first.", 401);
  // What this person owns (K-1085): the account's own publishes, and, with
  // a GitHub sign-in, what that GitHub login owns. Never by login text.
  const mine = new Set([`account:${user.id}`, ownerOf(await verifyGitHub(request, env))].filter(Boolean));
  const listing = await env.APPS.list({ prefix: "app:", limit: 1000 });
  const out = [];
  for (const k of listing.keys) {
    const a = JSON.parse((await env.APPS.get(k.name)) || "null");
    if (a && mine.has(ownerOfRecord(a))) {
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
  const typed = String(body.email || "").trim().toLowerCase();
  // A signed-in person needs no address: the reply lands in their thread,
  // and the account already knows how to reach them. An address they
  // typed anyway is kept, because a GitHub account can carry no email at
  // all. Only a signed-out sender must give one (K-785).
  const email = user ? (user.email || typed) : typed;
  const subject = String(body.subject || "").trim().slice(0, 140);
  const first = String(body.text || "").trim().slice(0, 4000);
  if (!subject || !first) return text("Say what it is about, and what happened.", 400);
  // The Studio session the ticket is about, when it came from inside one:
  // the desk opens that conversation beside the ticket.
  const session = /^[A-Za-z0-9_-]{1,64}$/.test(String(body.session || "")) ? String(body.session) : "";
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
    session,
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
    const reps = await env.APPS.list({ prefix: "report:", limit: 1000 });
    return json({
      users: users.keys.length,
      tickets: ticks.keys.length,
      reports: reps.keys.length,
      payments: pays.keys.length,
      billing_live: billingLive(env),
    });
  }
  if (route === "welcomes") {
    // Newest first, each with whether the founder has welcomed them and the
    // one-click compose link.
    const listing = await env.APPS.list({ prefix: "user:", limit: 1000 });
    const out = [];
    for (const k of listing.keys) {
      const u = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (!u) continue;
      out.push({
        id: u.id,
        name: u.name || "",
        login: u.login || "",
        email: u.email || "",
        providers: u.providers || [],
        created: u.created || 0,
        welcomed: u.welcomed || 0,
        compose: u.email ? welcomeComposeUrl(u) : null,
      });
    }
    out.sort((a, b) => b.created - a.created);
    return json({ people: out, waiting: out.filter((p) => !p.welcomed && p.email).length });
  }
  if (route === "welcomed" && request.method === "POST") {
    const u = JSON.parse((await env.APPS.get(`user:${body.id}`)) || "null");
    if (!u) return text("no such person", 404);
    if (body.welcomed) u.welcomed = Date.now();
    else delete u.welcomed;
    await env.APPS.put(`user:${body.id}`, JSON.stringify(u));
    return json({ ok: true, welcomed: u.welcomed || 0 });
  }
  if (route === "people") {
    const days = Math.min(90, Math.max(1, parseInt(new URL(request.url).searchParams.get("days") || "30", 10) || 30));
    return json(await people(env, days));
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
    }
    // Newest first. KV lists keys in id order -- random UUIDs -- so the
    // first 100 were an arbitrary 100 and a new sign-up could sit anywhere.
    out.sort((a, b) => (b.created || 0) - (a.created || 0));
    return json({ users: out.slice(0, 200) });
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
    // Krate sessions, and GitHub sign-ins the hub indexed (account-sessions.js).
    return json({ revoked: await endEverySession(env, String(body.id || ""), GITHUB_CLIENT_ID) });
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
  // Studio's "send a report": a zip of evidence plus who, which session,
  // which Krate and which OS. Stored since the hub began taking reports,
  // but the desk had no place that listed them, so a report someone sent
  // was invisible unless you already knew its id.
  if (route === "reports") {
    const listing = await env.APPS.list({ prefix: "report:", limit: 300 });
    const reports = [];
    for (const k of listing.keys) {
      const r = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (r) reports.push(r);
    }
    reports.sort((a, b) => (b.received || 0) - (a.received || 0));
    return json({ reports });
  }
  if (route === "report/state" && request.method === "POST") {
    const id = String(body.id || "");
    if (!/^[0-9a-f]{16}$/.test(id)) return text("no such report", 404);
    const r = JSON.parse((await env.APPS.get(`report:${id}`)) || "null");
    if (!r) return text("no such report", 404);
    if (!["new", "seen", "done"].includes(body.state)) return text("state is new, seen or done", 400);
    r.state = body.state;
    await env.APPS.put(`report:${id}`, JSON.stringify(r));
    return json({ ok: true });
  }
  // A person's Studio sessions, so a ticket or a report can be read beside
  // the conversation it is about. By account id (tickets carry it) or by
  // login (reports carry that), resolved to the same key Studio saves under.
  if (route === "person/sessions") {
    const q = new URL(request.url).searchParams;
    const owner = await adminSessionOwner(env, q.get("id") || "", q.get("login") || "");
    if (!owner) return json({ sessions: [], note: "no account found for that sender" });
    const listing = await env.APPS.list({ prefix: `sess:${owner}:`, limit: 200 });
    const sessions = [];
    for (const k of listing.keys) {
      const sess = JSON.parse((await env.APPS.get(k.name)) || "null");
      if (sess) sessions.push(sess);
    }
    sessions.sort((a, b) => (b.updated || 0) - (a.updated || 0));
    return json({ sessions });
  }
  return text("not found", 404);
}

// The key a person's sessions are saved under, from what the desk has:
// an account id from a ticket, or a GitHub login from a report.
async function adminSessionOwner(env, id, login) {
  if (id) {
    const u = JSON.parse((await env.APPS.get(`user:${id}`)) || "null");
    return sessionOwner(u || { id });
  }
  if (!login) return "";
  const want = login.toLowerCase();
  const listing = await env.APPS.list({ prefix: "user:", limit: 1000 });
  for (const k of listing.keys) {
    const u = JSON.parse((await env.APPS.get(k.name)) || "null");
    if (u && String(u.login || "").toLowerCase() === want) return sessionOwner(u);
  }
  return sessionOwner({ login });
}

/// The admin desk page. One file, no build step; signs in through the same
/// krate.tech login the product uses and talks to /admin/api/* with the
/// token. Renders nothing for non-admins because the APIs 404.
/// Rows from Analytics Engine, or null when the read token is missing or
/// the query fails -- the desk must say "unknown", never show a zero.
async function aeRows(env, sql) {
  if (!env.CF_ANALYTICS_TOKEN) return null;
  try {
    const r = await fetch(
      `https://api.cloudflare.com/client/v4/accounts/${env.CF_ACCOUNT_ID}/analytics_engine/sql`,
      { method: "POST", headers: { Authorization: `Bearer ${env.CF_ANALYTICS_TOKEN}` }, body: sql },
    );
    if (!r.ok) return null;
    return (await r.json()).data || [];
  } catch {
    return null;
  }
}

/// The founder's question in one answer: did real people come to krate.tech,
/// sign up, and try Krate? Per day, humans only, CI and cloud machines set
/// apart, and every sign-in attempt beside the ones that finished.
async function people(env, days) {
  const W = `timestamp > now() - INTERVAL '${days}' DAY`;
  const W7 = "timestamp > now() - INTERVAL '7' DAY";
  const human = "blob1 = 'view' AND blob5 = 'human'";
  const [views, steps, usage, pages, sources, countries, legacy] = await Promise.all([
    aeRows(env, `SELECT toDate(timestamp) AS day, blob6 AS ev, count(DISTINCT index1) AS uniq, sum(_sample_interval) AS n FROM krate_usage WHERE ${human} AND ${W} GROUP BY day, ev`),
    aeRows(env, `SELECT toDate(timestamp) AS day, blob1 AS kind, blob2 AS step, blob3 AS outcome, sum(_sample_interval) AS n FROM krate_usage WHERE blob1 IN ('signin', 'try') AND blob2 != '' AND ${W} GROUP BY day, kind, step, outcome`),
    aeRows(env, `SELECT toDate(timestamp) AS day, blob1 AS action, blob9 AS net, sum(_sample_interval) AS n, count(DISTINCT index1) AS machines FROM krate_usage WHERE blob1 IN ('install', 'make', 'open', 'publish') AND ${W} GROUP BY day, action, net`),
    aeRows(env, `SELECT blob2 AS k, count(DISTINCT index1) AS v FROM krate_usage WHERE ${human} AND blob6 = 'view' AND ${W7} GROUP BY k ORDER BY v DESC LIMIT 12`),
    aeRows(env, `SELECT blob3 AS k, count(DISTINCT index1) AS v FROM krate_usage WHERE ${human} AND blob3 != '-' AND blob3 != '' AND ${W7} GROUP BY k ORDER BY v DESC LIMIT 12`),
    aeRows(env, `SELECT blob4 AS k, count(DISTINCT index1) AS v FROM krate_usage WHERE ${human} AND ${W7} GROUP BY k ORDER BY v DESC LIMIT 12`),
    // Page loads from before visitor counting: no visitor, no bot flag.
    aeRows(env, `SELECT toDate(timestamp) AS day, sum(_sample_interval) AS n FROM krate_usage WHERE blob1 = 'view' AND blob5 = '' AND ${W} GROUP BY day`),
  ]);

  const byDay = {};
  const today = new Date();
  for (let i = 0; i < days; i++) {
    const d = new Date(today.getTime() - i * 86400_000).toISOString().slice(0, 10);
    byDay[d] = {
      day: d, visitors: 0, views: 0, views_old: 0, downloads: 0,
      signin_tries: 0, signin_done: 0, signin_failed: 0,
      make_tries: 0, accounts_new: 0,
      installs: 0, installs_ci: 0, installs_unmarked: 0,
      opens: 0, makes: 0, publishes: 0,
    };
  }
  const at = (d) => byDay[String(d).slice(0, 10)];
  for (const r of views || []) {
    const row = at(r.day); if (!row) continue;
    if (r.ev === "download") row.downloads += Number(r.n);
    else { row.visitors += Number(r.uniq); row.views += Number(r.n); }
  }
  for (const r of legacy || []) {
    const row = at(r.day); if (row) row.views_old += Number(r.n);
  }
  for (const r of steps || []) {
    const row = at(r.day); if (!row) continue;
    const n = Number(r.n);
    if (r.kind === "try") { row.make_tries += n; continue; }
    if (String(r.step).endsWith("-start")) row.signin_tries += n;
    if (String(r.step).endsWith("-done") && r.outcome === "ok") row.signin_done += n;
    if (r.outcome === "error") row.signin_failed += n;
  }
  for (const r of usage || []) {
    const row = at(r.day); if (!row) continue;
    const real = r.net === "home";
    if (r.action === "install") {
      if (real) row.installs += Number(r.machines);
      else if (r.net === "cloud") row.installs_ci += Number(r.machines);
      else row.installs_unmarked += Number(r.machines);
    } else if (real) {
      const key = { open: "opens", make: "makes", publish: "publishes" }[r.action];
      row[key] += Number(r.n);
    }
  }

  // How sign-ins went in the last 7 days, per way in (google, github,
  // email ...): a sign-in is an attempt, not a person, and the gap between
  // started and finished is people who never got an account.
  const weekAgo = new Date(Date.now() - 7 * 86400000).toISOString().slice(0, 10);
  const ways = {};
  for (const r of steps || []) {
    if (r.kind === "try" || String(r.day).slice(0, 10) < weekAgo) continue;
    const m = /^(.+)-(start|done)$/.exec(String(r.step || ""));
    if (!m) continue;
    const w = (ways[m[1]] = ways[m[1]] || { way: m[1], started: 0, finished: 0, failed: 0 });
    const n = Number(r.n) || 0;
    if (m[2] === "start") w.started += n;
    else if (r.outcome === "ok") w.finished += n;
    if (r.outcome === "error") w.failed += n;
  }

  // Accounts come from KV itself, so they are right even for the days
  // before this counting existed.
  const listing = await env.APPS.list({ prefix: "user:", limit: 1000 });
  const accounts = [];
  for (const k of listing.keys) {
    const u = JSON.parse((await env.APPS.get(k.name)) || "null");
    if (!u) continue;
    accounts.push({ name: u.name, login: u.login, email: u.email, providers: u.providers, created: u.created });
    const row = at(new Date(u.created || 0).toISOString());
    if (row) row.accounts_new += 1;
  }
  accounts.sort((a, b) => (b.created || 0) - (a.created || 0));

  return {
    days: Object.values(byDay),
    accounts_total: accounts.length,
    signin_ways_7d: Object.values(ways).sort((a, b) => b.started - a.started),
    newest_accounts: accounts.slice(0, 30),
    top_pages_7d: pages,
    top_sources_7d: sources,
    top_countries_7d: countries,
    analytics: views === null ? "unavailable (CF_ANALYTICS_TOKEN missing or the query failed)" : "ok",
    notes: [
      "Visitors are people, not page loads: one browser on one connection counts once a day. Bots, crawlers, link previews and scripts are left out.",
      "Visitor, source, download and sign-in-attempt counting started 2026-09-30. Before that there are only raw page loads (views_old), bots included, and only from the pages that sent them.",
      "Installs, opens, makes and publishes count only home or office connections. CI runners and cloud machines are in installs_ci. Rows from before 2026-09-30 carry no network mark (installs_unmarked).",
    ],
  };
}

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
.sbox{margin-top:8px}.sess{border-top:1px solid var(--line);padding:8px 0}.sess summary{cursor:pointer}
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
  <button data-t="welcome" class="on">Welcome <span id="waiting" class="pill" style="margin-left:4px;display:none"></span></button>
  <button data-t="people">People</button>
  <button data-t="tickets">Tickets</button>
  <button data-t="reports">Reports</button>
  <button data-t="users">Users</button>
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
  try{const o=await api("overview");el("#who").textContent=\`\${o.users} users · \${o.tickets} tickets · \${o.reports||0} reports\`;el("#app").style.display="";const first=location.hash==="#people"?"people":"welcome";document.querySelectorAll("nav button").forEach(x=>x.classList.toggle("on",x.dataset.t===first));show(first);}
  catch(e){el("#who").textContent="not signed in, or not an admin";el("#login").style.display="";}
}
document.querySelectorAll("nav button").forEach(b=>b.onclick=()=>{document.querySelectorAll("nav button").forEach(x=>x.classList.remove("on"));b.classList.add("on");show(b.dataset.t);});
// A person's Studio sessions with the whole conversation, newest first; the
// one the ticket or report names is opened and marked.
async function sessionsInto(box,q,want){
  box.innerHTML='<p class="mut">loading sessions…</p>';
  let r;try{r=await api("person/sessions?"+q);}catch(e){box.innerHTML='<p class="mut">Could not load sessions ('+esc(e.message)+').</p>';return;}
  const S=r.sessions||[];
  if(!S.length){box.innerHTML='<p class="mut">'+esc(r.note||"No Studio sessions saved for this person.")+'</p>';return;}
  box.innerHTML=S.map(x=>{const msgs=x.messages||x.thread||[];const hit=want&&x.id===want;
    return '<details class="sess"'+(hit||S.length===1?' open':'')+'><summary>'+(hit?'<span class="pill open">this one</span> ':'')+'<b>'+esc(x.title||x.id)+'</b> <span class="mut">'+esc(x.id)+' · '+msgs.length+' messages · '+when((x.updated||0)*(x.updated>1e12?1:1000))+'</span></summary>'
      +msgs.map(m=>'<div class="msg '+(/you|user/i.test(m.who||m.role||"")?"user":"krate")+'">'+esc(m.body??m.text??m.content??JSON.stringify(m))+'<div class="mut">'+esc(m.who||m.role||"")+(m.when||m.at?' · '+when((m.when||m.at)*((m.when||m.at)>1e12?1:1000)):'')+'</div></div>').join("")
      +'</details>';}).join("");
}
// How long ago, in words a glance reads: "12 min ago", "3 h ago", "2 d ago".
const ago=ms=>{const s=Math.max(0,(Date.now()-ms)/1000);if(s<90)return"just now";if(s<5400)return Math.round(s/60)+" min ago";if(s<129600)return Math.round(s/3600)+" h ago";return Math.round(s/86400)+" d ago";};
async function show(tab){
  const v=el("#view");v.innerHTML='<p class="mut">loading…</p>';
  if(tab==="welcome"){
    const W=await api("welcomes");
    const badge=el("#waiting");badge.textContent=W.waiting;badge.style.display=W.waiting?"":"none";
    const waiting=W.people.filter(p=>!p.welcomed&&p.email),done=W.people.filter(p=>p.welcomed),none=W.people.filter(p=>!p.email);
    const row=p=>'<div class="card row" data-id="'+esc(p.id)+'"><div class="grow"><b>'+esc(p.name||p.login||p.email)+'</b> <span class="mut">'+esc(p.email)+' · '+esc(p.providers.join(", "))+' · joined '+ago(p.created)+'</span>'
      +(p.welcomed?'<div class="mut">welcomed '+ago(p.welcomed)+'</div>':'')+'</div>'
      +(p.compose&&!p.welcomed?'<a class="act" style="text-decoration:none;padding:7px 13px;border-radius:8px;background:var(--acc);color:#fff;font-size:12.5px" target="_blank" rel="noopener" href="'+esc(p.compose)+'">Write the welcome</a>':'')
      +(p.email?'<button class="ghost mark">'+(p.welcomed?"Not welcomed":"Mark welcomed")+'</button>':'')+'</div>';
    v.innerHTML='<p class="mut" style="margin-bottom:10px">Everyone who made an account, newest first. "Write the welcome" opens Gmail as you, the note written; send it, then mark them welcomed. You also get an email the moment someone signs up.</p>'
      +'<h3 style="margin:14px 0 8px">Waiting for a welcome ('+waiting.length+')</h3>'+(waiting.length?waiting.map(row).join(""):'<p class="mut">Nobody waiting.</p>')
      +'<h3 style="margin:18px 0 8px">Welcomed ('+done.length+')</h3>'+(done.length?done.map(row).join(""):'<p class="mut">Nobody yet.</p>')
      +(none.length?'<h3 style="margin:18px 0 8px">No email given ('+none.length+')</h3>'+none.map(row).join(""):'');
    v.querySelectorAll(".card[data-id]").forEach(c=>{
      const id=c.dataset.id,p=W.people.find(x=>x.id===id);
      const link=c.querySelector("a.act");if(link)link.addEventListener("click",()=>setTimeout(()=>{c.querySelector(".mark").textContent="Mark welcomed";c.querySelector(".mark").classList.add("act");},500));
      const m=c.querySelector(".mark");if(m)m.onclick=async()=>{await api("welcomed",{method:"POST",body:JSON.stringify({id,welcomed:!p.welcomed})});show("welcome");};
    });
  }
  if(tab==="people"){
    const P=await api("people?days=30");
    const D=P.days;
    const last7=D.slice(0,7);
    const sum=(rows,k)=>rows.reduce((a,r)=>a+(r[k]||0),0);
    const card=(label,value,hint)=>'<div class="card" style="flex:1;min-width:150px"><div class="mut">'+label+'</div><div style="font-size:26px;font-weight:600">'+value+'</div><div class="mut">'+hint+'</div></div>';
    let h='<p class="mut" style="margin-bottom:10px">Last 7 days, real people only. Analytics: '+esc(P.analytics)+'</p>';
    h+='<div class="row" style="align-items:stretch;margin-bottom:10px">'
      +card("Visitors",sum(last7,"visitors"),"people on krate.tech, counted once a day")
      +card("Download clicks",sum(last7,"downloads"),"from the site")
      +card("Sign-ins finished",sum(last7,"signin_done")+" of "+sum(last7,"signin_tries"),"attempts, not people: one person can sign in many times · "+Math.max(0,sum(last7,"signin_tries")-sum(last7,"signin_done"))+" never finished · "+sum(last7,"signin_failed")+" failed on our side")
      +card("New accounts",sum(last7,"accounts_new"),"people: "+P.accounts_total+" accounts in all, one per person, made at their first finished sign-in")
      +card("Tried making",sum(last7,"make_tries"),"make attempts, desktop and web")
      +card("Real installs",sum(last7,"installs"),sum(last7,"installs_ci")+" CI/cloud set apart")
      +'</div>';
    const cols=[["day","Day"],["visitors","Visitors"],["views","Page views"],["views_old","Old page loads"],["downloads","Downloads"],["signin_tries","Sign-in starts"],["signin_done","Sign-ins done"],["signin_failed","Failed (ours)"],["accounts_new","New accounts"],["make_tries","Make attempts"],["installs","Installs"],["installs_ci","CI/cloud"],["installs_unmarked","Unmarked"],["opens","Opens"],["publishes","Publishes"]];
    h+='<div class="card" style="overflow-x:auto"><table style="border-collapse:collapse;width:100%;font-size:12.5px"><tr>'+cols.map(c=>'<th style="text-align:right;padding:4px 8px;color:var(--mut);font-weight:500;white-space:nowrap">'+c[1]+'</th>').join("")+'</tr>';
    for(const r of D){h+='<tr>'+cols.map(c=>{const v=r[c[0]];const z=(v===0);return '<td style="text-align:right;padding:4px 8px;border-top:1px solid var(--line);'+(z?'color:#475066':'')+'">'+esc(v)+'</td>';}).join("")+'</tr>';}
    h+='</table></div>';
    h+='<div class="card"><b>Newest accounts</b>'+(P.newest_accounts.length?'':'<p class="mut">None yet.</p>')+P.newest_accounts.map(a=>'<div class="row" style="padding:6px 0;border-top:1px solid var(--line)"><span class="grow"><b>'+esc(a.name||a.login||a.email)+'</b> <span class="mut">'+esc(a.email||"")+' · '+esc((a.providers||[]).join(", "))+'</span></span><span class="mut">'+when(a.created)+'</span></div>').join("")+'</div>';
    const list=(title,rows)=>'<div class="card" style="flex:1;min-width:220px"><b>'+title+'</b>'+((rows&&rows.length)?rows.map(x=>'<div class="row" style="padding:3px 0"><span class="grow">'+esc(x.k)+'</span><span class="mut">'+esc(x.v)+'</span></div>').join(""):'<p class="mut">Nothing yet.</p>')+'</div>';
    h+='<div class="card"><b>How sign-ins went, 7 days</b><p class="mut" style="margin:4px 0 8px">Started and never finished means that person has no account. Studio on a computer needs no sign-in, so its users are not here unless they publish.</p>'+((P.signin_ways_7d||[]).length?P.signin_ways_7d.map(w=>'<div class="row" style="padding:4px 0;border-top:1px solid var(--line)"><span class="grow"><b>'+esc(w.way)+'</b></span><span>'+w.finished+' of '+w.started+' finished</span>'+(w.failed?'<span class="mut"> · '+w.failed+' failed on our side</span>':'')+'</div>').join(""):'<p class="mut">No sign-ins in the last 7 days.</p>')+'</div>';
    h+='<div class="row" style="align-items:stretch">'+list("Pages, 7 days",P.top_pages_7d)+list("Where they came from, 7 days",P.top_sources_7d)+list("Countries, 7 days",P.top_countries_7d)+'</div>';
    h+='<div class="card mut">'+P.notes.map(esc).join("<br>")+'</div>';
    v.innerHTML=h;
  }
  if(tab==="tickets"){
    const {tickets}=await api("tickets");
    v.innerHTML=tickets.length?"":'<p class="mut">No tickets.</p>';
    for(const t of tickets){
      // Tickets sent before the session was its own field name it in the
      // words Studio adds: "[Studio web · ... · session s-123]".
      if(!t.session){const m=/session ([A-Za-z0-9_-]{1,64})\]/.exec((t.messages[0]||{}).text||"");if(m)t.session=m[1];}
      const d=document.createElement("div");d.className="card";
      d.innerHTML=\`<div class="row"><b>\${esc(t.subject)}</b>\${t.priority?'<span class="pill" style="border-color:#5a4a17;background:#2b2410;color:#e7c766">priority</span>':''}<span class="pill \${t.status}">\${t.status}</span><span class="grow"></span><span class="mut">\${esc(t.login||t.email)} · \${when(t.updated)}</span></div>
      <div>\${t.messages.map(m=>\`<div class="msg \${m.who}">\${esc(m.text)}<div class="mut">\${m.who==="krate"?"you":"them"} · \${when(m.at)}</div></div>\`).join("")}</div>
      <div class="row" style="margin-top:8px"><textarea placeholder="Reply…"></textarea></div>
      <div class="row" style="margin-top:8px"><button class="act">Send reply</button><button class="ghost">\${t.status==="open"?"Close":"Reopen"}</button><button class="ghost sbtn">\${t.session?"Open the session":"Their sessions"}</button>\${t.session?'<span class="mut">session '+esc(t.session)+'</span>':''}</div>
      <div class="sbox"></div>\`;
      d.querySelector(".sbtn").onclick=()=>sessionsInto(d.querySelector(".sbox"),t.userId?"id="+encodeURIComponent(t.userId):"login="+encodeURIComponent(t.login||""),t.session);
      d.querySelector(".act").onclick=async()=>{const x=d.querySelector("textarea").value.trim();if(!x)return;await api("ticket/reply",{method:"POST",body:JSON.stringify({id:t.id,text:x})});show("tickets");};
      d.querySelector(".ghost").onclick=async()=>{await api("ticket/status",{method:"POST",body:JSON.stringify({id:t.id,status:t.status==="open"?"closed":"open"})});show("tickets");};
      v.appendChild(d);
    }
  }
  if(tab==="reports"){
    const {reports}=await api("reports");
    v.innerHTML=reports.length?'<p class="mut" style="margin-bottom:10px">Sent from Studio with its evidence. Newest first.</p>':'<p class="mut">No reports.</p>';
    for(const r of reports){
      const d=document.createElement("div");d.className="card";
      d.innerHTML='<div class="row"><b>'+esc(r.name||r.from)+'</b> <span class="mut">@'+esc(r.from)+'</span><span class="pill '+(r.state==="done"?"closed":"open")+'">'+esc(r.state||"new")+'</span><span class="grow"></span><span class="mut">'+when((r.received||0)*1000)+'</span></div>'
        +'<div class="mut" style="margin:6px 0">session '+esc(r.session||"none")+' · Krate '+esc(r.krate||"?")+' · '+esc(r.os||"?")+' · '+Math.round((r.size||0)/1024)+' KB · id '+esc(r.id)+'</div>'
        +(r.note?'<div class="msg user">'+esc(r.note)+'</div>':'')
        +'<div class="row" style="margin-top:8px"><button class="act dl">Download evidence</button><button class="ghost ses">'+(r.session?"Open the session":"Their sessions")+'</button><button class="ghost st">'+(r.state==="done"?"Reopen":"Mark done")+'</button></div><div class="sbox"></div>';
      d.querySelector(".dl").onclick=async()=>{const res=await fetch(HUB+"/admin/report/"+r.id,{headers:{authorization:"Bearer "+TOKEN}});if(!res.ok){alert("Download failed: "+res.status);return;}const b=await res.blob();const a=document.createElement("a");a.href=URL.createObjectURL(b);a.download="krate-report-"+r.id+".zip";a.click();if(!r.state||r.state==="new")api("report/state",{method:"POST",body:JSON.stringify({id:r.id,state:"seen"})});};
      d.querySelector(".ses").onclick=()=>sessionsInto(d.querySelector(".sbox"),"login="+encodeURIComponent(r.from||""),r.session);
      d.querySelector(".st").onclick=async()=>{await api("report/state",{method:"POST",body:JSON.stringify({id:r.id,state:r.state==="done"?"seen":"done"})});show("reports");};
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

// ============================================================ organisations
//
// The hub half of the organisation pilot (IC-946), to the contract in
// Plan/org-pilot/CONTRACT.md. Every route here is a thin door: it proves
// who is asking (the same GitHub sign-in `krate publish` uses), reads and
// judges what was sent, and hands one command to the organisation's
// Durable Object, which decides. The answer is JSON; a refusal is
// `{ error: <stable reason>, message: <plain words> }`.
//
//   POST   /orgs                                  {slug, name}
//   GET    /orgs/<slug>
//   POST   /orgs/<slug>/invites                   {login, role}
//   DELETE /orgs/<slug>/invites/<code>
//   POST   /orgs/<slug>/invites/<code>/accept
//   PUT    /orgs/<slug>/members/<login>           {role}
//   DELETE /orgs/<slug>/members/<login>
//   PUT    /orgs/<slug>/root-key                  {public_key, revocations?}
//   POST   /orgs/<slug>/candidates                (bundle bytes)
//   GET    /orgs/<slug>/candidates                (history, for members who publish or approve)
//   POST   /orgs/<slug>/candidates/<digest>/approve
//   POST   /orgs/<slug>/candidates/<digest>/decline {reason}      (org-decline.js)
//   PUT    /orgs/<slug>/candidates/<digest>/note    {note}        (org-notes.js)
//   GET    /orgs/<slug>/requests, POST {text}, PUT /<id>          (org-requests.js)
//   GET    /orgs/<slug>/notifications, GET/PUT /me/notify         (org-notify.js)
//   GET    /orgs/<slug>/releases
//   GET    /orgs/<slug>/audit
//   GET    /a/<hash>                              (a private release: see fetchPrivate)

/// A refusal in the contract's shape.
function orgError(status, reason, slug, detail) {
  return json({ error: reason, message: messageFor(reason, slug, detail) }, status);
}

/// The answer to a request with no session (IC-946): `not-signed-in`, or
/// the anonymous wall's 429 once this caller has sent too many. Never
/// reaches an organisation's store.
async function anonymousOrgRefusal(request, env, status, slug) {
  const over = await rateLimited(request, env, "org/anonymous");
  if (over) {
    return new Response(JSON.stringify({ error: "rate-limited", message: "too many requests -- wait a minute and try again" }), {
      status: 429,
      headers: { "content-type": "application/json", "retry-after": over.headers.get("retry-after") || "60" },
    });
  }
  const res = orgError(status, "not-signed-in", slug);
  res.headers.set("cache-control", "private, no-store");
  return res;
}

/// Ask organisation `slug`'s store one command. Answers `{ status, body }`.
async function orgAsk(env, slug, command) {
  const stub = env.ORGS.get(env.ORGS.idFromName(`org:${slug}`));
  const res = await stub.fetch("https://org/command", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ ...command, slug }),
  });
  return res.json();
}

function orgAnswer(answer) {
  if (answer.status === 204 || answer.body === null) return new Response(null, { status: answer.status });
  return json(answer.body, answer.status);
}

/// The Idempotency-Key and what it is bound to: the method, the path and
/// the body, hashed, so the same key on a different request is caught.
async function idempotencyOf(request, path, bodyText) {
  const key = (request.headers.get("idempotency-key") || "").trim();
  if (!key) return null;
  return {
    key: key.slice(0, 200),
    hash: await sha256Hex(new TextEncoder().encode(`${request.method} ${path}\n${bodyText}`)),
  };
}

/// Where these bytes already live, and for whom: `{ exists, org }`. `org`
/// is the organisation whose private release or candidate they are, or
/// null for public bytes. Read from the R2 object's own metadata, for the
/// one-request bytes at their hash and the big tier's at big/<id>.
async function bytesHolder(env, hash) {
  let object = await env.BUNDLES.head(hash);
  if (!object) {
    let blob = null;
    try {
      blob = await env.APPS.get(`bigkey:${hash}`);
    } catch (_) {}
    if (blob) object = await env.BUNDLES.head(blob);
  }
  if (!object) return { exists: false, org: null };
  return { exists: true, org: (object.customMetadata && object.customMetadata.org) || null };
}

/// What the organisation's store is told about a judged candidate. The
/// signature is the validator's whole answer, chain included; the store
/// decides whether it is signed for the organisation.
function candidateArgs(judgement, digest, size, base) {
  const app = (judgement.manifest && judgement.manifest.app) || {};
  return {
    digest,
    size,
    name: app.name || "",
    // Which app it is a version of: the current release is per app id (IC-949, R).
    app_id: app.id || "",
    version: (judgement.release && judgement.release.version) || app.version || "",
    namespace: (judgement.release && judgement.release.namespace) || "",
    signature: judgement.signature || { state: "absent" },
    // The organisation the bundle's mark names, or null (Addendum 5).
    mark: judgement.org_mark || null,
    // What it asks to do, in plain words, for the approver (IC-950).
    capabilities: judgementCaps(judgement),
    // The company connections it asks for (IC-952).
    connections: connectionsAsked(judgement),
    // How it was made, from its signed build record (IC-954).
    provenance: provenanceArg(judgement.provenance),
    base,
  };
}

/// The public door's refusal of an organisation's private app (Addendum 5):
/// a marked bundle is published only with `krate publish --org`.
function privateAppRefusal(judgement) {
  const org = judgement && judgement.org_mark;
  if (!org) return null;
  return json(
    { error: "private-app", message: `this app is private to ${org}: publish it with krate publish --org ${org}` },
    422,
  );
}

/// The site whose pages the hub links to: join links, the private-release
/// open page. krate.tech, unless a local proof says otherwise.
function siteBase(env) {
  return String(env.SITE_BASE || "https://krate.tech").replace(/\/$/, "");
}

/// The KV display index of one principal's organisations (Addendum 1:
/// `GET /orgs`). Display only: it never answers who may do what, and
/// opening an organisation asks its store (R5). Written after the store
/// has committed; a failed write costs a stale list, never access.
async function noteOrgOf(env, principal, slug, name, role) {
  if (!principal) return;
  const key = `orgs-of:${principal}`;
  try {
    let list = [];
    try {
      list = JSON.parse((await env.APPS.get(key)) || "[]");
    } catch (_) {}
    const next = list.filter((o) => o && o.slug !== slug);
    if (role) next.push({ slug, name: name || slug, role });
    next.sort((a, b) => a.slug.localeCompare(b.slug));
    if (JSON.stringify(next) !== JSON.stringify(list)) {
      if (next.length) await env.APPS.put(key, JSON.stringify(next));
      else await env.APPS.delete(key);
    }
  } catch (_) {
    // The list is a convenience; the store holds the truth.
  }
}

/// Every organisation route, behind the refused-attempt limit (IC-948 B):
/// a principal with 30 refusals this minute gets 429 before any store is
/// asked, and each refusal the store gives is counted.
async function orgRoute(request, url, env, ctx) {
  const identity = await verifyGitHub(request, env);
  const principals = principalsOf(identity);
  if (principals.length && (await refusalsExhausted(env, principals))) return rateLimitedAnswer();
  const res = await orgRouteCore(request, url, env, identity, ctx);
  if (principals.length && isRefusal(res.status)) await noteRefusal(env, principals);
  return res;
}

async function orgRouteCore(request, url, env, identity, ctx) {
  if (!env.ORGS) {
    return json({ error: "orgs-off", message: "organisations are not switched on on this hub" }, 503);
  }
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const site = siteBase(env);
  const parts = url.pathname.split("/").filter(Boolean).map((p) => {
    try {
      return decodeURIComponent(p);
    } catch (_) {
      return p;
    }
  });
  const method = request.method;
  // Who is asking, as principals (IC-947). A session that proved nothing
  // an organisation can name -- an email account that has not yet signed
  // in by link or verified Google since principals arrived -- is treated
  // like no session at all.
  const principals = principalsOf(identity);
  const signedIn = principals.length > 0;
  const readBody = async () => {
    const read = await readBounded(request, 64 * 1024);
    if (read.tooLarge) return { tooLarge: true };
    const text_ = new TextDecoder().decode(read.body);
    let value = {};
    try {
      value = text_ ? JSON.parse(text_) : {};
    } catch (_) {
      return { bad: true };
    }
    return { text: text_, value: value && typeof value === "object" ? value : {} };
  };
  const bad = () => json({ error: "bad-request", message: "the body is JSON, under 64 KiB" }, 400);

  if (parts.length === 1) {
    // GET /orgs: the signed-in person's organisations, for display.
    if (method === "GET") {
      if (!signedIn) return anonymousOrgRefusal(request, env, 401, "");
      const bySlug = new Map();
      for (const principal of principals) {
        let list = [];
        try {
          list = JSON.parse((await env.APPS.get(`orgs-of:${principal}`)) || "[]");
        } catch (_) {}
        for (const o of list) if (o && o.slug && !bySlug.has(o.slug)) bySlug.set(o.slug, { slug: o.slug, name: o.name, role: o.role });
      }
      return json([...bySlug.values()].sort((a, b) => a.slug.localeCompare(b.slug)));
    }
    // POST /orgs
    if (method !== "POST") return text("not found", 404);
    const body = await readBody();
    if (body.tooLarge || body.bad) return bad();
    const slug = String(body.value.slug || "").toLowerCase();
    if (!SLUG.test(slug)) return orgError(400, "slug-invalid", slug);
    if (!signedIn) return anonymousOrgRefusal(request, env, 401, slug);
    const idempotency = await idempotencyOf(request, url.pathname, body.text);
    const answer = await orgAsk(env, slug, { op: "create", principals, args: { name: body.value.name, time_zone: body.value.time_zone }, idempotency });
    if (answer.status === 201 && answer.body.membership) {
      await noteOrgOf(env, answer.body.membership.principal, slug, answer.body.org.name, "owner");
    }
    return orgAnswer(answer);
  }

  const slug = String(parts[1] || "").toLowerCase();
  if (!SLUG.test(slug)) return orgError(400, "slug-invalid", slug);
  // GET /orgs/<slug>/invites/<code>/door (IC-958 C): which way in an open
  // invitation's join page should offer first, before anyone signs in --
  // the company's sign-in when the organisation has one, and whether the
  // invitation is for a GitHub account or an inbox. Only for a code that
  // is open now; it names nothing else, and writes nothing.
  if (parts.length === 5 && parts[2] === "invites" && parts[4] === "door" && method === "GET") {
    const over = await rateLimited(request, env, "org/anonymous");
    if (over) return over;
    const answer = await orgAsk(env, slug, { op: "invite-door", principals: [], args: { code: String(parts[3] || "") } });
    const res = orgAnswer(answer);
    res.headers.set("cache-control", "private, no-store");
    return res;
  }
  // No session: answered here, never by the organisation's store, so an
  // anonymous flood appends nothing to its audit and creates no object.
  // Only an authenticated principal's attempts are audited.
  if (!signedIn) return anonymousOrgRefusal(request, env, 401, slug);
  const rest = parts.slice(2);
  const askRaw = (op, args = {}, idempotency = null) => orgAsk(env, slug, { op, principals, args, idempotency });
  const ask = async (op, args = {}, idempotency = null) => orgAnswer(await askRaw(op, args, idempotency));
  const mutateRaw = async (op, argsOf) => {
    const body = await readBody();
    if (body.tooLarge || body.bad) return { status: 400, body: { error: "bad-request", message: "the body is JSON, under 64 KiB" } };
    const idempotency = await idempotencyOf(request, url.pathname, body.text);
    return askRaw(op, argsOf(body.value), idempotency);
  };
  const mutate = async (op, argsOf) => orgAnswer(await mutateRaw(op, argsOf));
  // Work that must not hold up or fail the answer -- notification mail --
  // runs after it when the platform allows; a test, with no context, waits.
  const later = (pending) => {
    const safe = Promise.resolve(pending).catch((e) => console.error("after the answer:", e && e.message ? e.message : String(e)));
    if (ctx && typeof ctx.waitUntil === "function") {
      ctx.waitUntil(safe);
      return Promise.resolve();
    }
    return safe;
  };
  const notices = { env, smtpSend, askRaw, site };

  // Company sign-in and offline access (IC-948, F and G).
  {
    const routed = { request, env, slug, rest, method, askRaw, readBody, idempotencyOf, path: url.pathname, answer: orgAnswer };
    const mine = (await ssoRoute(routed)) || (await leaseRoute(routed));
    if (mine) return mine;
  }

  if (rest.length === 0 && method === "GET") return ask("show");
  // GET /orgs/<slug>/available (IC-958 B1): is this short name free, for
  // the create form's live check. Asks nothing of anyone's membership, so
  // checking a name records nothing in that organisation's activity.
  if (rest.length === 1 && rest[0] === "available" && method === "GET") return ask("exists");
  if (rest[0] === "invites") {
    if (rest.length === 1 && method === "POST") {
      const key = (request.headers.get("idempotency-key") || "").trim().slice(0, 200);
      const answer = await mutateRaw("invite", (b) => ({ github: b.github, email: b.email, role: b.role, site }));
      if (answer.status !== 201 || answer.replayed) return orgAnswer(answer);
      // Mailed when it can be; a failed send never fails the invitation.
      const show = await askRaw("show");
      const mail = await mailInvite(env, smtpSend, answer.body, {
        orgName: (show.body && show.body.org && show.body.org.name) || slug,
        slug,
        inviter: (show.body && show.body.me && show.body.me.display) || "Someone",
        orgSlot: async () => Boolean((await askRaw("invite-mail-slot")).body.allowed),
      });
      if (key) await askRaw("invite-mailed", { idempotency_key: key, ...mail });
      return json({ ...answer.body, ...mail }, 201);
    }
    if (rest.length === 2 && method === "GET") return ask("invite-info", { code: rest[1], site });
    if (rest.length === 2 && method === "DELETE") return mutate("revoke-invite", () => ({ code: rest[1] }));
    if (rest.length === 3 && rest[2] === "accept" && method === "POST") {
      const answer = await mutateRaw("accept", () => ({ code: rest[1] }));
      if (answer.status === 200 && answer.body.membership) {
        const m = answer.body.membership;
        await noteOrgOf(env, m.principal, slug, answer.body.org && answer.body.org.name, m.role);
      }
      return orgAnswer(answer);
    }
  }
  if (rest[0] === "members" && rest.length === 2) {
    // The member is named by principal, URL-encoded: github%3Aalice.
    const target = rest[1];
    if (method === "PUT") {
      const answer = await mutateRaw("set-role", (b) => ({ principal: target, role: b.role }));
      if (answer.status === 200 && answer.body.membership) {
        const m = answer.body.membership;
        await noteOrgOf(env, m.principal, slug, answer.body.org && answer.body.org.name, m.role);
      }
      return orgAnswer(answer);
    }
    if (method === "DELETE") {
      const answer = await mutateRaw("remove", () => ({ principal: target }));
      if (answer.status === 204) await noteOrgOf(env, String(target).toLowerCase(), slug, null, null);
      return orgAnswer(answer);
    }
  }
  if (rest[0] === "root-key" && rest.length === 1 && method === "PUT") {
    const body = await readBody();
    if (body.tooLarge || body.bad) return bad();
    const publicKey = String(body.value.public_key || "").trim().toLowerCase();
    let revocations;
    if (body.value.revocations !== undefined && body.value.revocations !== null) {
      // A list `krate revoke` wrote, checked by the Rust validator: signed
      // by the root it names, and that root is the one being registered.
      const list = readRevocations(new TextEncoder().encode(JSON.stringify(body.value.revocations)));
      if (!list.ok) return orgError(422, "not-signed-for-this-org", slug, `the revocation list does not hold: ${list.problem}`);
      if (list.root !== publicKey) {
        return orgError(422, "not-signed-for-this-org", slug, "the revocation list is signed by a different root than the key being registered");
      }
      revocations = list.revocations;
    }
    const idempotency = await idempotencyOf(request, url.pathname, body.text);
    return ask("set-root", { public_key: publicKey, revocations }, idempotency);
  }
  if (rest[0] === "candidates") {
    if (rest.length === 1 && method === "POST") {
      const res = await orgCandidate(request, env, slug, principals, url.pathname, base);
      // A new version waits: the people who may approve it hear (org-notify.js).
      if (res.status === 201) {
        const made = await res.clone().json().catch(() => null);
        if (made && made.digest && made.state === "waiting-for-approval") await later(sendNotices(notices, { event: "waiting", digest: made.digest }));
      }
      return res;
    }
    if (rest.length === 1 && method === "GET") return ask("candidates");
    if (rest.length === 3 && rest[2] === "approve" && method === "POST") {
      const digest = String(rest[1]).toLowerCase();
      if (!/^[0-9a-f]{64}$/.test(digest)) {
        return orgError(404, "digest-mismatch", slug, "a digest is the candidate's 64-hex sha256");
      }
      // `accept`: the new capabilities the approver confirmed (IC-950).
      const answer = await mutateRaw("approve", (b) => ({ digest, base, accept: Array.isArray(b && b.accept) ? b.accept.slice(0, 200).map(String) : [] }));
      // Its sender hears it is out; never holds up or fails the approval.
      if (answer.status === 200 && !answer.replayed) await later(sendNotices(notices, { event: "approved", digest }));
      return orgAnswer(answer);
    }
  }
  if (rest[0] === "releases" && rest.length === 1 && method === "GET") return ask("releases", { base });
  if (rest[0] === "audit" && rest.length === 1 && method === "GET") {
    const answer = await askRaw("audit");
    if (answer.status !== 200) return orgAnswer(answer);
    // Recomputed here with crypto.subtle -- a second implementation from
    // the one that wrote the chain -- every time it is read.
    const events = answer.body.events;
    return json({ events, chain_ok: await chainOk(events) });
  }
  const h = {
    request, url, env, slug, rest, method, principals, identity, site, base,
    askRaw, mutateRaw, orgAsk, orgAnswer, json, readBody, readBounded, isAdmin, smtpSend, fetchBundle, idempotencyOf,
    validate: callValidator, later, notices,
  };
  const routed =
    (await orgLifecycleRoute(h)) || (await orgBillingRoute(h)) || (await inventoryRoute(h)) || (await insightsRoute(h)) || (await orgTimeRoute(h)) || (await orgBrandRoute(h)) ||
    (await orgDeclineRoute(h)) || (await orgNotesRoute(h)) || (await orgRequestsRoute(h)) || (await orgNotifyRoute(h)) ||
    (await orgReleaseRoute(h)) || (await connectionRoute(h)) ||
    (await orgScimRoute(h)) || (await auditExportRoute(h)) || (await orgSamlRoute(h)) ||
    (await orgKeysRoute(h)) ||
    (await orgSigningRoute(h)) ||
    (await orgDataRoute(h));
  if (routed) return routed;
  return text("not found", 404);
}

/// POST /orgs/<slug>/candidates: the bundle bytes, as `krate publish`
/// sends them to /publish (IC-946). The same archive check and the same
/// validator as the publish door; then the organisation decides whether
/// this person may publish there and whether the app is signed for it,
/// before a byte is stored; then the bytes are stored under their hash
/// with the organisation's flag; then the candidate is recorded. An app
/// over 5 MiB goes through the big-file path with `org` in its start.
async function orgCandidate(request, env, slug, principals, path, base) {
  const declared = request.headers.get("content-length");
  if (declared !== null && Number(declared) > MAX_UPLOAD_BYTES) {
    return json(
      { error: "too-large", message: "apps over 5 MiB go through the big-file path: POST /publish/big/start with \"org\"" },
      413,
    );
  }
  const read = await readBounded(request, MAX_UPLOAD_BYTES);
  if (read.tooLarge) {
    return json(
      { error: "too-large", message: "apps over 5 MiB go through the big-file path: POST /publish/big/start with \"org\"" },
      413,
    );
  }
  const body = read.body;
  const note = noteHeader(request.headers.get("x-krate-note"));
  if (note.bad) return json({ error: "bad-request", message: note.bad }, 400);
  const invalid = (problem) => json({ error: "invalid-bundle", message: `not a valid .krate bundle: ${problem}` }, 422);
  if (body.length === 0) return invalid("the body is empty");
  const problem = archiveProblem(body);
  if (problem) return invalid(problem);
  const judged = judgeBundle(body);
  if (!judged.ok) return invalid(judged.problem);
  const digest = await sha256Hex(body);
  const idempotency = await idempotencyOf(request, path, digest);
  const args = candidateArgs(judged.judgement, digest, body.length, base);
  if (note.value) args.note = note.value;

  const check = await orgAsk(env, slug, { op: "check-candidate", principals, args, idempotency });
  if (check.replayed || check.status !== 200) return orgAnswer(check);

  const holder = await bytesHolder(env, digest);
  if (holder.exists && holder.org !== slug) {
    return orgError(409, "bytes-held-elsewhere", slug, "these exact bytes are already held outside this organisation, so they cannot be its private release");
  }
  const stored = await storeBundle(env, digest, body, { org: slug });
  if (stored.problem) return json({ error: "store-failed", message: stored.problem }, 500);
  // The web pack for this candidate (IC-956), so an approved release opens
  // in a member's tab at once. Stored beside the bytes; served only behind
  // the same organisation checks as the bytes.
  await ensureWebPack(env, digest, body, judgedComponent);
  return orgAnswer(await orgAsk(env, slug, { op: "add-candidate", principals, args, idempotency }));
}

/// GET /a/<hash> for an organisation's private release (IC-946, R1, R8).
/// The organisation's store is asked on every fetch, with the person's
/// own session; a copied URL alone carries none, so it downloads nothing.
/// No session, a stranger, a removed member and a digest that is only a
/// candidate are each refused with 403 and a stable reason. Never cached
/// by anything between: the answer depends on who is asking.
///
/// A browser (Accept: text/html) is sent to the site's open page instead
/// (Addendum 2, item 5): the hub cannot see the site's sign-in and sets no
/// cookies, so the page asks again with Authorization and renders the
/// state from the stable reason. The redirect names no organisation.
async function fetchPrivate(request, url, hash, object, slug, big, env) {
  const drop = async () => {
    try {
      if (object.body && typeof object.body.cancel === "function") await object.body.cancel();
    } catch (_) {}
  };
  const accept = request.headers.get("accept") || "";
  // A page navigation only: Accept includes text/html AND no Authorization.
  // Never keyed on the user agent -- the dashboards' "Open in Krate" is a
  // fetch() from a browser tab, with a browser's UA, `*/*` and the
  // person's session, and it must get the bytes.
  if (accept.includes("text/html") && !request.headers.get("authorization")) {
    await drop();
    return new Response(null, {
      status: 303,
      headers: {
        location: `${siteBase(env)}/open/?a=${hash}&private=1`,
        "cache-control": "private, no-store",
        vary: "accept, authorization",
      },
    });
  }
  // Accept: application/json is the site's open page asking what this is
  // (Addendum 2, item 6): the same checks, an answer with no bytes. The CLI
  // and every download send no such Accept and get the bytes.
  const look = accept.includes("application/json");
  const checked = await privateAnswer(request, hash, slug, env, look);
  const noStore = { "cache-control": "private, no-store", vary: "accept, authorization" };
  if (checked.refused) {
    await drop();
    return checked.refused;
  }
  const answer = checked.answer;
  if (look) {
    await drop();
    return new Response(
      JSON.stringify({
        name: answer.body.name,
        version: answer.body.version,
        size: object.size,
        org: { slug, name: answer.body.org_name },
        offline: answer.body.offline,
      }),
      { headers: { "content-type": "application/json", ...noStore } },
    );
  }
  const name = slugOf(answer.body.name || "") || hash.slice(0, 12);
  return new Response(request.method === "HEAD" ? null : object.body, {
    headers: {
      "content-type": "application/octet-stream",
      "content-disposition": `attachment; filename="${name}.krate"`,
      "content-length": String(object.size),
      ...(big ? { "x-krate-tier": "big" } : {}),
      ...noStore,
    },
  });
}

/// The organisation's answer for one person and one private release: who
/// is asking (their own session), the refused-attempt limit, and the
/// store's decision. `{ answer }` when the store said yes, `{ refused }`
/// with the Response to send otherwise. One function for /a/<hash> and the
/// Player's /a/<hash>/web (IC-956), so a tab gets exactly the checks a
/// download gets -- and an open in a tab is audited as the download it is.
async function privateAnswer(request, hash, slug, env, look) {
  const identity = await verifyGitHub(request, env);
  const principals = principalsOf(identity);
  if (!principals.length) {
    // A copied URL with no session: refused without asking the store, so
    // it is neither audited nor able to flood the audit.
    return { refused: await anonymousOrgRefusal(request, env, 403, slug) };
  }
  if (await refusalsExhausted(env, principals)) {
    return { refused: rateLimitedAnswer() };
  }
  const answer = env.ORGS
    ? await orgAsk(env, slug, { op: look ? "view" : "download", principals, args: { digest: hash } })
    : { status: 403, body: { error: "not-a-member", message: "organisations are not switched on on this hub" } };
  if (answer.status === 200) return { answer };
  await noteRefusal(env, principals);
  // A removed member already knows which organisation sent the app, so
  // that refusal names it; anyone else holding a copied link learns no
  // organisation at all (R8) -- the store's own words for a stranger name
  // the slug, so they are replaced here.
  const reason = answer.body && answer.body.error;
  const body =
    reason === "removed"
      ? { ...answer.body, org: slug }
      : reason === "not-a-member"
        ? { error: reason, message: "this app was not shared with you -- ask whoever sent the link to invite you" }
        : reason === "digest-mismatch"
          ? { error: reason, message: "this app is not released yet" }
          : answer.body;
  // A withdrawn release is gone, not forbidden (IC-949, R): 410.
  return {
    refused: new Response(JSON.stringify(body), {
      status: reason === "release-withdrawn" ? 410 : 403,
      headers: { "content-type": "application/json", "cache-control": "private, no-store", vary: "accept, authorization" },
    }),
  };
}

/// The Player's private-release check: the refusal Response, or null.
async function privateRefusal(request, hash, object, slug, env) {
  const checked = await privateAnswer(request, hash, slug, env, false);
  if (!checked.refused) return null;
  try {
    if (object.body && typeof object.body.cancel === "function") await object.body.cancel();
  } catch (_) {}
  return checked.refused;
}

/// The stored bundle a content address names, as /a/<hash> finds it: a
/// takedown answers for it, the big tier's bytes are found by their record,
/// and an organisation's private release says whose it is.
/// `{ response }` when there is nothing to serve, else
/// `{ object, privateOrg, big }`.
async function bundleObject(hash, env) {
  if (!/^[0-9a-f]{64}$/.test(hash)) return { response: json({ error: "not-found" }, 404) };
  const takedown = await env.APPS.get(`takedown:${hash}`);
  if (takedown) return { response: takedownResponse(takedown, env) };
  let object = await env.BUNDLES.get(hash);
  let big = false;
  if (!object) {
    const blob = await env.APPS.get(`bigkey:${hash}`);
    if (blob) {
      object = await env.BUNDLES.get(blob);
      big = Boolean(object);
    }
  }
  if (!object) return { response: json({ error: "not-found" }, 404) };
  return { object, privateOrg: (object.customMetadata && object.customMetadata.org) || null, big };
}

// The door's pieces, exported so a test can watch them work rather than
// infer it from a status code: a bounded inflate and a whole inflate both
// answer 422 to a bomb, and only a byte count tells them apart (IC-833).
export { archiveProblem, canonicalRecords, judgeBundle, readBounded, zipDirectory };

// ------------------------------------------------------------- durability
//
// The hub's state lives in two places that can drift: R2 holds the bytes,
// keyed by their SHA-256; KV holds the control records that point at them
// (listings, aliases, channels, takedowns). Nothing reconciled them, and
// nothing could tell a missing object from a never-published one, or a
// corrupt object from a good one (IC-852). This is the inventory, the
// reconciliation, the independent export and the restore, as admin routes
// and a scheduled run.
//
// Repairs are the ones that cannot lose anything: a listing or alias that
// points at bytes which no longer exist is removed (it served 404s), a
// channel is settled (its own rule). Bytes are never deleted by a scrub --
// an orphan object and a digest mismatch are REPORTED, because deleting
// on the strength of a reconciliation is how a bug in the reconciliation
// becomes data loss.

/// The KV families that are the registry's control state. Sessions,
/// logins, entitlements and counters are deliberately not here: they are
/// not the registry, and an export must not carry secrets.
const CONTROL_FAMILIES = ["app:", "alias:", "channel:", "takedown:", "takedown-closed:"];
const EXPORT_SCHEMA = "krate.hub-export.v1";
const SCRUB_SCHEMA = "krate.hub-scrub.v1";

async function listAllKeys(kv, prefix) {
  const out = [];
  let cursor;
  for (let page = 0; page < 200; page += 1) {
    const listing = await kv.list({ prefix, limit: 1000, cursor });
    out.push(...listing.keys.map((k) => k.name));
    if (listing.list_complete || !listing.cursor) break;
    cursor = listing.cursor;
  }
  return out;
}

async function listAllObjects(bucket) {
  const out = [];
  let cursor;
  for (let page = 0; page < 200; page += 1) {
    // customMetadata, so an organisation's private bytes (IC-946) are
    // recognised as owned: they have no listing, alias or channel by
    // design, and would otherwise read as orphans.
    const listing = await bucket.list({ limit: 1000, cursor, include: ["customMetadata"] });
    for (const object of listing.objects) {
      const org = (object.customMetadata && object.customMetadata.org) || null;
      out.push({ key: object.key, size: object.size, sha256: checksumHex(object), org });
    }
    if (!listing.truncated || !listing.cursor) break;
    cursor = listing.cursor;
  }
  return out;
}

/// Inventory the bucket and the control records, reconcile them, and
/// (with `repair`) apply the repairs that cannot lose anything. The
/// report is stored under `scrub:last` for /health and /admin/scrub/last.
async function runScrub(env, { repair }) {
  const at = Math.floor(Date.now() / 1000);
  const problems = [];
  const objects = await listAllObjects(env.BUNDLES);
  const bundles = new Map(objects.filter((o) => /^[0-9a-f]{64}$/.test(o.key)).map((o) => [o.key, o]));
  // A big-tier app's bytes live at big/<id>, named by its bigkey record
  // (IC-945): present when that object is. Without this a repairing scrub
  // would delete every big listing as "bytes gone".
  {
    const byKey = new Map(objects.map((o) => [o.key, o]));
    for (const key of await listAllKeys(env.APPS, "bigkey:")) {
      const blob = await env.APPS.get(key);
      const object = blob ? byKey.get(blob) : null;
      if (object) bundles.set(key.slice("bigkey:".length), { ...object, sha256: null });
    }
  }

  // 1896: every content-addressed object hashes to its own key when the
  // store recorded a digest; one that does not is corrupt and is named.
  for (const object of bundles.values()) {
    if (object.sha256 && object.sha256 !== object.key) {
      problems.push({ kind: "digest-mismatch", key: object.key, detail: `stored digest ${object.sha256.slice(0, 12)}... differs from the key`, repaired: false });
    }
  }

  const referenced = new Set();
  // 1869: dangling listings and aliases.
  const listings = await listAllKeys(env.APPS, "app:");
  for (const key of listings) {
    const hash = key.slice(4);
    referenced.add(hash);
    if (!bundles.has(hash)) {
      let repaired = false;
      if (repair) {
        await env.APPS.delete(key);
        repaired = true;
      }
      problems.push({ kind: "dangling-listing", key, detail: "listed, but its bytes are gone", repaired });
    }
  }
  const aliases = await listAllKeys(env.APPS, "alias:");
  for (const key of aliases) {
    const hash = await env.APPS.get(key);
    if (hash) referenced.add(hash);
    if (!hash || !bundles.has(hash)) {
      let repaired = false;
      if (repair) {
        await env.APPS.delete(key);
        repaired = true;
      }
      problems.push({ kind: "dangling-alias", key, detail: hash ? "points at bytes that are gone" : "points at nothing", repaired });
    }
  }
  const channels = await listAllKeys(env.APPS, "channel:");
  for (const key of channels) {
    const raw = await env.APPS.get(key);
    let channel = null;
    try {
      channel = raw ? JSON.parse(raw) : null;
    } catch (_) {
      channel = null;
    }
    if (!channel || !channel.current) {
      problems.push({ kind: "unreadable-channel", key, detail: "not a channel record", repaired: false });
      continue;
    }
    for (const move of channel.history || []) referenced.add(move.hash);
    if (!bundles.has(channel.current)) {
      let repaired = false;
      if (repair) {
        await settleChannel(env, channel, "scrub");
        repaired = true;
      }
      problems.push({ kind: "dangling-channel", key, detail: `names ${channel.current.slice(0, 12)}..., whose bytes are gone`, repaired });
    }
  }
  const takedowns = await listAllKeys(env.APPS, "takedown:");
  for (const key of takedowns) referenced.add(key.slice("takedown:".length));
  const closed = await listAllKeys(env.APPS, "takedown-closed:");
  for (const key of closed) referenced.add(key.slice("takedown-closed:".length));

  // Orphans: bytes nothing points at. Reported, never removed here -- a
  // retired listing keeps its bytes on purpose, and so may a link that
  // was only ever sent by hand. An organisation's private bytes (IC-946)
  // are owned by that organisation's store, which no KV record mirrors on
  // purpose: never an orphan, and counted on their own.
  let orgObjects = 0;
  for (const [hash, object] of bundles) {
    if (object.org) {
      orgObjects += 1;
      continue;
    }
    if (!referenced.has(hash)) {
      problems.push({ kind: "orphan-object", key: hash, detail: "no listing, alias, channel or takedown names it", repaired: false });
    }
  }

  const report = {
    schema: SCRUB_SCHEMA,
    at,
    repair,
    objects: objects.length,
    bundles: bundles.size,
    listings: listings.length,
    aliases: aliases.length,
    channels: channels.length,
    takedowns: takedowns.length,
    org_objects: orgObjects,
    problems,
  };
  try {
    await env.APPS.put("scrub:last", JSON.stringify(report));
  } catch (_) {
    // A quota day must not turn a scrub into a failure; the report is
    // still returned to whoever asked.
  }
  return report;
}

async function scrubRoute(request, env, repair) {
  if (!(await isAdmin(request, env))) return text("not found", 404);
  return json(await runScrub(env, { repair }));
}

async function lastScrub(request, env) {
  if (!(await isAdmin(request, env))) return text("not found", 404);
  const raw = await env.APPS.get("scrub:last");
  if (!raw) return json({ schema: SCRUB_SCHEMA, at: null, problems: null, note: "no scrub has run" });
  return new Response(raw, { headers: { "content-type": "application/json" } });
}

/// /health says "ok" and, when the last scrub found problems, says so:
/// the alert a person or a monitor can read (1886).
async function health(env) {
  let raw = null;
  try {
    raw = await env.APPS.get("scrub:last");
  } catch (_) {
    raw = null;
  }
  if (raw) {
    try {
      const report = JSON.parse(raw);
      const open = (report.problems || []).filter((p) => !p.repaired);
      if (open.length > 0) {
        const kinds = [...new Set(open.map((p) => p.kind))].join(", ");
        return text(`ok -- the last scrub found ${open.length} problem(s): ${kinds} (see /admin/scrub/last)`);
      }
    } catch (_) {
      // an unreadable report is not a reason to say the hub is down
    }
  }
  return text("ok");
}

/// Canonical bytes of an export's records, for its digest: families in a
/// fixed order, keys sorted, values as stored.
function canonicalRecords(records) {
  const out = [];
  for (const family of CONTROL_FAMILIES) {
    const keys = Object.keys(records[family] || {}).sort();
    for (const key of keys) {
      out.push(`${key}\n${records[family][key]}\n`);
    }
  }
  return new TextEncoder().encode(out.join(""));
}

/// Every control record, with a digest over them, so a restore can prove
/// it holds exactly what was exported (1890).
async function exportControlRecords(env) {
  const records = {};
  let count = 0;
  for (const family of CONTROL_FAMILIES) {
    records[family] = {};
    for (const key of await listAllKeys(env.APPS, family)) {
      const value = await env.APPS.get(key);
      if (value === null) continue;
      records[family][key] = value;
      count += 1;
    }
  }
  const digest = await sha256Hex(canonicalRecords(records));
  return { schema: EXPORT_SCHEMA, at: Math.floor(Date.now() / 1000), count, digest, records };
}

async function exportRoute(request, env) {
  if (!(await isAdmin(request, env))) return text("not found", 404);
  const dump = await exportControlRecords(env);
  return new Response(JSON.stringify(dump), {
    headers: {
      "content-type": "application/json",
      "content-disposition": `attachment; filename="krate-hub-export-${dump.at}.json"`,
      "cache-control": "no-store",
    },
  });
}

/// Put an export's records back. Only into keys that are absent, unless
/// `overwrite` was asked for; the digest must verify first, so a dump
/// edited in transit restores nothing.
async function restoreControlRecords(env, dump, { overwrite }) {
  if (!dump || dump.schema !== EXPORT_SCHEMA || typeof dump.records !== "object") {
    return { error: "not a hub export" };
  }
  const digest = await sha256Hex(canonicalRecords(dump.records));
  if (digest !== dump.digest) {
    return { error: `the export's digest does not match its records (${digest.slice(0, 12)}... vs ${String(dump.digest).slice(0, 12)}...); nothing restored` };
  }
  let restored = 0;
  let skipped = 0;
  for (const family of CONTROL_FAMILIES) {
    for (const [key, value] of Object.entries(dump.records[family] || {})) {
      if (!key.startsWith(family) || typeof value !== "string") {
        return { error: `record ${key} is not a ${family} record; nothing more restored` };
      }
      if (!overwrite && (await env.APPS.get(key)) !== null) {
        skipped += 1;
        continue;
      }
      await env.APPS.put(key, value);
      restored += 1;
    }
  }
  return { restored, skipped, digest };
}

async function restoreRoute(request, url, env) {
  if (!(await isAdmin(request, env))) return text("not found", 404);
  let dump;
  try {
    dump = await request.json();
  } catch (_) {
    return text("the body must be a hub export (JSON)", 400);
  }
  const result = await restoreControlRecords(env, dump, { overwrite: url.searchParams.has("overwrite") });
  if (result.error) return text(result.error, 400);
  return json(result);
}
