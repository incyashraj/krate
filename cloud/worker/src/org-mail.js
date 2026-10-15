// Organisation mail (IC-948, Addendum 3 A and D): invitations by email,
// and the notice to owners and admins when Krate support is asked to
// restore an owner.
//
// Sent through the hub's existing Gmail path -- From incyashraj@gmail.com,
// Cc yashraj@krate.tech -- because krate.tech mail lands in spam. Needs
// GMAIL_APP_PASSWORD; without it nothing is mailed and the answer says so.
// A failed send never fails what it is about: the invitation stands, with
// `emailed: false` and the reason in plain words.
//
// At most 20 invitation mails a day per organisation (counted in its store)
// and 100 a day for the whole hub (counted in KV, one write per mail).

export const MAIL_FROM_ADDRESS = "incyashraj@gmail.com";
export const MAIL_CC = "yashraj@krate.tech";
export const ORG_DAILY_LIMIT = 20;
export const HUB_DAILY_LIMIT = 100;

/// Send one plain-text mail through Gmail; null when it went, else why.
/// Copied to krate.tech unless `cc` is null (a notification is the
/// person's own, not the founder's to read).
export async function gmail(env, smtpSend, { to, subject, text, cc = MAIL_CC }) {
  if (!env.GMAIL_APP_PASSWORD) return "invitation mail is not set up on this hub";
  const connect = env.SMTP_CONNECT || (await import("cloudflare:sockets")).connect;
  const problem = await smtpSend(connect, {
    user: MAIL_FROM_ADDRESS,
    password: env.GMAIL_APP_PASSWORD,
    from: `Yashraj Pardeshi <${MAIL_FROM_ADDRESS}>`,
    to,
    cc: cc || undefined,
    subject,
    text,
  });
  return problem ? "the mail could not be sent -- copy the link and send it yourself" : null;
}

const when = (seconds) =>
  new Date(seconds * 1000).toLocaleString("en-GB", { timeZone: "UTC", dateStyle: "long", timeStyle: "short" }) + " UTC";

/// What each role can do, in the words the dashboard's invite uses ("can
/// use apps", "can build apps", "can approve apps"), so the mail and the
/// page name a role the same way.
export const ROLE_WORDS = {
  recipient: "use its apps",
  developer: "build apps and send them",
  reviewer: "approve apps before they go out",
  admin: "invite people and look after the team",
  owner: "run everything, with the other owners",
};

/// The invitation mail: who invited them, to which organisation, what they
/// can do there, the one link to press, the expiry, and that it works only
/// for this inbox. No code to copy: the link carries it.
export function inviteMailText({ inviter, orgName, slug, role, joinUrl, expiresAt, email }) {
  // The site the join link is on carries the download too.
  let site = "https://krate.tech";
  try {
    site = new URL(joinUrl).origin;
  } catch (_) {}
  return [
    `${inviter} invited you to ${orgName} on Krate. There you can ${ROLE_WORDS[role] || ROLE_WORDS.recipient}.`,
    "",
    `Join ${orgName}: ${joinUrl}`,
    "",
    `It takes one click: sign in with this address and press Join. Then get Krate to open ${orgName}'s apps (${site}/download/). Every one is signed by ${orgName} and checked before it opens.`,
    "",
    `The invitation works once, only for ${email}, until ${when(expiresAt)}.`,
    `If you were not expecting it, ignore this mail. (${orgName} is "${slug}" on Krate.)`,
  ].join("\n");
}

/// Count one mail against the hub's day; false once 100 have gone.
async function hubSlot(env) {
  const key = `org-invite-mail:${new Date().toISOString().slice(0, 10)}`;
  try {
    const used = Number((await env.APPS.get(key)) || 0);
    if (used >= HUB_DAILY_LIMIT) return false;
    await env.APPS.put(key, String(used + 1), { expirationTtl: 2 * 24 * 60 * 60 });
    return true;
  } catch (_) {
    // A counter must not break what it counts; the store's own per-org
    // limit still holds.
    return true;
  }
}

/// Mail an invitation when it can be mailed. Answers `{emailed, email_problem?}`.
/// `orgSlot()` asks the organisation's store for one of its 20 a day.
export async function mailInvite(env, smtpSend, invite, { orgName, slug, inviter, orgSlot }) {
  if (invite.kind !== "email") return { emailed: false, email_problem: "GitHub invitations are not mailed -- copy the link and send it" };
  if (!env.GMAIL_APP_PASSWORD) return { emailed: false, email_problem: "Invitation mail is not set up on this hub -- copy the link and send it" };
  if (!(await orgSlot())) {
    return { emailed: false, email_problem: `This organisation has sent its ${ORG_DAILY_LIMIT} invitation mails for today -- copy the link and send it` };
  }
  if (!(await hubSlot(env))) {
    return { emailed: false, email_problem: "Krate has sent all its invitation mails for today -- copy the link and send it" };
  }
  const problem = await gmail(env, smtpSend, {
    to: invite.display,
    subject: `${inviter} invited you to ${orgName} on Krate`,
    text: inviteMailText({
      inviter,
      orgName,
      slug,
      role: invite.role,
      joinUrl: invite.join_url,
      expiresAt: invite.expires_at,
      email: invite.display,
    }),
  });
  return problem ? { emailed: false, email_problem: problem } : { emailed: true };
}

/// Where a principal can be mailed, or null. An email principal is its own
/// proved inbox. A GitHub principal is mailed at the address of the account
/// its GitHub sign-in belongs to (the `ghlogin:` record a GitHub sign-in
/// writes): an inbox that account proved by email link or verified Google,
/// or else the email GitHub itself showed on the profile, which GitHub only
/// lets a person choose from their verified addresses.
export async function addressFor(env, principal) {
  if (principal.startsWith("email:")) return principal.slice("email:".length);
  if (!principal.startsWith("github:")) return null;
  try {
    const userId = await env.APPS.get(`ghlogin:${principal.slice("github:".length)}`);
    if (!userId) return null;
    const user = JSON.parse((await env.APPS.get(`user:${userId}`)) || "null");
    if (!user) return null;
    if (Array.isArray(user.verified_emails) && user.verified_emails.length) return user.verified_emails[0];
    if ((user.providers || [])[0] === "github" && user.email) return user.email;
  } catch (_) {}
  return null;
}

/// Tell every current owner and admin Krate can reach that Krate support
/// has been asked to make someone an owner, and how to object. Only those
/// with no address at all are listed back as not mailed; the dashboard's
/// banner and the audit say it to everyone either way.
export async function mailRecovery(env, smtpSend, { slug, orgName, recovery, notify, site }) {
  const mailed = [];
  const notMailed = [];
  for (const principal of notify) {
    const to = env.GMAIL_APP_PASSWORD ? await addressFor(env, principal) : null;
    if (!to) {
      notMailed.push(principal);
      continue;
    }
    const problem = await gmail(env, smtpSend, {
      to,
      subject: `Krate support was asked to add an owner to ${orgName}`,
      text: [
        `Krate support has been asked to make ${recovery.for} an owner of ${orgName} (${slug}).`,
        "",
        `Reason given: ${recovery.reason}`,
        "",
        `It takes effect on ${when(recovery.effective_at)} unless an owner or admin objects before then.`,
        `To object, open ${site}/org/?o=${encodeURIComponent(slug)} and choose Object, or reply to this mail.`,
      ].join("\n"),
    });
    (problem ? notMailed : mailed).push(principal);
  }
  return { mailed, not_mailed: notMailed };
}
