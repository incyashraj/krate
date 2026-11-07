// Whether a person wants Krate's non-essential mail, and the one-click way
// to stop it.
//
// Every mail the hub sends is one of two kinds:
//
//   essential      the person asked for it, or it is about access or
//                  security. Always sent; the switch below never touches it.
//                    sign-in link                  index.js emailStart
//                    team invitation an admin sent org-mail.js mailInvite
//                    "support was asked to add an owner" (security)
//                                                  org-mail.js mailRecovery
//                    team notifications (approval waiting, version decided,
//                    requests, security) -- work mail with its own per-team
//                    switch, "Turn it off under Notifications"
//                                                  org-notify.js
//                    "New on Krate: <who>" -- to the founder only
//                                                  index.js notifyFounderOfSignup
//                    billing receipts are Stripe's own mail, not ours.
//
//   non-essential  nobody asked for this one mail. Each checks the switch
//                  first, carries the unsubscribe line as its last line and
//                  RFC 8058 one-click headers.
//                    the founder's welcome         index.js sendWelcome
//                    "<app> is live"               mail.js tellPublished
//                    "Your app is ready"           mail.js tellMade
//                    "Someone just opened <app>"   mail.js tellFirstOpen
//                    announcements                 mail-announce.js
//
// The switch is `mail_news: false` on the account record (KV `user:<id>`),
// written only when it changes; no field means on.
//
// The link carries the account id and an HMAC of it, so it works without
// signing in. The HMAC key is derived from KEY_WRAP_SECRET with its own
// label, so it can never be confused with the key that wraps API keys. A
// hub without that secret cannot make a link, and then sends no
// non-essential mail at all: a mail with no way out is not sent.
//
//   GET  /mail/unsubscribe?u=<id>&t=<token>   a page that asks; changes nothing
//                                              (mail scanners fetch links)
//   POST /mail/unsubscribe?u=<id>&t=<token>   do=stop | do=resume (the page's
//                                              buttons), or the one-click body
//                                              List-Unsubscribe=One-Click
//   GET  /me/mail                              {news} for the signed-in account
//   PUT  /me/mail {news: true|false}           -> {news}

const LABEL = "krate mail unsubscribe v1";
export const SETTINGS_URL = "https://krate.tech/account/#mail";

/// Whether this account still wants non-essential mail.
export function wantsNews(user) {
  return !(user && user.mail_news === false);
}

async function hmacHex(keyBytes, message) {
  const key = await crypto.subtle.importKey("raw", keyBytes, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const sig = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(message));
  return [...new Uint8Array(sig)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// The token for one account, or null when the hub has no secret.
export async function unsubscribeToken(env, accountId) {
  const secret = env.KEY_WRAP_SECRET || "";
  if (!secret || !accountId) return null;
  // Domain separation: a sub-key for this purpose only, then the account.
  const sub = await hmacHex(new TextEncoder().encode(secret), LABEL);
  return (await hmacHex(new TextEncoder().encode(sub), `account:${accountId}`)).slice(0, 32);
}

/// Constant-time-ish comparison of two hex strings.
function same(a, b) {
  if (typeof a !== "string" || typeof b !== "string" || a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

export async function tokenValid(env, accountId, token) {
  if (!/^[A-Za-z0-9_.-]{1,64}$/.test(String(accountId || ""))) return false;
  const want = await unsubscribeToken(env, accountId);
  return Boolean(want) && same(want, String(token || ""));
}

/// The one-click link for an account, or null when none can be made.
export async function unsubscribeLink(env, accountId) {
  const token = await unsubscribeToken(env, accountId);
  if (!token) return null;
  const base = String(env.PUBLIC_BASE || "https://hub.krate.tech").replace(/\/$/, "");
  return `${base}/mail/unsubscribe?u=${encodeURIComponent(accountId)}&t=${token}`;
}

export const unsubscribeLine = (link) => `Don't want these emails? Unsubscribe: ${link}`;

/// The RFC 8058 headers a non-essential mail carries.
export function unsubscribeHeaders(link) {
  return { "List-Unsubscribe": `<${link}>`, "List-Unsubscribe-Post": "List-Unsubscribe=One-Click" };
}

/// What a non-essential mail to this account needs, or why it must not go.
/// `{link, headers, footer}` when it may go; `{skip}` when not, logged.
export async function newsMail(env, accountId, user, what = "a mail") {
  if (!wantsNews(user)) {
    console.log(`${what} was not sent: the account turned Krate's emails off`);
    return { skip: "turned off" };
  }
  const link = await unsubscribeLink(env, accountId);
  if (!link) {
    console.error(`${what} was not sent: this hub has no KEY_WRAP_SECRET, so no unsubscribe link can be made`);
    return { skip: "no unsubscribe link" };
  }
  return { link, headers: unsubscribeHeaders(link), footer: `\n\n${unsubscribeLine(link)}` };
}

/// Turn the switch; writes the record only when it changes. Answers the
/// new value, or throws when the write fails.
export async function setNews(env, accountId, user, news) {
  if (wantsNews(user) === news) return news;
  if (news) delete user.mail_news;
  else user.mail_news = false;
  user.mail_news_changed = Date.now();
  await env.APPS.put(`user:${accountId}`, JSON.stringify(user));
  return news;
}

// ---------------------------------------------------------------- the page

const esc = (s) =>
  String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/// One small card on krate.tech's look, inline (a page that leans on
/// another host's stylesheet breaks when that host does).
function page({ title, lines, action, button, status = 200 }) {
  const form = action
    ? `<form method="post"><input type="hidden" name="do" value="${esc(action)}"><button class="btn" type="submit">${esc(button)}</button></form>`
    : "";
  return new Response(
    `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>${esc(title)}</title>
<link rel="icon" href="https://krate.tech/krate-favicon.png">
<style>
:root { color-scheme: light dark; --bg:#fbfbfd; --card:#fff; --ink:#1c1c1e; --mute:#6b6b72; --faint:#8a8a92;
  --btn:#1c1c1e; --btn-ink:#fff; --accent:#3d6df0; --shadow:0 1px 2px rgba(0,0,0,.04),0 12px 40px rgba(0,0,0,.07); }
@media (prefers-color-scheme: dark) { :root { --bg:#08080a; --card:#18181b; --ink:#f4f4f5; --mute:#8e8e96; --faint:#7c7c85;
  --btn:#f4f4f5; --btn-ink:#08080a; --accent:#6f94ff; --shadow:0 0 0 1px rgba(255,255,255,.06); } }
* { box-sizing:border-box; }
body { margin:0; min-height:100vh; background:var(--bg); color:var(--ink);
  font:400 16px/1.55 Geist,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; -webkit-font-smoothing:antialiased; }
.panel { width:min(420px, 100% - 32px); margin:72px auto 0; padding:36px 32px 32px; border-radius:26px;
  background:var(--card); box-shadow:var(--shadow); text-align:center; }
.mk { width:52px; height:52px; display:block; margin:0 auto 18px; }
h1 { margin:0; font-size:28px; line-height:1.2; letter-spacing:-.035em; text-wrap:balance; }
p { margin:8px 0 0; font-size:15px; color:var(--mute); text-wrap:pretty; }
form { margin:24px 0 0; }
.btn { width:100%; height:44px; border:0; border-radius:999px; font:500 15px/1 inherit; font-family:inherit;
  color:var(--btn-ink); background:var(--btn); cursor:pointer; }
.btn:active { transform:scale(.97); }
.fine { margin-top:18px; font-size:13px; color:var(--faint); }
a { color:var(--accent); text-decoration:none; } a:hover { text-decoration:underline; }
@media (max-width:480px) { .panel { margin-top:40px; padding:28px 22px 24px; } }
</style></head><body>
<main class="panel">
<a href="https://krate.tech/"><img class="mk" src="https://krate.tech/krate-mark-3d-96.png" alt="Krate" width="52" height="52"></a>
<h1>${esc(title)}</h1>
${lines.map((l) => `<p>${esc(l)}</p>`).join("\n")}
${form}
<p class="fine"><a href="${SETTINGS_URL}">Your email settings</a></p>
</main></body></html>`,
    { status, headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store", "referrer-policy": "no-referrer" } },
  );
}

const STILL = "Sign-in links and team invitations still arrive.";

function answer(state) {
  switch (state) {
    case "ask":
      return page({
        title: "Stop emails from Krate?",
        lines: ["Tips, the welcome note and news about your apps.", STILL],
        action: "stop",
        button: "Stop these emails",
      });
    case "already-off":
      return page({
        title: "These emails are off",
        lines: ["You don't get tips, the welcome note or news about your apps.", STILL],
        action: "resume",
        button: "Turn them back on",
      });
    case "stopped":
      return page({ title: "Done.", lines: [`You won't get these emails any more. ${STILL}`], action: "resume", button: "Undo" });
    case "resumed":
      return page({
        title: "Emails are back on",
        lines: ["You'll hear about your apps again."],
        action: "stop",
        button: "Stop them again",
      });
    case "failed":
      return page({ title: "That did not go through", lines: ["Nothing changed. Try again in a minute."], status: 503 });
    default:
      return page({
        title: "This link does not work",
        lines: ["It may have been copied only in part. Sign in and turn emails off in your settings instead."],
        status: 400,
      });
  }
}

async function readUser(env, accountId) {
  return JSON.parse((await env.APPS.get(`user:${accountId}`)) || "null");
}

/// GET and POST /mail/unsubscribe.
export async function unsubscribeRoute(request, url, env) {
  const accountId = url.searchParams.get("u") || "";
  const token = url.searchParams.get("t") || "";
  if (!(await tokenValid(env, accountId, token))) {
    return request.method === "POST" ? new Response("this link does not work", { status: 400 }) : answer("bad");
  }
  const user = await readUser(env, accountId);
  if (!user) return request.method === "POST" ? new Response("this link does not work", { status: 400 }) : answer("bad");

  // A GET only asks: mail scanners open every link in a mail.
  if (request.method === "GET") return answer(wantsNews(user) ? "ask" : "already-off");

  let form = new URLSearchParams();
  try {
    form = new URLSearchParams(await request.text());
  } catch (_) {}
  const oneClick = form.get("List-Unsubscribe") === "One-Click";
  const action = oneClick ? "stop" : form.get("do");
  if (action !== "stop" && action !== "resume") return answer("ask");
  try {
    await setNews(env, accountId, user, action === "resume");
  } catch (e) {
    console.error(`could not change an account's mail switch: ${e}`);
    return oneClick ? new Response("try again later", { status: 503 }) : answer("failed");
  }
  // A mail provider's one-click POST reads nobody's page.
  if (oneClick) return new Response("unsubscribed", { status: 200, headers: { "content-type": "text/plain; charset=utf-8" } });
  return answer(action === "stop" ? "stopped" : "resumed");
}

/// GET and PUT /me/mail for the signed-in account.
export async function meMailRoute(request, env, authedUser) {
  const user = await authedUser(request, env);
  if (!user || !user.id) return json({ error: "signin", message: "Sign in first." }, 401);
  if (request.method === "GET") return json({ news: wantsNews(user) });
  const body = await request.json().catch(() => null);
  if (!body || typeof body.news !== "boolean") return json({ error: "bad-request", message: "Say {\"news\": true} or {\"news\": false}." }, 400);
  try {
    return json({ news: await setNews(env, user.id, user, body.news) });
  } catch (e) {
    console.error(`could not change an account's mail switch: ${e}`);
    return json({ error: "unavailable", message: "That did not save. Try again in a minute." }, 503);
  }
}

function json(value, status = 200) {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
}
