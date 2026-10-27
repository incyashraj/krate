// The receive loop's side of the hub (F-320: make an app, send it like a
// photo). Three things a person who was sent a link meets:
//
// 1. The link card. A chat app that is asked to show a link (WhatsApp,
//    iMessage, Slack, Discord, X, LinkedIn, Facebook, Telegram) fetches it
//    with its own user agent, reads the page's Open Graph tags and draws a
//    card. Before this every /a/<hash> link drew nothing: a previewer that
//    says text/html was sent to a page with no tags, and one that says */*
//    was handed the raw bundle. A person's browser and the runtime are not
//    affected: only the user agents below get the card.
// 2. The open, counted per person (K-1132). The Player tells the hub once
//    the app's first frame is on the screen: one Analytics Engine point of
//    kind "open" in the slots the native opens use, so one query counts
//    both, plus whether the person is the app's maker and which post's link
//    (`?via=`) they came from. No KV, no cookie, nothing kept on the device.
// 3. A report a stranger can send from the Player, with no account, into a
//    record the desk lists beside a one-click takedown (IC-669's record).
//
// Nothing here decides who may open what: the takedown, the private-release
// and the bytes checks stay in index.js, and run before any of this.

import { WEB_PACK_TAG } from "./web-player.js";

/// The link previewers, by their own user agents. iMessage's previewer
/// says facebookexternalhit and Twitterbot. Only previewers: an in-app
/// browser (LinkedInApp, FBAN, Instagram, Snapchat, Line) is a person and
/// keeps going where a browser goes.
export const PREVIEWERS =
  /facebookexternalhit|Facebot|meta-externalagent|Twitterbot|Slackbot|Slack-ImgProxy|LinkedInBot|WhatsApp\/|Discordbot|TelegramBot|Applebot|redditbot|Embedly|SkypeUriPreview|Iframely|Pinterestbot|Mastodon\/|Bluesky|Cardyb|kakaotalk-scrap|Google-PageRenderer|vkShare/i;

export function isPreviewer(request) {
  return PREVIEWERS.test(request.headers.get("user-agent") || "");
}

/// A post's tag (`?via=reddit`), as one plain word, or "-". It goes into the
/// dataset as it is, so it may carry nothing but a short word.
export function viaOf(raw) {
  const v = String(raw || "").trim().toLowerCase();
  return /^[a-z0-9][a-z0-9_-]{0,31}$/.test(v) ? v : "-";
}

/// `?via=<word>` to carry on, or "".
export function viaQuery(via, joiner = "?") {
  return via && via !== "-" ? `${joiner}via=${via}` : "";
}

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/// One printable line, cut at a word.
function line(raw, max) {
  const s = String(raw || "").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/g, " ").trim();
  if (s.length <= max) return s;
  const cut = s.slice(0, max);
  return `${cut.slice(0, cut.lastIndexOf(" ") > max * 0.6 ? cut.lastIndexOf(" ") : max).replace(/[\s,;:.-]+$/, "")}...`;
}

/// Where the app opens, from its web pack: "tab" when the hub made one,
/// "krate" when the transpiler refused it, "" when none was made yet.
async function opensIn(env, hash) {
  try {
    const pack = await env.BUNDLES.head(`web/${WEB_PACK_TAG}/${hash}`);
    if (!pack) return "";
    return pack.customMetadata && pack.customMetadata.failed ? "krate" : "tab";
  } catch (_) {
    return "";
  }
}

/// What the card says under the name: what the app is, and that a tap opens
/// it with nothing to install. Every clause is true of this app: an app the
/// tab cannot run is not said to open in the browser.
export function cardDescription(name, description, where) {
  const what = line(description, 160);
  const how = {
    tab: "opens in your browser with a tap, nothing to install.",
    krate: "opens as an app on Mac, Windows and Linux.",
    "": "tap to open it.",
  }[where];
  if (what) return `${/[.!?]$/.test(what) ? what : `${what}.`} Made with Krate: ${how}`;
  return `${name}, made with Krate: ${how}`;
}

/// The card a previewer gets for `/a/<hash>` (and `/run/<hash>`).
/// `privateOrg`: an organisation's private release, which gets the generic
/// card and no name or picture -- the link already says only that it is a
/// private app, and the card says no more.
export async function linkCard(env, hash, { privateOrg = null } = {}) {
  const base = (env.PUBLIC_BASE || "").replace(/\/$/, "");
  const link = `${base}/a/${hash}`;
  const card = `${base}/player/krate-card.png`;
  let title;
  let description;
  let image = card;
  let alt = "Made with Krate";
  let large = { width: 1200, height: 630 };
  if (privateOrg) {
    title = "A private app";
    description = "Shared with a team on Krate. Open the link and sign in with the account the team invited.";
  } else {
    let meta = {};
    try {
      meta = JSON.parse((await env.APPS.get(`app:${hash}`)) || "{}") || {};
    } catch (_) {
      meta = {};
    }
    title = line(meta.name, 80) || "A Krate app";
    description = cardDescription(title, meta.description, await opensIn(env, hash));
    if (await env.BUNDLES.head(`shot:${hash}`)) {
      image = `${base}/shot/${hash}`;
      alt = `${title}, a Krate app`;
      large = null;
    }
  }
  const tags = [
    `<meta property="og:type" content="website">`,
    `<meta property="og:site_name" content="Krate">`,
    `<meta property="og:title" content="${esc(title)}">`,
    `<meta property="og:description" content="${esc(description)}">`,
    `<meta property="og:url" content="${esc(link)}">`,
    `<meta property="og:image" content="${esc(image)}">`,
    ...(large ? [`<meta property="og:image:width" content="${large.width}">`, `<meta property="og:image:height" content="${large.height}">`] : []),
    `<meta property="og:image:alt" content="${esc(alt)}">`,
    `<meta name="twitter:card" content="summary_large_image">`,
    `<meta name="twitter:title" content="${esc(title)}">`,
    `<meta name="twitter:description" content="${esc(description)}">`,
    `<meta name="twitter:image" content="${esc(image)}">`,
    `<meta name="description" content="${esc(description)}">`,
    `<link rel="canonical" href="${esc(link)}">`,
  ];
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)} · Krate</title>
<meta name="robots" content="noindex">
${tags.join("\n")}
</head>
<body>
<p><a href="${esc(privateOrg ? link : `${base}/run/${hash}`)}">Open ${esc(title)}</a></p>
</body>
</html>`;
  return new Response(html, {
    headers: {
      "content-type": "text/html; charset=utf-8",
      // Short, so a takedown or a new shot reaches the next card soon.
      "cache-control": privateOrg ? "private, no-store" : "public, max-age=600",
      vary: "accept, user-agent",
      "x-robots-tag": "noindex",
    },
  });
}

/// Which stored app a hash names, without reading its bytes: its listing
/// (public or unlisted), or bare bytes (an organisation's private release,
/// or a listing its author took down). Null when nothing is stored.
async function whichApp(env, hash) {
  let record = null;
  try {
    record = JSON.parse((await env.APPS.get(`app:${hash}`)) || "null");
  } catch (_) {
    record = null;
  }
  if (record) return { record, scope: record.unlisted ? "unlisted" : "public" };
  let head = await env.BUNDLES.head(hash);
  if (!head) {
    const blob = await env.APPS.get(`bigkey:${hash}`);
    if (blob) head = await env.BUNDLES.head(blob);
  }
  if (!head) return null;
  return { record: null, scope: head.customMetadata && head.customMetadata.org ? "org" : "unlisted" };
}

const TAB_EVENTS = { open: "open", make: "badge" };

/// POST /tab {event, app, via}: the Player saying an app opened in a tab
/// (its first frame is on the screen), or that the person pressed "Make
/// your own". One Analytics Engine point, in the slots the native opens use:
///
///   blob1 open|badge, blob2 "-" (no Krate version), blob3 "tab" (the
///   client), blob4 ok, blob5 "-", blob6 day, blob7 "-", blob8 the app hash,
///   blob9 home|cloud, blob10 maker|other ("-" when the hub keeps no listing
///   to say whose it is), blob11 the post's tag, blob12 public|unlisted|org,
///   blob13 human|bot; index1 the daily visitor hash.
///
/// "maker" needs the Player's own sign-in on this origin; a maker who is not
/// signed in here reads as "other". Answers {maker} so the page knows too.
export async function tabEvent(request, env, d) {
  const quiet = () => new Response(null, { status: 204 });
  let body;
  try {
    body = await request.json();
  } catch (_) {
    return quiet();
  }
  const kind = TAB_EVENTS[body && body.event];
  const hash = String((body && body.app) || "");
  if (!kind || !/^[0-9a-f]{64}$/.test(hash)) return quiet();
  // A made-up address is not an open: nothing is counted for an app the hub
  // does not hold, so the gallery's counts cannot be fed invented ones.
  const app = await whichApp(env, hash);
  if (!app) return quiet();
  let maker = app.record ? "other" : "-";
  if (app.record && request.headers.get("authorization")) {
    const identity = await d.verifyGitHub(request, env).catch(() => null);
    const me = d.ownerOf(identity);
    if (me && me === d.ownerOfRecord(app.record)) maker = "maker";
  }
  if (env.USAGE) {
    env.USAGE.writeDataPoint({
      blobs: [
        kind,
        "-",
        "tab",
        "ok",
        "-",
        new Date().toISOString().slice(0, 10),
        "-",
        hash,
        d.netOf(request),
        maker,
        viaOf(body.via),
        app.scope,
        d.isBot(request) ? "bot" : "human",
      ],
      doubles: [1],
      indexes: [await d.visitorHash(request, env)],
    });
  }
  return new Response(JSON.stringify({ ok: true, maker: maker === "maker" }), {
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

/// What a person can say is wrong with an app, in the Player's words.
export const REPORT_REASONS = ["scam", "harmful", "copyright", "other"];

/// How many reports one app's record keeps word for word; past this they
/// are counted only.
const REPORTS_KEPT = 50;

/// POST /report/app/<hash> {why, note}: a report from anyone, no account.
///
/// One KV record per app (`app-report:<hash>`), so the desk lists apps, not
/// a pile of rows, and a takedown is one click beside it. The same person
/// (the daily visitor hash) reporting the same app again writes nothing:
/// KV's 1,000 writes a day are what sign-ins and publishes live on (K-952),
/// and a report must never be able to spend them faster than one per
/// person per app per day, under the per-minute wall index.js puts in
/// front of this.
export async function appReport(request, env, hash, d) {
  if (!/^[0-9a-f]{64}$/.test(hash)) return d.json({ error: "not-found" }, 404);
  const app = await whichApp(env, hash);
  if (!app) return d.json({ error: "not-found", message: "There is no app at this address." }, 404);
  const raw = await request.text().catch(() => "");
  if (raw.length > 4096) return d.json({ error: "too-long" }, 413);
  let body = {};
  try {
    body = JSON.parse(raw || "{}") || {};
  } catch (_) {
    return d.json({ error: "bad-request" }, 400);
  }
  const why = REPORT_REASONS.includes(body.why) ? body.why : null;
  if (!why) return d.json({ error: "why", message: `Say what is wrong: one of ${REPORT_REASONS.join(", ")}.` }, 400);
  const note = line(body.note, 500);
  const visitor = await d.visitorHash(request, env);
  const key = `app-report:${hash}`;
  let record = null;
  try {
    record = JSON.parse((await env.APPS.get(key)) || "null");
  } catch (_) {
    record = null;
  }
  const now = Math.floor(Date.now() / 1000);
  if (!record) {
    record = {
      hash,
      name: (app.record && line(app.record.name, 80)) || "",
      scope: app.scope,
      count: 0,
      reasons: {},
      reports: [],
      first: now,
      last: now,
      state: "new",
    };
  }
  if (record.reports.some((r) => r.visitor === visitor)) {
    return d.json({ ok: true, again: true });
  }
  const identity = request.headers.get("authorization") ? await d.verifyGitHub(request, env).catch(() => null) : null;
  record.count += 1;
  record.reasons[why] = (record.reasons[why] || 0) + 1;
  record.last = now;
  // A new report on an app the desk had closed opens it again.
  record.state = "new";
  if (record.reports.length < REPORTS_KEPT) {
    record.reports.push({
      why,
      note,
      at: now,
      visitor,
      country: String((request.cf && request.cf.country) || "-"),
      net: d.netOf(request),
      ...(identity && identity.login ? { login: String(identity.login) } : {}),
    });
  }
  await env.APPS.put(key, JSON.stringify(record));
  if (env.USAGE) {
    env.USAGE.writeDataPoint({
      blobs: ["report", why, "tab", "ok", "-", new Date().toISOString().slice(0, 10), "-", hash, d.netOf(request)],
      doubles: [1],
      indexes: [visitor],
    });
  }
  return d.json({ ok: true });
}

/// Every reported app for the desk, most recently reported first, each
/// with whether it is already taken down.
export async function appReportsList(env, listAllKeys) {
  const keys = await listAllKeys(env.APPS, "app-report:");
  const out = [];
  for (const key of keys) {
    let record = null;
    try {
      record = JSON.parse((await env.APPS.get(key)) || "null");
    } catch (_) {
      record = null;
    }
    if (!record) continue;
    record.taken_down = Boolean(await env.APPS.get(`takedown:${record.hash}`));
    out.push(record);
  }
  out.sort((a, b) => (b.last || 0) - (a.last || 0));
  return out;
}

/// The desk marks an app's reports seen or done.
export async function appReportState(env, hash, state) {
  if (!/^[0-9a-f]{64}$/.test(String(hash || ""))) return { status: 404, body: "no such report" };
  if (!["new", "seen", "done"].includes(state)) return { status: 400, body: "state is new, seen or done" };
  const key = `app-report:${hash}`;
  const record = JSON.parse((await env.APPS.get(key)) || "null");
  if (!record) return { status: 404, body: "no such report" };
  record.state = state;
  await env.APPS.put(key, JSON.stringify(record));
  return { status: 200, body: { ok: true } };
}
