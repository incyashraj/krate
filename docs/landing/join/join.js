/* /join/?o=<slug>&c=<code>: an invitation, then Join, then Krate (IC-947,
 * IC-958 C).
 *
 * The invitation email's one button lands here. Signed out, the page asks
 * the hub only which way in to offer first -- the organisation's company
 * sign-in, when it has one -- and knows nothing else about the invitation
 * (CONTRACT.md Addendum 2). Signed in, the hub shows the invitation to the
 * person it names and to nobody else; one click joins, and the next step
 * is Krate itself. No code to copy on the way: the link carries it, and it
 * is kept under "Having trouble?" for the people who need it.
 */
(function () {
  "use strict";
  var K = window.KrateOrg;
  var el = K.el;
  var $ = function (id) { return document.getElementById(id); };
  var q = new URLSearchParams(location.search);
  var slug = (q.get("o") || "").toLowerCase();
  var code = q.get("c") || "";
  var HERE = "/join/?o=" + encodeURIComponent(slug) + "&c=" + encodeURIComponent(code);

  function say(h, p, kids) {
    $("jH").textContent = h;
    var para = $("jP");
    para.textContent = p || "";
    para.className = "";
    para.hidden = !p;
    var body = $("jBody");
    body.textContent = "";
    (kids || []).forEach(function (k) { if (k) body.appendChild(k); });
  }

  function refused(reason, extra) {
    var ctx = { org: slug, slug: slug, here: HERE };
    for (var k in extra || {}) ctx[k] = extra[k];
    var w = K.refusal(reason, ctx);
    document.title = w.title;
    say(w.title, w.body, [
      w.action ? el("div", { class: "stack" }, [el("a", { class: "btn dark", href: w.action.href, text: w.action.label })]) : null,
    ]);
  }

  /// For the few who need it: the code, and the command that uses it.
  function trouble() {
    var cp = el("button", { class: "btn ghost sm", type: "button", text: "Copy the code" });
    cp.addEventListener("click", function () { K.copy(code, cp); });
    return el("details", { class: "jtrouble" }, [
      el("summary", { text: "Having trouble?" }),
      el("div", { class: "jt" }, [
        el("p", { text: "Sign in with the account the invitation was sent to: the address the email went to, or the GitHub account it names. On a computer without a browser, join from a terminal with the invitation code:" }),
        el("pre", { class: "k", text: "krate org join " + slug + " " + code }),
        cp,
      ]),
    ]);
  }

  if (!/^[a-z][a-z0-9-]{1,38}$/.test(slug) || !/^[A-Za-z0-9_-]{6,80}$/.test(code)) {
    say("This invitation link is incomplete", "Open the link from the email again, or ask for a new one.", [
      el("div", { class: "stack" }, [el("a", { class: "btn ghost", href: "/orgs/", text: "Your organisations" })]),
    ]);
    return;
  }
  $("jTile").textContent = slug.slice(0, 1).toUpperCase();

  if (!K.token()) {
    say("You have an invitation to " + slug, "Sign in with the account it was sent to. You come straight back here to join.", [
      el("div", { id: "jWays" }, [K.signInChoices(HERE)]),
      el("p", { class: "jfine", text: "New to Krate? Signing in makes your account. It is free." }),
      trouble(),
    ]);
    // Which way first: the company's own sign-in when it has one, else the
    // account kind the invitation names. Nothing is said about it here.
    K.api("GET", "/orgs/" + slug + "/invites/" + encodeURIComponent(code) + "/door", null, { token: "" }).then(function (res) {
      if (!res.ok || !res.body) return;
      // The name the email already said, instead of the short name.
      if (res.body.name) {
        $("jH").textContent = "You have an invitation to " + res.body.name;
        $("jTile").textContent = String(res.body.name).slice(0, 1).toUpperCase();
        document.title = "Join " + res.body.name;
      }
      // The same buttons and email box as every other sign-in, led by the
      // kind of account the invitation names; the company's own sign-in
      // above them all, when it has one.
      var box = $("jWays");
      box.textContent = "";
      if (res.body.company_sign_in) {
        box.appendChild(el("div", { class: "jcompany" }, [
          el("a", { class: "btn dark", href: "/login/?org=" + encodeURIComponent(slug) + "&next=" + encodeURIComponent(HERE), text: "Continue with company sign-in" }),
          el("div", { class: "or" }, [el("span", { text: "or" })]),
        ]));
      }
      box.appendChild(K.signInChoices(HERE, { first: res.body.kind === "email" ? "email" : "github" }));
    });
    return;
  }

  /// Who this browser is signed in as, for "this invitation is for someone
  /// else". Best effort: the refusal reads fine without it.
  function whoAmI() {
    return K.api("GET", "/me").then(function (res) {
      var u = res.ok && res.body && res.body.user;
      if (!u) return "";
      return u.email || (u.login ? "@" + u.login : "");
    });
  }

  function refuse(res) {
    var reason = res.body.error;
    if (reason === "not-signed-in") {
      K.forget();
      location.replace(HERE);
      return;
    }
    if (reason === "invite-for-someone-else") {
      whoAmI().then(function (me) { refused(reason, { me: me }); });
      return;
    }
    refused(reason, { message: res.body.message });
  }

  /// Joined: Krate is the next step, for whatever this person can do.
  function joined(org, role) {
    var rn = (K.ROLE_PLAIN[role] || "Can use apps").toLowerCase();
    document.title = "You joined " + org;
    var ios = /iPhone|iPad|Android/i.test(navigator.userAgent || "");
    say("You are in " + org, "You " + rn + " in " + org + ". Its apps open in Krate, on your computer.", [
      el("div", { class: "stack" }, [
        el("a", { class: "btn dark", href: "/download/", text: "Get Krate" }),
        ios ? null : el("a", { class: "btn ghost", href: "krate://org/apps?org=" + encodeURIComponent(slug), text: "Already have Krate? Open it" }),
        el("a", { class: "btn link", href: "/org/?o=" + encodeURIComponent(slug), text: "See " + org + " on the web" }),
      ]),
      el("p", { class: "jfine", text: "Every app " + org + " sends you is signed by " + org + " and checked before it opens." }),
    ]);
  }

  var path = "/orgs/" + slug + "/invites/" + encodeURIComponent(code);
  K.api("GET", path).then(function (res) {
    if (!res.ok) { refuse(res); return; }
    var inv = res.body;
    var org = (inv.org && inv.org.name) || slug;
    var by = K.who(inv.invited_by);
    var forWho = K.who(inv["for"]);
    document.title = "Join " + org;
    $("jTile").textContent = org.slice(0, 1).toUpperCase();
    var accept = el("button", { class: "btn dark", type: "button", text: "Join " + org });
    var key = K.key();
    accept.addEventListener("click", function () {
      accept.disabled = true;
      accept.textContent = "Joining " + org;
      K.api("POST", path + "/accept", {}, { key: key }).then(function (out) {
        accept.disabled = false;
        accept.textContent = "Join " + org;
        if (!out.ok) { refuse(out); return; }
        joined(org, (out.body.membership && out.body.membership.role) || inv.role);
      });
    });
    say("Join " + org, by.label + " invited " + forWho.label + " to " + org + " on Krate.", [
      el("div", { class: "jrole" }, [el("b", { text: K.ROLE_PLAIN[inv.role] || K.ROLE_NAME[inv.role] || inv.role }), el("span", { text: K.ROLE_SAYS[inv.role] || "" })]),
      el("div", { class: "stack" }, [accept]),
      el("p", { class: "jfine", text: "This invitation works once, until " + K.day(inv.expires_at) + "." }),
      trouble(),
    ]);
  });
})();
