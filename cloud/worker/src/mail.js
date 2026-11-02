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

import { addressFor } from "./org-mail.js";

export const KRATE_FROM = "Krate <hello@krate.tech>";
export const REPLY_TO = "incyashraj@gmail.com";
const SITE = "https://krate.tech";
const SIGN_OFF = "\n\nKrate\nkrate.tech\n\nReply to this mail and it reaches the person who builds Krate.";

/// One mail through Resend; null when it went, else why not.
export async function sendMail(env, { to, subject, text }) {
  if (!env.RESEND_API_KEY) return "mail is not set up on this hub";
  if (!to) return "no address";
  try {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: { authorization: `Bearer ${env.RESEND_API_KEY}`, "content-type": "application/json" },
      body: JSON.stringify({ from: KRATE_FROM, to: [to], reply_to: REPLY_TO, subject, text }),
    });
    if (res.ok) return null;
    return `resend ${res.status} ${await res.text().catch(() => "")}`;
  } catch (e) {
    return `could not reach resend: ${e}`;
  }
}

/// Where the owner of a listing can be mailed: an account's own address
/// (a proved inbox first), or a GitHub login's account. Null when unknown.
export async function addressOfOwner(env, owner) {
  const who = String(owner || "");
  try {
    if (who.startsWith("account:")) {
      const user = JSON.parse((await env.APPS.get(`user:${who.slice("account:".length)}`)) || "null");
      if (!user) return null;
      if (Array.isArray(user.verified_emails) && user.verified_emails.length) return user.verified_emails[0];
      return user.email || null;
    }
    if (who.startsWith("github:")) return await addressFor(env, who);
  } catch (_) {}
  return null;
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
    const to = await addressOfOwner(env, owner);
    if (!to) return "no address";
    const why = await sendMail(env, { to, ...publishedMail({ name, url, listed }) });
    if (why) console.error(`the published mail did not go: ${why}`);
    return why;
  } catch (e) {
    return String(e);
  }
}

/// Tell a person the app they asked for is made. Never throws.
export async function tellMade(env, user, record) {
  try {
    const to = (Array.isArray(user && user.verified_emails) && user.verified_emails[0]) || (user && user.email);
    if (!to) return "no address";
    const why = await sendMail(env, { to, ...madeMail({ request: record && record.request }) });
    if (why) console.error(`the made mail did not go: ${why}`);
    return why;
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
    const flag = `told-opened:${hash}`;
    if (await env.APPS.get(flag)) return "already told";
    const to = await addressOfOwner(env, record.owner || (record.author_login ? `github:${String(record.author_login).toLowerCase()}` : ""));
    if (!to) return "no address";
    // Marked first: a second open while this mail is in flight is told nothing.
    await env.APPS.put(flag, String(Math.floor(Date.now() / 1000)));
    const why = await sendMail(env, { to, ...firstOpenMail({ name: record.name || "your app", url: `${base}/a/${hash}` }) });
    if (why) console.error(`the first-open mail did not go: ${why}`);
    return why;
  } catch (e) {
    return String(e);
  }
}
