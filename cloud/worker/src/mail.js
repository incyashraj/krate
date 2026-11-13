// Mail the hub sends a person about their own things: their app is live,
// the app they asked for is made, someone opened what they shared.
//
// Plain text, from Krate's own address on krate.tech (Resend signs it with
// DKIM and send.krate.tech passes SPF), replies to the founder. Every send
// is best effort: a mail that fails changes nothing about what it is about,
// and nothing here ever throws into a publish, a build or an open.
//
// Resend's plan counts mails per day, so each moment mails once: a publish
// only for bytes the hub did not hold, a build only the first time its case
// makes a file, an open only the first time someone else opens an app.
//
// All three are NON-ESSENTIAL (mail-prefs.js has the whole list): each
// asks the account's switch first, ends with the unsubscribe line, and
// carries the one-click headers.

import { addressFor } from "./org-mail.js";
import { newsMail } from "./mail-prefs.js";

export const KRATE_FROM = "Krate <hello@krate.tech>";
export const REPLY_TO = "incyashraj@gmail.com";
const SITE = "https://krate.tech";
const SIGN_OFF = "\n\nKrate\nkrate.tech\n\nReply to this mail and it reaches the person who builds Krate.";

/// One mail through Resend; null when it went, else why not. `html` goes
/// beside the text when given; `headers` are extra mail headers (the
/// one-click unsubscribe pair).
export async function sendMail(env, { to, subject, text, html, headers, from = KRATE_FROM, replyTo = REPLY_TO }) {
  if (!env.RESEND_API_KEY) return "mail is not set up on this hub";
  if (!to) return "no address";
  const mail = { from, to: [to], reply_to: replyTo, subject, text };
  if (html) mail.html = html;
  if (headers) mail.headers = headers;
  try {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: { authorization: `Bearer ${env.RESEND_API_KEY}`, "content-type": "application/json" },
      body: JSON.stringify(mail),
    });
    if (res.ok) return null;
    return `resend ${res.status} ${await res.text().catch(() => "")}`;
  } catch (e) {
    return `could not reach resend: ${e}`;
  }
}

/// Where an account is mailed: a proved inbox first, else its email.
export function accountAddress(user) {
  if (!user) return null;
  if (Array.isArray(user.verified_emails) && user.verified_emails.length) return user.verified_emails[0];
  return user.email || null;
}

/// The account that owns a listing, with where it is mailed:
/// `{id, user, to}`, or null when there is no account. An `account:<id>`
/// owner is that account; a `github:<login>` owner is the account its
/// GitHub sign-in belongs to, mailed where addressFor says.
export async function ownerAccount(env, owner) {
  const who = String(owner || "");
  try {
    let id = null;
    if (who.startsWith("account:")) id = who.slice("account:".length);
    else if (who.startsWith("github:")) id = await env.APPS.get(`ghlogin:${who.slice("github:".length)}`);
    if (!id) return null;
    const user = JSON.parse((await env.APPS.get(`user:${id}`)) || "null");
    if (!user) return null;
    const to = who.startsWith("github:") ? await addressFor(env, who) : accountAddress(user);
    return { id, user, to };
  } catch (_) {
    return null;
  }
}

/// Where the owner of a listing can be mailed, or null.
export async function addressOfOwner(env, owner) {
  const account = await ownerAccount(env, owner);
  return (account && account.to) || null;
}

/// One non-essential mail to an account: its switch asked first, the
/// unsubscribe line last, the one-click headers on. Null when it went.
async function sendNews(env, { id, user, to }, mail, what) {
  const news = await newsMail(env, id, user, what);
  if (news.skip) return news.skip;
  if (!(await momentAllowed(env, to))) return "over today's mail ceiling";
  const why = await sendMail(env, { to, subject: mail.subject, text: mail.text + news.footer, headers: news.headers });
  if (why) console.error(`${what} did not go: ${why}`);
  return why;
}

/// How many moment mails go in one day (UTC), hub-wide and to one inbox.
/// Every moment is caused by something a signed-in person can repeat --
/// publish another app, open another version -- and the moments share one
/// Resend account with the sign-in links. Without a ceiling, one account
/// publishing a hundred small apps spent the day's sending and nobody could
/// sign in by email until midnight. Sign-in mail never passes through here,
/// so these ceilings leave it the rest of the day's allowance.
export const MOMENTS_PER_DAY = 50;
export const MOMENTS_PER_INBOX = 5;

async function digest16(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(String(text).toLowerCase()));
  return [...new Uint8Array(d)].slice(0, 8).map((b) => b.toString(16).padStart(2, "0")).join("");
}

/// Whether one more moment mail may go to `to` today; counts it when so.
/// Two KV reads and two writes per mail that goes, nothing for one that
/// does not. A KV failure says no: a moment is a nicety, and the cost of
/// a mail that does not go is far smaller than an unbounded one.
export async function momentAllowed(env, to) {
  try {
    const day = new Date().toISOString().slice(0, 10);
    const perDay = Number(env.MAIL_MOMENTS_PER_DAY) || MOMENTS_PER_DAY;
    const perInbox = Number(env.MAIL_MOMENTS_PER_INBOX) || MOMENTS_PER_INBOX;
    const allKey = `mail-day:${day}:all`;
    const inboxKey = `mail-day:${day}:${await digest16(to)}`;
    const all = Number(await env.APPS.get(allKey)) || 0;
    const inbox = Number(await env.APPS.get(inboxKey)) || 0;
    if (all >= perDay || inbox >= perInbox) return false;
    const ttl = { expirationTtl: 2 * 24 * 3600 };
    await env.APPS.put(allKey, String(all + 1), ttl);
    await env.APPS.put(inboxKey, String(inbox + 1), ttl);
    return true;
  } catch (_) {
    return false;
  }
}

/// One slot of the hub's day for an announcement: the same hub-wide
/// ceiling the moments count against (no per-inbox one: an announcement
/// reaches each inbox once). One read and one write per mail that goes.
export async function daySlot(env) {
  try {
    const day = new Date().toISOString().slice(0, 10);
    const perDay = Number(env.MAIL_MOMENTS_PER_DAY) || MOMENTS_PER_DAY;
    const allKey = `mail-day:${day}:all`;
    const all = Number(await env.APPS.get(allKey)) || 0;
    if (all >= perDay) return false;
    await env.APPS.put(allKey, String(all + 1), { expirationTtl: 2 * 24 * 3600 });
    return true;
  } catch (_) {
    return false;
  }
}

/// "Your app is live": the link, what it does when opened, and where it is.
export function publishedMail({ name, url, listed }) {
  return {
    subject: `${name} is live on Krate`,
    text: `Your app ${name} is live. Send this link to anyone:

${url}

It opens in any browser, on a phone or a computer, with nothing to install, and shows what it asks for before it runs. With Krate installed, the same app runs natively on Mac, Windows and Linux.

${listed ? `It is in the gallery too: ${SITE}/cloud/` : "It is unlisted: only people with the link can find it."}${SIGN_OFF}`,
  };
}

/// "Your app is ready": a build finished, maybe after the person left.
export function madeMail({ request }) {
  const asked = String(request || "").trim();
  return {
    subject: "Your app is ready",
    text: `The app you asked Krate to make is ready${asked ? `:\n\n  "${asked}"` : "."}

Open Krate Studio to try it, change it by asking, and send it as a link that opens on any phone or computer:

${SITE}/app/${SIGN_OFF}`,
  };
}

/// "Someone opened your app": the first time a person other than its maker
/// opened it from a link.
export function firstOpenMail({ name, url }) {
  return {
    subject: `Someone just opened ${name}`,
    text: `Someone opened ${name} from your link for the first time, in their browser.

Send it to more people the same way:

${url}

Each one sees what it asks for before it runs, and opens it with nothing to install.${SIGN_OFF}`,
  };
}

/// Tell the publisher their app is live. Never throws.
export async function tellPublished(env, owner, { name, url, listed }) {
  try {
    const account = await ownerAccount(env, owner);
    if (!account || !account.to) return "no address";
    return await sendNews(env, account, publishedMail({ name, url, listed }), "the published mail");
  } catch (e) {
    return String(e);
  }
}

/// Tell a person the app they asked for is made. Never throws.
export async function tellMade(env, user, record) {
  try {
    const to = accountAddress(user);
    if (!to) return "no address";
    if (!env.RESEND_API_KEY) return "mail is not set up on this hub";
    if (!user.id) return "no account to unsubscribe";
    return await sendNews(env, { id: user.id, user, to }, madeMail({ request: record && record.request }), "the made mail");
  } catch (e) {
    return String(e);
  }
}

/// Tell a publisher the first time someone else opens their app: one KV
/// read on every such open, one write the first time. Never throws.
export async function tellFirstOpen(env, hash, record, base) {
  try {
    // No mail set up: no KV read or write spent on a mail that cannot go.
    if (!env.RESEND_API_KEY) return "mail is not set up on this hub";
    // Once per app, not per version: every update of an app is a new hash,
    // and "someone opened it" for v2, v3 ... is the same news again.
    const flag = `told-opened:${record.line || hash}`;
    if (await env.APPS.get(flag)) return "already told";
    const account = await ownerAccount(env, record.owner || (record.author_login ? `github:${String(record.author_login).toLowerCase()}` : ""));
    if (!account || !account.to) return "no address";
    // Marked first: a second open while this mail is in flight is told nothing.
    await env.APPS.put(flag, String(Math.floor(Date.now() / 1000)));
    return await sendNews(env, account, firstOpenMail({ name: record.name || "your app", url: `${base}/a/${hash}` }), "the first-open mail");
  } catch (e) {
    return String(e);
  }
}
