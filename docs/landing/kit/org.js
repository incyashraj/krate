/* Organisations on krate.tech (IC-947): what /orgs/, /org/, /join/ and the
 * private side of /open/ share.
 *
 * The hub decides everything. These pages ask it, show what it said, and
 * hide only the buttons the role matrix says a role can never use, so a
 * person is not offered a click that is sure to be refused. A refusal that
 * still comes back is shown in words, with the next step, never as a code.
 *
 * Plain functions on purpose: docs/landing/studio/org-pages-test.mjs runs
 * this file as it ships.
 */
(function (root) {
  "use strict";

  var LIVE_HUB = "https://hub.krate.tech";

  /// The hub these pages talk to. Always hub.krate.tech, except on a copy
  /// of the site served from this machine, which may point at a hub on this
  /// machine (`wrangler dev`) through localStorage "krate-dev-hub". Both
  /// sides must be loopback, so krate.tech can never be pointed anywhere.
  function hub(loc, store) {
    loc = loc || root.location;
    store = store || root.localStorage;
    var LOOP = /^(127\.0\.0\.1|localhost|\[::1\])$/;
    if (loc && LOOP.test(loc.hostname || "")) {
      var dev = "";
      try { dev = store.getItem("krate-dev-hub") || ""; } catch (e) {}
      if (/^http:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d{2,5}$/.test(dev)) return dev;
    }
    return LIVE_HUB;
  }

  /// This site's session: what /login/done keeps.
  function token(store) {
    store = store || root.localStorage;
    try { return store.getItem("krate_tok") || ""; } catch (e) { return ""; }
  }

  function forget(store) {
    store = store || root.localStorage;
    try { store.removeItem("krate_tok"); } catch (e) {}
  }

  /// A fresh Idempotency-Key. One per click: a retry of the same click
  /// sends the same key, so a doubled request does the thing once.
  function key() {
    var b = new Uint8Array(16);
    root.crypto.getRandomValues(b);
    var s = "";
    for (var i = 0; i < b.length; i++) s += (b[i] + 256).toString(16).slice(1);
    return s;
  }

  /// One call to the hub. Never throws: answers {ok, status, body}, where a
  /// refusal's body is the contract's {error, message} and a network
  /// failure is {error: "offline"}.
  function api(method, path, body, opts) {
    opts = opts || {};
    var headers = {};
    var tok = opts.token !== undefined ? opts.token : token();
    if (tok) headers.authorization = "Bearer " + tok;
    if (body !== undefined && body !== null) headers["content-type"] = "application/json";
    if (opts.key) headers["idempotency-key"] = opts.key;
    if (opts.accept) headers.accept = opts.accept;
    var f = opts.fetch || root.fetch;
    return f(hub() + path, {
      method: method,
      headers: headers,
      body: body !== undefined && body !== null ? JSON.stringify(body) : undefined,
    }).then(function (r) {
      if (r.status === 204) return { ok: true, status: 204, body: null };
      return r.text().then(function (t) {
        var parsed = null;
        try { parsed = t ? JSON.parse(t) : null; } catch (e) { parsed = null; }
        if (!r.ok && (!parsed || typeof parsed !== "object" || !parsed.error)) {
          parsed = { error: r.status === 401 ? "not-signed-in" : "hub-error", message: "" };
        }
        return { ok: r.ok, status: r.status, body: parsed };
      });
    }, function () {
      return { ok: false, status: 0, body: { error: "offline", message: "" } };
    });
  }

  // ------------------------------------------------------------ people

  /// A principal as a person reads it. Takes the hub's `github:alice` or
  /// `email:x@corp.com`, or an object carrying {principal, display, kind}.
  function who(p) {
    if (p && typeof p === "object") {
      var base = who(p.principal || "");
      if (p.display) base.display = String(p.display);
      if (p.kind) base.kind = String(p.kind);
      base.label = base.kind === "github" ? "@" + base.display : base.display;
      return base;
    }
    var s = String(p || "");
    // Who acts for Krate itself in the audit (Addendum 3 D, E).
    if (s === "system:krate") return { principal: s, kind: "krate", display: "Krate", label: "Krate" };
    if (s === "stripe") return { principal: s, kind: "stripe", display: "Stripe", label: "Stripe" };
    // The company's directory, acting through SCIM (IC-955).
    if (s === "scim") return { principal: s, kind: "scim", display: "Your directory", label: "Your directory (SCIM)" };
    var sup = s.match(/^krate-support:(.*)$/);
    if (sup) return { principal: s, kind: "krate-support", display: sup[1], label: "Krate support" + (sup[1] ? " (" + sup[1].replace(/^github:/, "@") + ")" : "") };
    var m = s.match(/^(github|email):(.+)$/);
    if (m) return { principal: s, kind: m[1], display: m[2], label: m[1] === "github" ? "@" + m[2] : m[2] };
    return { principal: s, kind: "", display: s || "someone", label: s || "someone" };
  }

  // ------------------------------------------------------------- roles

  var ROLES = ["owner", "admin", "developer", "reviewer", "recipient"];
  var ROLE_NAME = { owner: "Owner", admin: "Admin", developer: "Developer", reviewer: "Reviewer", recipient: "Member" };
  /// What each role is for, in one line (the same words as docs/book's
  /// organisations page).
  var ROLE_SAYS = {
    owner: "Everything, including the root key and other owners.",
    admin: "Invites and removes people, reads the activity.",
    developer: "Sends apps for approval.",
    reviewer: "Approves apps before they go out.",
    recipient: "Opens the organisation's apps, nothing else.",
  };
  /// The roles as a person picks one when inviting (IC-958 B3): what the
  /// person will be able to do. The invitation mail says the same
  /// (cloud/worker/src/org-mail.js ROLE_WORDS).
  var ROLE_PLAIN = {
    recipient: "Can use apps",
    developer: "Can build apps",
    reviewer: "Can approve apps",
    admin: "Can manage people",
    owner: "Can do everything",
  };

  /// The contract's role matrix (CONTRACT.md "Roles"), the same table the
  /// hub enforces. Used only to decide which controls to show.
  var MATRIX = {
    "org.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "own" },
    "invite.owner-admin": { owner: "yes" },
    "invite.other": { owner: "yes", admin: "yes" },
    "role.change": { owner: "yes", admin: "yes" },
    "role.owner": { owner: "yes" },
    "member.remove": { owner: "yes", admin: "not-owners", developer: "self", reviewer: "self", recipient: "self" },
    "root-key.set": { owner: "yes" },
    "candidate.publish": { owner: "yes", developer: "yes" },
    "candidate.approve": { owner: "yes", reviewer: "yes" },
    "release.download": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
    "audit.read": { owner: "yes", admin: "yes" },
    // Addendum 3: the organisation's lifecycle, billing, SSO and offline
    // access are an owner's; admins read billing.
    "org.close": { owner: "yes" },
    "org.export": { owner: "yes" },
    "billing.read": { owner: "yes", admin: "yes" },
    "billing.checkout": { owner: "yes" },
    "sso.set": { owner: "yes" },
    "offline.set": { owner: "yes" },
    // Release control and groups (IC-949, FLEET-CONTRACT.md R).
    "release.withdraw": { owner: "yes", admin: "yes", reviewer: "yes" },
    "release.set-current": { owner: "yes", admin: "yes" },
    "group.manage": { owner: "yes", admin: "yes" },
    "release.audience": { owner: "yes", admin: "yes" },
    // Company connections (IC-952).
    "connection.manage": { owner: "yes", admin: "yes" },
    "connection.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes" },
    "connection.use": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
    // IC-955: provisioning, audit export and SAML are an owner's to set up;
    // admins watch the export's delivery, as they read the activity.
    "scim.manage": { owner: "yes" },
    "audit-export.manage": { owner: "yes" },
    "audit-export.view": { owner: "yes", admin: "yes" },
    "saml.set": { owner: "yes" },
    // IC-954: the inventory is read by the people who run and review the
    // organisation's apps; who opened each, an app's owner and the usage
    // counts are owners' and admins'.
    "inventory.view": { owner: "yes", admin: "yes", reviewer: "yes" },
    "inventory.people": { owner: "yes", admin: "yes" },
    "app.owner": { owner: "yes", admin: "yes" },
    "usage.view": { owner: "yes", admin: "yes" },
    // IC-959: Insights, a section per role; names of people, and counts of
    // 1 or 2 people, only for owners and admins.
    "insights.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
    "insights.org": { owner: "yes", admin: "yes" },
    "insights.developer": { owner: "yes", developer: "yes" },
    "insights.reviewer": { owner: "yes", reviewer: "yes" },
    "insights.people": { owner: "yes", admin: "yes" },
    // F-319: the organisation's time zone (org-time.js).
    "org.time-zone": { owner: "yes", admin: "yes" },
    "org.logo": { owner: "yes", admin: "yes" },
    // App requests (org-requests.js): anyone asks; owners, admins and
    // developers see and answer them all; everyone else sees their own.
    "request.create": { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" },
    "request.view": { owner: "yes", admin: "yes", developer: "yes", reviewer: "own", recipient: "own" },
    "request.manage": { owner: "yes", admin: "yes", developer: "yes" },
    // Approval rules and the code behind an approval (org-approvals.js):
    // the people who approve, and a version's own sender, read its code;
    // an owner sets the rules.
    "candidate.changes": { owner: "yes", reviewer: "yes", developer: "own" },
    "approval.policy": { owner: "yes" },
  };

  function can(role, action) {
    var row = MATRIX[action];
    return (row && row[role]) || null;
  }

  /// The tabs a role sees on /org/, in order.
  /// Each role's console: the sections it came for, in its order (the
  /// org console by role, approved 2026-10-16). Owners and admins run the
  /// organisation; developers make its apps; reviewers clear the queue;
  /// members open what was released.
  function tabsFor(role) {
    var out;
    if (role === "owner" || role === "admin") {
      out = ["home", "apps", "waiting", "requests", "releases", "people", "computers", "security", "connections", "insights", "activity", "billing", "settings"];
    } else if (role === "developer") {
      out = ["home", "myapps", "build", "requests", "releases", "connections", "insights", "people", "settings"];
    } else if (role === "reviewer") {
      out = ["home", "waiting", "apps", "releases", "insights", "people", "settings"];
    } else {
      out = ["home", "requests", "help"];
    }
    return out.filter(function (id) {
      if (id === "billing") return can(role, "billing.read");
      if (id === "connections") return can(role, "connection.view");
      if (id === "apps") return can(role, "inventory.view");
      if (id === "insights") return can(role, "insights.view");
      if (id === "activity") return can(role, "audit.read");
      return true;
    });
  }

  /// The roles `role` may invite, in order.
  function invitable(role) {
    return ROLES.filter(function (r) {
      return can(role, r === "owner" || r === "admin" ? "invite.owner-admin" : "invite.other") === "yes";
    });
  }

  function high(r) { return r === "owner" || r === "admin"; }

  /// What `me` may do to `them` on the People tab: the roles they may be
  /// moved to (empty: no role control) and whether Remove is offered, with
  /// the reason in words when it is shown but cannot be used.
  function memberActions(me, them, owners) {
    var self = me.principal === them.principal;
    var roles = ROLES.filter(function (r) {
      if (r === them.role) return true;
      var action = them.role === "owner" || r === "owner" ? "role.owner" : high(them.role) || high(r) ? "invite.owner-admin" : "role.change";
      return can(me.role, action) === "yes";
    });
    var changeable = roles.length > 1;
    var lastOwner = them.role === "owner" && owners <= 1;
    var rule = can(me.role, "member.remove");
    var removable = rule === "yes" || (rule === "self" && self) || (rule === "not-owners" && (them.role !== "owner" || self));
    var out = {
      // The only owner is not offered another role: the hub would refuse
      // it (last-owner), and a control that can only fail is noise.
      roles: changeable && !lastOwner ? roles : [],
      roleWhy: changeable && lastOwner ? "The only owner stays an owner until someone else is one." : "",
      remove: removable,
      removeLabel: self ? "Leave" : "Remove",
      removeWhy: "",
    };
    if (removable && lastOwner) {
      out.removeWhy = self
        ? "You are the only owner. Make someone else an owner first, then you can leave."
        : "The only owner cannot be removed. Make someone else an owner first.";
    }
    return out;
  }

  // ----------------------------------------------------------- refusals

  /// A refusal in words, with the next step. `ctx` may carry org (slug or
  /// name), here (this page's path, for sign-in to come back to), what
  /// (the thing that was tried, as "invite people"), and invite details.
  function refusal(reason, ctx) {
    ctx = ctx || {};
    var org = ctx.org || "this organisation";
    var signIn = { label: "Sign in", href: signInHref(ctx.here || "/orgs/") };
    var mine = { label: "Your organisations", href: "/orgs/" };
    switch (reason) {
      case "not-signed-in":
        return { title: "Sign in to continue", body: "Your sign-in has ended or was never here. Sign in and you come straight back to this page.", action: signIn };
      case "not-a-member":
        return { title: "You are not in " + org, body: "Only its members can open this. Ask one of its owners to invite you.", action: mine };
      case "removed":
        return { title: "You were removed from " + org, body: "Its apps no longer open for you. Your own apps and your account are untouched. If this is a mistake, ask one of its owners.", action: mine };
      case "role-cannot":
        return { title: "Your role cannot " + (ctx.what || "do that"), body: "In " + org + " that is for " + (ctx.whoCan || "an owner") + ". Nothing changed.", action: null };
      case "last-owner":
        return { title: org + " would have no owner", body: "Nothing changed. Make someone else an owner first, then try again.", action: null };
      case "self-approval":
        return { title: "Someone else approves this one", body: "You sent it, so a second person has to look. Ask a reviewer or another owner in " + org + ".", action: null };
      case "digest-mismatch":
        return { title: "That version is not waiting any more", body: "It was approved, replaced or never sent here. The list below is fresh.", action: null };
      case "not-signed-for-this-org":
        return { title: "This app is not signed for " + org, body: "It needs the organisation's root key, or a release key that root delegated and has not withdrawn. Sign it again and send it again.", action: null };
      case "invite-expired":
        return { title: "This invitation has expired", body: "Invitations last 7 days. Ask " + (ctx.inviter || "the person who invited you") + " for a new one.", action: null };
      case "invite-used":
        return { title: "This invitation was already used", body: "Each one works once. If you accepted it, " + org + " is in your organisations.", action: mine };
      case "invite-revoked":
        return { title: "This invitation was withdrawn", body: "Ask " + (ctx.inviter || "the person who invited you") + " for a new one.", action: null };
      case "invite-for-someone-else":
        return {
          title: "This invitation is for someone else",
          body: (ctx.me ? "You are signed in as " + ctx.me + ". " : "") + "It works only for the account it names. Sign in with that account, or ask for an invitation to yours.",
          action: { label: "Sign in with another account", href: signInHref(ctx.here || "/orgs/") },
        };
      case "slug-taken":
        return { title: (ctx.slug || "That name") + " is taken", body: "Pick another short name. It is part of the organisation's links.", action: null };
      case "slug-invalid":
        return { title: "That short name will not work", body: "Use 2 to 39 lower-case letters, digits and dashes, starting with a letter. For example acme or acme-labs.", action: null };
      case "idempotency-key-reused":
        return { title: "That click was already used", body: "Nothing changed. Press the button again.", action: null };
      case "no-such-org":
        return { title: "There is no organisation called " + (ctx.slug || "that"), body: "Check the link you were sent.", action: mine };
      case "offline":
        return { title: "Krate could not be reached", body: "Check your connection and try again. Nothing changed.", action: null };
      case "org-closing":
        return { title: org + " is closing", body: "Its apps still open until it closes, but nothing new can be sent or approved. An owner can cancel the closing in Settings.", action: null };
      case "org-closed":
        return { title: org + " is closed", body: "Its apps no longer open. Former owners can still download its export from Settings until the export window ends.", action: mine };
      case "org-suspended":
        return { title: org + " is paused for billing", body: "Apps already released still open. Sending new apps, extra seats and company sign-in come back when an owner settles billing.", action: null };
      case "close-statement-mismatch":
        return { title: "The closing changed while you were looking", body: "Nothing was approved. Look at the closing again and approve what it says now.", action: null };
      case "close-confirm-mismatch":
        return { title: "That is not the short name", body: "Nothing changed. Type " + (ctx.slug || "the organisation's short name") + " exactly to close it.", action: null };
      case "export-not-ready":
        return { title: "The export is still being built", body: "It is usually ready in a minute. This list refreshes by itself.", action: null };
      case "billing-not-open":
        return { title: "Card billing is not open yet", body: "Krate invoices pilot organisations directly: write to yashraj@krate.tech.", action: { label: "Write to yashraj@krate.tech", href: "mailto:yashraj@krate.tech" } };
      case "sso-required":
        return {
          title: org + " requires company sign-in",
          body: "Your address is at " + org + "'s company domain, so it asks you to sign in through your company. You come straight back here.",
          action: { label: "Use company sign-in", href: "/login/?org=" + encodeURIComponent(ctx.slug || "") + "&next=" + encodeURIComponent(ctx.here || "/orgs/") },
        };
      case "rate-limited":
        return { title: "Too many refused attempts", body: "Wait a minute, then try again. Nothing changed.", action: null };
      case "domain-not-verified":
        return { title: "The domain is not verified yet", body: "The TXT record is not there yet. Add it at your DNS provider as shown above, wait a few minutes, then press Verify again.", action: null };
      case "sso-not-set-up":
        return { title: "Company sign-in is not set up", body: "An owner of " + org + " sets it up in Settings. Until then, sign in with GitHub, Google or email.", action: null };
      case "sso-unverified-email":
        return { title: "Your company did not confirm your email", body: "Company sign-in needs an address your company has verified. Ask your IT team, or sign in another way.", action: null };
      case "sso-wrong-domain":
        return { title: "That address is not at " + org + "'s domain", body: "Company sign-in takes addresses at the domain " + org + " verified. Sign in with your work account, or another way.", action: null };
      case "sso-needs-break-glass":
        return { title: "Keep a way in that does not need company sign-in", body: "Before " + org + " can require company sign-in, at least one owner must sign in another way, with GitHub or an address outside the company domain. Invite a second owner like that, then turn it on.", action: null };
      case "sso-off":
        return { title: "Company sign-in is not switched on on this hub yet", body: "Nothing changed. Sign in with GitHub, Google or email for now.", action: null };
      case "leases-off":
        return { title: "Offline access is not switched on on this hub yet", body: "Apps still open while you are online. Nothing changed.", action: null };
      case "recovery-not-waiting":
        return { title: "That recovery is not waiting any more", body: "It was already cancelled or has taken effect. The banner is gone once the page refreshes.", action: null };
      case "sso-failed":
        return { title: "Company sign-in did not finish", body: "Nothing changed. Try again, or sign in another way.", action: null };
      case "release-withdrawn": {
        var wd = ctx.withdrawn || {};
        return {
          title: "This version was withdrawn",
          body: org + " withdrew it" + (wd.at ? " on " + day(wd.at) : "") + (wd.reason ? ": “" + wd.reason + "”" : "") + ". It no longer opens, here or on any computer that connects. Ask " + org + " which version to use now.",
          action: null,
        };
      }
      case "not-in-audience":
        return {
          title: "This app is only for " + (Array.isArray(ctx.groups) && ctx.groups.length ? groupsSaid(ctx.groups) : "some groups") + " in " + org,
          body: "You are in " + org + ", but this app was sent only to certain groups, and you are not in them. Ask an admin of " + org + " to add you.",
          action: null,
        };
      case "reason-required":
        return { title: "Say why it is withdrawn", body: "The people who had it are shown the reason. Write a few words, up to 300 characters.", action: null };
      case "no-such-group":
        return { title: "That group does not exist", body: "Nothing changed. Make the group in People first.", action: null };
      case "group-taken":
        return { title: "There is already a group by that name", body: "Pick another name, or add people to the one there is.", action: null };
      case "audit-export-off":
        return { title: "The audit export is not switched on on this hub yet", body: "Nothing changed. The activity is still recorded here, and its chain is checked every time you read it.", action: null };
      // An app sent from the page (IC-958): the file, not the person.
      case "invalid-bundle":
        return { title: "That file is not a Krate app", body: "Choose a .krate file: what Krate Studio saves, or krate pack makes. Nothing was sent.", action: null };
      case "too-large":
        return { title: "That app is over 5 MB", body: "Send it from Krate Studio, which sends big apps in parts. Nothing was sent.", action: { label: "Get Krate Studio", href: "/download/" } };
      case "not-marked-for-this-org":
        return { title: "This app was made for another organisation", body: "It is marked as another organisation's private app, so it cannot be released in " + org + ". Choose a ready app, or send yours from Krate Studio, which marks it for " + org + ". Nothing was released.", action: null };
      case "bytes-held-elsewhere":
        return { title: "That exact file is already public", body: "It was published for everyone, so it cannot be " + org + "'s private app. Make a new version and send that.", action: null };
      case "bad-request":
        return { title: "That is not quite right", body: (ctx.message ? String(ctx.message).replace(/ -- /g, ". ").replace(/\.?$/, ".") + " " : "") + "Nothing changed.", action: null };
      case "group-invalid":
        return { title: "That group name will not work", body: "Use lower-case letters, digits and dashes, starting with a letter. For example finance or field-ops.", action: null };
      // Company connections (IC-952).
      case "no-such-connection":
        return { title: "That connection is not there any more", body: "Someone may have removed it. The list is fresh now.", action: null };
      case "connection-taken":
        return { title: "There is already a connection by that name", body: "Nothing changed. Pick another name, or replace the credential of the one there is.", action: null };
      case "connection-invalid":
        return { title: "That connection is not complete", body: (ctx.message ? String(ctx.message).replace(/\.?$/, ". ") : "") + "Nothing changed.", action: null };
      case "credential-required":
        return { title: "A new address needs its credential again", body: "The stored credential is never sent anywhere new. Remove this connection and add it again with the new address.", action: null };
      case "connections-off":
        return { title: "Connections are not switched on on this hub yet", body: "Nothing changed.", action: null };
      // The organisation keyring (F-316): a credential is sealed to it.
      case "no-org-key":
        return { title: org + " needs one setup step first", body: "Credentials are encrypted so that only " + org + "'s computers can read them, never Krate. An owner turns that on once, on their computer: krate org keys init. Nothing was sent.", action: null };
      case "key-rotated":
        return { title: "Save once more", body: org + "'s encryption was just renewed after someone left. Nothing was saved: save again and it is encrypted the new way.", action: null };
      case "credential-needs-entry":
        return { title: "This credential must be entered again", body: "It was saved before Krate encrypted credentials, so Krate deleted it rather than keep a copy it could read. An owner or admin of " + org + " enters it again with Replace credential.", action: null };
      case "connection-not-in-audience":
      case "app-not-allowed":
      case "connection-not-requested":
        return { title: "This app cannot use that connection", body: (ctx.message ? String(ctx.message).split(" -- ")[0] + ". " : "") + "Ask an admin of " + org + ".", action: null };
      // Approval rules (org-approvals.js): the hub's sentence says whose
      // approval is needed; this says it as the next step.
      case "approval-policy":
        return { title: "Someone else's approval is needed", body: (ctx.message ? String(ctx.message).split(" -- ")[0].replace(/\.?$/, ". ") : "") + "Nothing was approved.", action: null };
      case "already-approved":
        return { title: "You approved this already", body: (ctx.message ? "It " + String(ctx.message).split(" -- it ")[1].replace(/\.?$/, ". ") : "") + "It is released once the rest are in.", action: null };
      case "policy-invalid":
        return { title: "Those rules will not work", body: (ctx.message ? String(ctx.message).replace(/ -- /g, ". ").replace(/\.?$/, ". ") : "") + "Nothing changed.", action: null };
      default:
        return { title: "That did not work", body: (ctx.message ? String(ctx.message) + ". " : "") + "Nothing changed. Try again in a moment.", action: null };
    }
  }

  /// "the finance group", "the finance and ops groups".
  function groupsSaid(list) {
    var l = (list || []).slice();
    var said = l.length <= 1 ? l.join("") : l.slice(0, -1).join(", ") + " and " + l[l.length - 1];
    return "the " + said + (l.length === 1 ? " group" : " groups");
  }

  // ------------------------------------------------------------ sign-in

  /// The /login link that comes back to `here` once signed in. `via` goes
  /// straight on to that sign-in ("github", "google", "email").
  function signInHref(here, via) {
    return "/login/?next=" + encodeURIComponent(here || "/") + (via ? "#" + via : "");
  }

  // ------------------------------------------------------------- audit

  function targetOf(t) {
    var s = String(t || "");
    var at = s.indexOf(":");
    if (at < 0) return { kind: s, value: "" };
    return { kind: s.slice(0, at), value: s.slice(at + 1) };
  }

  var SHORT_WHY = {
    "not-signed-in": "not signed in",
    "not-a-member": "not a member",
    removed: "removed from the organisation",
    "role-cannot": "their role cannot",
    "last-owner": "it would leave no owner",
    "self-approval": "they sent it themselves",
    "digest-mismatch": "no such version waiting",
    "not-signed-for-this-org": "not signed for the organisation",
    "not-marked-for-this-org": "it was made for another organisation",
    "invite-expired": "the invitation had expired",
    "invite-used": "the invitation was already used",
    "invite-for-someone-else": "the invitation names someone else",
    "invite-revoked": "the invitation was withdrawn",
    "slug-taken": "the name is taken",
    "idempotency-key-reused": "a reused request",
    "org-closing": "the organisation is closing",
    "org-closed": "the organisation is closed",
    "org-suspended": "paused for billing",
    "close-statement-mismatch": "the closing had changed",
    "close-confirm-mismatch": "the short name did not match",
    "sso-required": "company sign-in required",
    "recovery-not-waiting": "no recovery was waiting",
    "release-withdrawn": "it was withdrawn",
    "not-in-audience": "it is not for their groups",
    "reason-required": "no reason was given",
    "no-such-group": "no such group",
    "group-taken": "the group exists already",
    "group-invalid": "not a group name",
    // Company connections (IC-952).
    "no-such-connection": "no such connection",
    "connection-taken": "the connection exists already",
    "connection-invalid": "the connection was not complete",
    "credential-required": "a new address needs the credential again",
    "connection-not-in-audience": "the connection is not for their groups",
    "app-not-allowed": "the connection does not allow that app",
    "connection-not-requested": "that version did not ask for it",
    "credential-needs-entry": "its credential must be entered again",
    // Declining, release notes and app requests.
    declined: "it was declined",
    "not-waiting": "it was not waiting for approval",
    "note-closed": "a declined or withdrawn version keeps its note",
    "no-such-request": "no such request",
    "requests-full": "too many open requests",
    // Approval rules.
    "approval-policy": "the approval rules need someone else",
    "already-approved": "they approved it already",
    "policy-invalid": "the rules were not complete",
  };

  /// One audit event as a sentence. `names` maps a digest to "Ledger 1.2".
  function eventWords(ev, names) {
    names = names || {};
    var a = who(ev.actor).label;
    var t = targetOf(ev.target);
    var app = function () {
      if (t.kind !== "candidate" && t.kind !== "release") return "an app";
      return names[t.value] || "version " + String(t.value).slice(0, 12);
    };
    var invitee = function () {
      var v = t.value, slash = v.lastIndexOf("/");
      return { p: who(slash > 0 ? v.slice(0, slash) : v).label, role: slash > 0 ? v.slice(slash + 1) : "" };
    };
    var roleWord = function (r) { return ROLE_NAME[r] ? ROLE_NAME[r].toLowerCase() : r; };
    if (ev.result === "denied") {
      var tried = {
        "org.create": "create the organisation",
        "org.view": "open the organisation",
        "invite.create": "invite " + invitee().p,
        "invite.revoke": "withdraw an invitation",
        "invite.accept": "accept an invitation",
        "member.role": "change " + who(t.value).label + "'s role",
        "member.remove": "remove " + who(t.value).label,
        "root-key.set": "set the root key",
        "candidate.publish": "send an app for approval",
        "candidate.approve": "approve " + app(),
        "candidate.list": "see what is waiting",
        "release.list": "see the releases",
        "release.download": "open " + app(),
        "audit.read": "read the activity",
        // The store's own command names, which a lifecycle or billing gate
        // refuses before the command runs (Addendum 3 D).
        approve: "approve " + app(),
        "add-candidate": "send an app for approval",
        "check-candidate": "send an app for approval",
        invite: "invite someone",
        accept: "accept an invitation",
        "set-role": "change a role",
        remove: "remove someone",
        "set-root": "set the root key",
        close: "close the organisation",
        "close-approve": "approve the closing",
        "close-cancel": "cancel the closing",
        export: "build an export",
        "billing-checkout": "open checkout",
        "recover-object": "object to a recovery",
        // Release control and groups (IC-949, R).
        "release.withdraw": "withdraw " + app(),
        "release.set-current": "choose which version of " + (t.value || "an app") + " opens",
        "release.audience": "choose who opens " + app(),
        "group.manage": "make the group " + t.value,
        "group.add": "add someone to a group",
        "group.remove": "take someone out of a group",
        "group.list": "see the groups",
        "lease.issue": "get offline access to " + app(),
        // Company connections (IC-952).
        "connection.manage": "change the " + (t.kind === "connection" ? t.value : "") + " connection",
        "connection.view": "see the connections",
        "connection.use": "use the " + (t.kind === "connection" ? t.value : "") + " connection",
        // Inventory and ownership (IC-954).
        "inventory.view": "see the organisation's apps",
        "app.owner": "choose who owns " + (t.value || "an app"),
        "usage.view": "see how the apps are used",
        "org.logo": "change the organisation's logo",
        "org.time-zone": "change the organisation's time zone",
        // Declining, release notes and app requests.
        "candidate.decline": "decline " + app(),
        decline: "decline " + app(),
        "candidate.note": "write the release note on " + app(),
        "note-set": "write a release note",
        "request.create": "ask for an app",
        "request-create": "ask for an app",
        "request.view": "see the app requests",
        "request.manage": "answer an app request",
        "request-update": "answer an app request",
        "notify-prefs-set": "change their notifications",
        "candidate.changes": "read the code of " + app(),
        "approval.policy": "change the approval rules",
        "approval-policy-set": "change the approval rules",
      }[ev.action] || "do " + ev.action;
      // Shown beside a Refused badge, so the sentence says why, not "refused" again.
      return a + " tried to " + tried + ": " + (SHORT_WHY[ev.reason] || ev.reason || "not allowed") + ".";
    }
    switch (ev.action) {
      case "org.create": return a + " created the organisation.";
      case "member.add": {
        if (ev.actor === "scim") {
          var r0 = String(ev.reason || "").split(";")[0];
          if (/inactive/.test(ev.reason || "")) return a + " listed " + who(t.value).label + ", with no access yet.";
          return a + (/reactivated/.test(ev.reason || "") ? " brought back " : " added ") + who(t.value).label + " as " + roleWord(r0) + ".";
        }
        return a + " joined as " + roleWord(ev.reason || "owner") + ".";
      }
      case "invite.create": { var i = invitee(); return a + " invited " + i.p + " as " + roleWord(i.role) + "."; }
      case "invite.revoke": {
        var j = invitee();
        return a + " withdrew the invitation for " + j.p + (ev.reason === "issuer removed" ? ", because the person who sent it was removed." : ".");
      }
      case "invite.accept": { var k = invitee(); return a + " accepted the invitation and joined as " + roleWord(k.role) + "."; }
      case "member.role": {
        var ch = String(ev.reason || "").split("->");
        return a + " changed " + who(t.value).label + " from " + roleWord(ch[0]) + " to " + roleWord(ch[1] || "") + ".";
      }
      case "member.remove": {
        var them = who(t.value).label;
        if (ev.actor === "scim") return a + " removed " + them + (/deleted/.test(ev.reason || "") ? ", deleted in the directory." : ", deactivated in the directory.");
        return them === a ? a + " left the organisation." : a + " removed " + them + ".";
      }
      // IC-955: provisioning, audit export, SAML.
      case "scim.token.create": return a + " made a SCIM token for the company directory.";
      case "scim.token.revoke": return a + " revoked a SCIM token. It stops at its next request.";
      case "scim.default-role": return a + " set people the directory adds to join as " + roleWord(ev.reason || "") + ".";
      case "group.link": return a + " took over the group " + t.value + ": its members now follow the directory.";
      case "group.delete": return a + " deleted the group " + t.value + ".";
      case "audit-export.set": return a + " started sending the activity to " + t.value + ".";
      case "audit-export.change": return a + " changed where the activity is sent: " + t.value + ".";
      case "audit-export.secret": return a + " made a new signing secret for the activity export.";
      case "audit-export.clear": return a + " stopped sending the activity to " + t.value + (/^0 /.test(ev.reason || "") || !ev.reason ? "." : " (" + String(ev.reason).replace(/ not sent$/, "") + " events were not sent).");
      case "audit-export.test": return a + " sent a test event to " + t.value + ".";
      case "saml.set": return a + " set up company sign-in by SAML.";
      case "saml.update": return a + " changed the SAML sign-in.";
      case "saml.clear": return a + " turned off SAML sign-in.";
      case "root-key.register": return a + " set up the organisation's signature.";
      // The keyring and the signing ring (F-316, IC-958): said as what a
      // person did, never as keys.
      case "key.init": return a + " turned on end-to-end encryption for the team's data.";
      case "key.device.add": return a + " added a computer that can open the team's data.";
      case "key.device.remove": return a + " removed a computer from the team's data.";
      case "key.rotate": return a + " renewed the team's encryption, so a removed computer cannot read anything new.";
      case "key.stale": return "The team's encryption is due to be renewed, after someone left.";
      case "key.recovery.read": return a + " read the recovery code.";
      case "key.deleted": return "Older encryption was deleted after it was replaced.";
      case "signing.init": return a + " set up signing, so every release is signed as the organisation.";
      case "signing.device.remove": return a + " removed a computer that could sign releases.";
      case "signing.recovery.read": return a + " read the signing recovery code.";
      case "publisher.request": return a + " asked to send apps.";
      case "publisher.approve": return a + " let " + who(t.value).label + " send apps.";
      case "publisher.close": return who(t.value).label + " can no longer send apps.";
      case "publisher.revoke-due": return who(t.value).label + "'s permission to send apps ended.";
      case "settings.publishers": return a + " chose to " + (ev.reason || "change how new senders are approved") + ".";
      case "settings.two-person": return a + " set it so " + (ev.reason || "the release rule changed") + ".";
      case "review.two-person.on": return "A second person now approves every release.";
      case "settings.logo": return a + (ev.reason === "removed" ? " removed the organisation's logo." : " set the organisation's logo.");
      case "settings.time-zone": return a + " changed the time zone the days are counted in: " + String(ev.reason || "").replace(" -> ", " to ") + ".";
      case "data.daily": return "The team's data was written to" + (ev.reason ? ": " + ev.reason : "") + ".";
      case "data.deleted": return a + " deleted an app's team data.";
      case "root-key.rotate": return a + " replaced the root key with " + fingerprint(t.value) + ".";
      case "release-key.revoke": return a + " withdrew the release key " + fingerprint(t.value) + ".";
      case "candidate.publish": return a + " sent " + app() + " for approval.";
      case "candidate.decline": return a + " declined " + app() + (ev.reason ? ": " + ev.reason : "") + ".";
      // Approval rules: an approval that does not release yet, and the rules changing.
      case "candidate.approve.partial": {
        var still = String(ev.reason || "").split(" -- ")[1];
        return a + " approved " + app() + ". It " + (still || "waits for more approvals") + ".";
      }
      case "settings.approvals": return a + " changed the approval rules: " + (ev.reason || "see Settings") + ".";
      case "candidate.note": return a + (ev.reason === "cleared" ? " cleared the release note on " : " wrote the release note on ") + app() + ".";
      case "request.create": return a + " asked for an app.";
      case "request.update": {
        var rs = String(ev.reason || "").split(",")[0];
        var rw = { open: "open again", planned: "planned", done: "done", declined: "declined" }[rs] || rs;
        return a + " marked an app request " + rw + (/replied/.test(ev.reason || "") ? ", with a reply." : ".");
      }
      // Solo release (IC-958 addendum): the hub marks a release its sender
      // made while nobody else could approve with the reason "solo".
      case "candidate.approve":
        return ev.reason === "solo"
          ? a + " released " + app() + " themselves, with no second approver yet."
          : a + " approved " + app() + ". It is released.";
      case "release.download": return a + " opened " + app() + ".";
      // Addendum 3: closing, export, recovery, billing, SSO, offline.
      case "org.close":
        return /second owner/.test(ev.reason || "") ? a + " asked to close the organisation. It waits for a second owner." : a + " scheduled the organisation to close.";
      case "org.close.approve": return a + " approved closing the organisation. The window has started.";
      case "org.close.cancel": return a + " cancelled the closing. Nothing closes.";
      case "org.closed": return "The organisation closed. Its apps no longer open.";
      case "org.export": return a + " built an export.";
      case "org.files.deleted": {
        var n = String(ev.reason || "").match(/^(\d+)/);
        return "Krate deleted the closed organisation's files after the export window" + (n ? " (" + n[1] + " files)" : "") + ".";
      }
      case "member.recover.request": return a + " asked to make " + who(t.value).label + " an owner, because: " + (ev.reason || "no reason given") + ".";
      case "member.recover": return "Krate support restored ownership to " + who(t.value).label + ", by " + who(ev.actor).display + ", reason: " + (ev.reason || "none given") + ".";
      case "member.recover.object": return a + " objected. The ownership recovery is cancelled.";
      case "billing.customer": return a + " set up billing with Stripe.";
      case "billing.checkout": return a + " opened checkout" + (ev.reason ? " for " + ev.reason : "") + ".";
      case "billing.update": return "Stripe updated billing" + (ev.reason ? ": " + ev.reason : "") + ".";
      case "billing.manual": return a + " marked the organisation as invoiced" + (ev.reason ? ", reference " + ev.reason : "") + ".";
      case "offline.set": {
        var to = String(ev.reason || "").split("->")[1] || "";
        return a + " set " + (t.value || "an app") + " to " + offlineName(to).toLowerCase() + ".";
      }
      case "offline.default": return a + " set new apps to " + offlineName(String(ev.reason || "").split("->")[1] || "").toLowerCase() + ".";
      case "lease.issue": {
        var u = String(ev.reason || "").match(/until (\d{4}-\d{2}-\d{2})/);
        return "Offline access issued to " + a + " for " + app() + (u ? ", until " + day(Date.parse(u[1] + "T12:00:00Z") / 1000) : "") + ".";
      }
      case "sso.set": return a + " set up company sign-in.";
      case "sso.update": return a + " changed company sign-in.";
      case "sso.clear": return a + " turned off company sign-in.";
      case "sso.verify-domain": return a + " verified the domain " + (t.value || "") + ".";
      case "sso.join": return a + " joined through company sign-in as " + roleWord(ev.reason || "") + ".";
      // Release control and groups (IC-949, R).
      case "release.withdraw": return a + " withdrew " + app() + (ev.reason ? ": “" + ev.reason + "”" : "") + ".";
      case "release.current": {
        var cur = String(ev.reason || "").split(" after withdrawing ");
        var ver = function (d) { return names[d] || "version " + String(d).slice(0, 12); };
        var appId = t.value || "the app";
        if (cur[0] === "none") return "Nothing of " + appId + " opens any more, after " + a + " withdrew " + ver(cur[1]) + ".";
        return cur[1]
          ? ver(cur[0]) + " became current, after " + a + " withdrew " + ver(cur[1]) + "."
          : a + " made " + ver(cur[0]) + " the version of " + appId + " that opens.";
      }
      case "release.audience": {
        var aud = String(ev.reason || "");
        return aud === "everyone" ? a + " opened " + app() + " to everyone." : a + " limited " + app() + " to " + groupsSaid(aud.replace(/^groups:/, "").split(",")) + ".";
      }
      case "group.create": return a + " made the group " + t.value + ".";
      case "group.add":
      case "group.remove": {
        var gs = t.value.indexOf("/"), g = gs > 0 ? t.value.slice(0, gs) : t.value, whom = who(gs > 0 ? t.value.slice(gs + 1) : "").label;
        if (ev.action === "group.add") return a + " added " + whom + " to " + g + ".";
        return ev.reason === "member removed" ? whom + " left " + g + ", on being removed from the organisation." : a + " took " + whom + " out of " + g + ".";
      }
      // Company connections (IC-952). The credential is never in an event.
      case "connection.create": return a + " added the " + t.value + " connection, to " + (ev.reason || "its address") + ".";
      case "connection.change": return a + " changed the " + t.value + " connection: " + (ev.reason || "its settings") + ".";
      case "connection.credential": return a + " replaced the credential of the " + t.value + " connection.";
      case "connection.remove": return a + " removed the " + t.value + " connection and its credential.";
      case "connection.reseal": return a + "'s computer encrypted the connections' credentials again, after someone left.";
      case "connection.needs-entry": return "The credential of the " + t.value + " connection must be entered again: Krate deleted the copy it could have read.";
      case "connection.use": {
        var cu = String(ev.reason || "").match(/^(\d+) uses? on (\d{4}-\d{2}-\d{2})$/);
        return cu
          ? "Apps used the " + t.value + " connection " + cu[1] + (cu[1] === "1" ? " time" : " times") + " on " + day(Date.parse(cu[2] + "T12:00:00Z") / 1000) + "."
          : "Apps used the " + t.value + " connection.";
      }
      // Ownership (IC-954): who IT asks about an app.
      case "app.owner": {
        var gone = String(ev.reason || "").match(/^needs an owner: (.+) was removed$/);
        if (gone) return (t.value || "An app") + " needs an owner: " + who(gone[1]).label + ", who owned it, was removed.";
        return a + " made " + who(ev.reason || "").label + " the owner of " + (t.value || "an app") + ".";
      }
      default: return a + " " + ev.action + (ev.target ? " " + ev.target : "") + ".";
    }
  }

  // ------------------------------------------------------------ format

  /// A key's fingerprint as people compare it: its first 16 hex digits in
  /// fours (the audit records the same 16).
  function fingerprint(hex) {
    var h = String(hex || "").toLowerCase().replace(/[^0-9a-f]/g, "").slice(0, 16);
    return h.replace(/(.{4})(?=.)/g, "$1 ");
  }

  var MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  function day(seconds) {
    if (!seconds) return "";
    var d = new Date(seconds * 1000);
    return d.getDate() + " " + MONTHS[d.getMonth()] + " " + d.getFullYear();
  }
  function moment(seconds) {
    if (!seconds) return "";
    var d = new Date(seconds * 1000);
    var hh = String(d.getHours()).padStart(2, "0"), mm = String(d.getMinutes()).padStart(2, "0");
    return day(seconds) + ", " + hh + ":" + mm;
  }

  /// What a release or candidate is called: "Ledger 1.2", else its digest.
  function appName(c) {
    var n = (c && c.name) || "";
    var v = (c && c.version) || "";
    if (!n) return "Version " + String((c && c.digest) || "").slice(0, 12);
    return v ? n + " " + v : n;
  }

  // ------------------------------------------------- offline, SSO, money

  /// Offline classes (Addendum 3 G). Nothing longer than 30 days exists.
  var OFFLINE = [
    ["online", "Online only", "Checks with the organisation every time it opens."],
    ["days-7", "7 days offline", "Opens without a connection for up to 7 days."],
    ["days-30", "30 days offline", "Opens without a connection for up to 30 days."],
  ];
  function offlineName(c) {
    for (var i = 0; i < OFFLINE.length; i++) if (OFFLINE[i][0] === c) return OFFLINE[i][1];
    return "7 days offline";
  }
  /// The exact terms a person reads before the first private open.
  function offlineNotice(off) {
    if (!off || off.class === "online" || !off.until) {
      return "Needs a connection every time it opens. If you are removed, it stops at once.";
    }
    return "Works offline until " + day(off.until) + ", then needs a connection. If you are removed, it stops when it next connects.";
  }

  /// The company sign-in providers the page names, with the issuer each
  /// one uses (Addendum 3 F: each is one issuer URL).
  var PROVIDERS = [
    ["Google Workspace", "https://accounts.google.com"],
    ["Okta", "https://<your-company>.okta.com"],
    ["Microsoft Entra", "https://login.microsoftonline.com/<tenant-id>/v2.0", "and its tenant must issue verified email claims, or sign-in is refused"],
  ];

  /// What a finished company sign-in did (Addendum 3 F): the hub returns
  /// `org` and `sso` in /login/done's fragment. Words for each.
  function ssoNotice(state, org) {
    var o = org || "the organisation";
    return {
      joined: "You signed in with " + o + "'s company sign-in and joined " + o + ".",
      member: "You are signed in through " + o + "'s company sign-in.",
      removed: "You are signed in, but you were removed from " + o + ", so company sign-in did not add you back. Ask one of its owners if that is a mistake.",
      "no-auto-join": "You are signed in. " + o + " does not let people join by themselves: ask one of its owners for an invitation.",
      "off-domain": "You are signed in, but your address is not at " + o + "'s verified domain, so it did not add you.",
      "no-sso": "You are signed in. " + o + " has no company sign-in set up any more.",
    }[state] || "You are signed in.";
  }
  /// A company sign-in's result, kept by /login/done for the page it
  /// returns to, read once.
  function takeSsoNotice(store, name) {
    store = store || root.localStorage;
    var raw = null;
    try { raw = store.getItem("krate_sso_notice"); store.removeItem("krate_sso_notice"); } catch (e) {}
    if (!raw) return "";
    try { var n = JSON.parse(raw); return n && n.state ? ssoNotice(n.state, name || n.org) : ""; } catch (e) { return ""; }
  }

  /// Money only as the hub sent it: Stripe's minor units and currency.
  function money(amount, currency) {
    if (typeof amount !== "number" || !currency) return "";
    var c = String(currency).toUpperCase();
    try { return new Intl.NumberFormat("en", { style: "currency", currency: c }).format(amount / 100); } catch (e) { return (amount / 100).toFixed(2) + " " + c; }
  }

  /// The join link for an invitation answer (`join_url`, Addendum 2).
  function joinUrl(answer) {
    return (answer && answer.join_url) || "";
  }

  // ------------------------------------------------------------- DOM

  /// el("p", {class: "x", text: "hi"}, [child, ...])
  function el(tag, attrs, kids) {
    var n = root.document.createElement(tag);
    attrs = attrs || {};
    for (var k in attrs) {
      if (!Object.prototype.hasOwnProperty.call(attrs, k) || attrs[k] === undefined || attrs[k] === null || attrs[k] === false) continue;
      if (k === "text") n.textContent = attrs[k];
      else if (k === "class") n.className = attrs[k];
      else if (k.slice(0, 2) === "on") n.addEventListener(k.slice(2), attrs[k]);
      else if (k === "hidden") n.hidden = true;
      else n.setAttribute(k, attrs[k] === true ? "" : attrs[k]);
    }
    (kids || []).forEach(function (c) {
      if (c === null || c === undefined || c === false) return;
      n.appendChild(typeof c === "string" ? root.document.createTextNode(c) : c);
    });
    return n;
  }

  /// A refusal as a call-out block, its action as a button.
  function refusalBlock(words, cls) {
    return el("div", { class: "call " + (cls || "warn") + " org-why", role: "status" }, [
      el("div", {}, [
        el("b", { text: words.title }),
        el("p", { text: words.body }),
        words.action ? el("a", { class: "btn ghost sm", href: words.action.href, text: words.action.label }) : null,
      ]),
    ]);
  }

  /// The signed-out face: the three ways in, each coming back to `here`.
  var GH_MARK = '<path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.42 7.42 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/>';
  var G_MARK = '<path fill="#4285F4" d="M17.64 9.2c0-.64-.06-1.25-.16-1.84H9v3.48h4.84a4.14 4.14 0 0 1-1.8 2.72v2.26h2.92c1.7-1.57 2.68-3.88 2.68-6.62z"/><path fill="#34A853" d="M9 18c2.43 0 4.47-.8 5.96-2.18l-2.92-2.26c-.8.54-1.84.86-3.04.86-2.34 0-4.32-1.58-5.03-3.7H.96v2.33A9 9 0 0 0 9 18z"/><path fill="#FBBC05" d="M3.97 10.72a5.4 5.4 0 0 1 0-3.44V4.95H.96a9 9 0 0 0 0 8.1l3.01-2.33z"/><path fill="#EA4335" d="M9 3.58c1.32 0 2.5.45 3.44 1.35l2.58-2.58A9 9 0 0 0 .96 4.95l3.01 2.33C4.68 5.16 6.66 3.58 9 3.58z"/>';
  function mark(paths, box, fill) {
    var svg = root.document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", box);
    svg.setAttribute("width", "17");
    svg.setAttribute("height", "17");
    svg.setAttribute("aria-hidden", "true");
    if (fill) svg.setAttribute("fill", fill);
    svg.innerHTML = paths;
    return svg;
  }
  /// The ways in: GitHub and Google side by side with their marks, then
  /// an email box that sends the sign-in link straight away. The address
  /// travels to /login/ in this tab's session storage, never in the link,
  /// and /login/ sends it at once, keeping the one place that knows how a
  /// sign-in comes back here.
  /// `opts.first`: "email" puts the email box first (an invitation sent to
  /// an inbox); otherwise GitHub and Google lead.
  function signInChoices(here, opts) {
    var emailFirst = !!(opts && opts.first === "email");
    var input = el("input", { class: "inp", type: "email", placeholder: "you@company.com", "aria-label": "Work email", autocomplete: "email", required: "" });
    var go = el("button", { class: "btn " + (emailFirst ? "dark" : "ghost"), type: "submit", text: "Continue" });
    var form = el("form", { class: "si-email" }, [input, go]);
    form.addEventListener("submit", function (e) {
      if (e && e.preventDefault) e.preventDefault();
      var v = String(input.value || "").trim();
      if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(v)) { if (input.focus) input.focus(); return; }
      try { root.sessionStorage.setItem("krate-login-email", v); } catch (x) {}
      root.location.href = signInHref(here, "email");
    });
    var row = el("div", { class: "si-row" }, [
      el("a", { class: "btn " + (emailFirst ? "ghost" : "dark") + " si-btn", href: signInHref(here, "github") }, [mark(GH_MARK, "0 0 16 16", "currentColor"), el("span", { text: "GitHub" })]),
      el("a", { class: "btn ghost si-btn", href: signInHref(here, "google") }, [mark(G_MARK, "0 0 18 18"), el("span", { text: "Google" })]),
    ]);
    var or = el("div", { class: "or" }, [el("span", { text: "or" })]);
    return el("div", { class: "si" }, emailFirst ? [form, or, row] : [row, or, form]);
  }

  /// Copy text, with the button saying it worked.
  function copy(text, button) {
    var done = function () {
      if (!button) return;
      var was = button.textContent;
      button.textContent = "Copied";
      setTimeout(function () { button.textContent = was; }, 1500);
    };
    try {
      root.navigator.clipboard.writeText(text).then(done, done);
    } catch (e) { done(); }
  }

  /// Hand the browser a release's bytes as <name>.krate. The link alone
  /// carries no sign-in, so the bytes are fetched with it (Addendum 2).
  function download(url, filename) {
    return root.fetch(url, { headers: { authorization: "Bearer " + token() } }).then(function (r) {
      if (!r.ok) {
        return r.text().then(function (t) {
          var b = null;
          try { b = JSON.parse(t); } catch (e) {}
          return { ok: false, body: b && b.error ? b : { error: r.status === 401 ? "not-signed-in" : "hub-error" } };
        });
      }
      return r.blob().then(function (blob) {
        var a = root.document.createElement("a");
        a.href = URL.createObjectURL(blob);
        a.download = filename;
        root.document.body.appendChild(a);
        a.click();
        setTimeout(function () { URL.revokeObjectURL(a.href); a.remove(); }, 4000);
        return { ok: true };
      });
    }, function () { return { ok: false, body: { error: "offline" } }; });
  }

  /// This browser's time zone by name ("Asia/Singapore"), or null. An
  /// organisation counts its days in one (F-319, the hub's org-time.js).
  function zone() {
    try {
      var z = Intl.DateTimeFormat().resolvedOptions().timeZone;
      return typeof z === "string" && z ? z : null;
    } catch (e) {
      return null;
    }
  }
  /// A zone as a person says it: "Singapore time", "Los Angeles time".
  function zoneName(z) {
    if (!z || z === "UTC" || z === "Etc/UTC") return "UTC";
    return String(z).split("/").pop().replace(/_/g, " ") + " time";
  }

  function fileName(name) {
    var s = String(name || "app").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
    return (s || "app") + ".krate";
  }

  root.KrateOrg = {
    LIVE_HUB: LIVE_HUB, hub: hub, token: token, forget: forget, key: key, api: api,
    who: who, ROLES: ROLES, ROLE_NAME: ROLE_NAME, ROLE_SAYS: ROLE_SAYS, ROLE_PLAIN: ROLE_PLAIN, MATRIX: MATRIX,
    can: can, tabsFor: tabsFor, invitable: invitable, memberActions: memberActions,
    refusal: refusal, signInHref: signInHref, eventWords: eventWords, fingerprint: fingerprint,
    day: day, moment: moment, zone: zone, zoneName: zoneName, appName: appName, joinUrl: joinUrl, fileName: fileName,
    ssoNotice: ssoNotice, takeSsoNotice: takeSsoNotice,
    OFFLINE: OFFLINE, offlineName: offlineName, offlineNotice: offlineNotice, PROVIDERS: PROVIDERS, money: money,
    el: el, refusalBlock: refusalBlock, signInChoices: signInChoices, copy: copy, download: download,
    groupsSaid: groupsSaid,
    whyShort: function (reason) { return SHORT_WHY[reason] || ""; },
  };
})(typeof window !== "undefined" ? window : globalThis);
