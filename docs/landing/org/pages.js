/* The console's own sections (the org console by role, 2026-10-16):
 * Computers, Security and Connections for those who run the organisation;
 * New app and My apps for those who make its apps; Help for everyone who
 * only uses them. Each reads what the hub already keeps and says it in
 * words; each row opens its detail on the right.
 */
(function (root) {
  "use strict";
  var K = root.KrateOrg;
  var el = K.el;

  root.KrateOrgPages = function (D) {
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    function nameOf() { return (D.S.org && D.S.org.name) || slug; }
    function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
    function ago(at) {
      if (!at) return "";
      var s = Math.max(0, Date.now() / 1000 - at);
      if (s < 3600) return Math.max(1, Math.round(s / 60)) + " min ago";
      if (s < 86400) return Math.round(s / 3600) + " h ago";
      if (s < 2 * 86400) return "yesterday";
      return K.day(at);
    }
    /// A panel of rows under a heading: Studio's one surface per section.
    function panel(title, rows, extra) {
      return el("section", { class: "card pnl" }, [
        title ? el("div", { class: "pnl-h" }, [el("h3", { text: title }), extra || null]) : null,
      ].concat(rows));
    }
    function row(main, sub, side, onclick) {
      var r = el(onclick ? "button" : "div", { class: "prow" + (onclick ? " opens" : ""), type: onclick ? "button" : null, onclick: onclick || null }, [
        el("span", { class: "pr-t" }, [el("b", { text: main }), sub ? el("small", { text: sub }) : null]),
        side ? (typeof side === "string" ? el("span", { class: "pr-s", text: side }) : side) : null,
      ]);
      return r;
    }
    function stateDot(text, tone) { return el("span", { class: "pdot " + (tone || "") }, [el("i", { "aria-hidden": "true" }), text]); }
    function empty(title, body) { return el("div", { class: "empty" }, [el("b", { text: title }), el("p", { text: body })]); }

    // ------------------------------------------------------------ computers

    function computers(p) {
      Promise.all([K.api("GET", path("/keys")), K.api("GET", path("/signing"))]).then(function (got) {
        var keys = got[0], sign = got[1];
        D.head(p, "Computers", "Every computer that can open " + nameOf() + "'s team data, and the ones that sign its releases. A computer leaves when its person does.");
        if (!keys.ok) return D.failed(p, keys, { what: "see the computers", whoCan: "members" });
        var devs = (keys.body.devices || []).filter(function (d) { return d.state !== "removed"; });
        var cur = keys.body.current || {};
        var signers = {};
        ((sign.ok && sign.body.holders) || []).forEach(function (h) { (h.devices || []).forEach(function (n) { signers[h.principal + "|" + n] = true; }); });
        var holding = devs.filter(function (d) { return d.holds === cur.version; });
        var waiting = (keys.body.requests || []).length;
        p.appendChild(el("div", { class: "hstats three" }, [
          tile("With access", String(holding.length), "of " + devs.length + " computers"),
          tile("Waiting to be let in", String(waiting), waiting ? "an owner's computer lets them in when it next opens " + nameOf() : "nobody waiting"),
          tile("Team data key", cur.state === "stale" ? "Renew" : "Current", cur.state === "stale" ? "someone left since it was made" : "version " + (cur.version || 1) + ", made " + K.day(cur.created_at), cur.state === "stale"),
        ]));
        if (!devs.length) { p.appendChild(panel("", [empty("No computer holds access yet", "The first owner's computer makes it when Krate or this page releases an app.")])); return; }
        var table = el("div", { class: "ptable" }, [
          el("div", { class: "pth" }, [el("span", { text: "Computer" }), el("span", { text: "Person" }), el("span", { text: "Added" }), el("span", { text: "Can" })]),
        ].concat(devs.map(function (d) {
          var signs = signers[d.principal + "|" + d.name];
          var can = d.holds === cur.version ? (signs ? "Open team data · sign releases" : "Open team data") : "Waiting for the new key";
          var r = el("button", { class: "ptr opens", type: "button", onclick: function () { inspectDevice(d, signs, cur); } }, [
            el("span", { class: "pc" }, [el("span", { class: "dev-ic", "aria-hidden": "true" }, [d.kind === "browser" ? "B" : "K"]), el("b", { text: d.name || "A computer" })]),
            el("span", { text: K.who(d.principal).label }),
            el("span", { text: K.day(d.added_at) }),
            el("span", {}, [stateDot(can, d.holds === cur.version ? "ok" : "warn")]),
          ]);
          return r;
        })));
        p.appendChild(el("section", { class: "card pnl flush" }, [table]));
      });
    }
    function tile(label, value, sub, warn) {
      return el("div", { class: "hstat static" + (warn ? " warn" : "") }, [el("span", { class: "hs-l", text: label }), el("span", { class: "hs-v" }, [el("b", { text: value })]), el("span", { class: "hs-d flat", text: sub })]);
    }
    function inspectDevice(d, signs, cur) {
      D.inspect(d.name || "A computer", [
        el("dl", { class: "cr-kv" }, [
          el("dt", { text: "Belongs to" }), el("dd", { text: K.who(d.principal).label }),
          el("dt", { text: "Kind" }), el("dd", { text: d.kind === "browser" ? "A browser" : "Krate on a computer" }),
          el("dt", { text: "Added" }), el("dd", { text: K.moment(d.added_at) }),
          el("dt", { text: "Team data" }), el("dd", { text: d.holds === cur.version ? "Can open it" : "Waiting for the new key" }),
          el("dt", { text: "Releases" }), el("dd", { text: signs ? "Can sign them as " + nameOf() : "Does not sign" }),
        ]),
        el("p", { class: "cr-sub", text: "A computer leaves with its person: removing them in People takes its access away and renews the key." }),
        el("button", { class: "btn ghost sm", type: "button", text: "Open People", onclick: function () { D.show("people"); } }),
      ]);
    }

    // ------------------------------------------------------------ security

    function security(p) {
      var calls = [K.api("GET", path("/keys")), K.api("GET", path("/signing")), K.api("GET", path("/sso")), K.api("GET", path("/audit")), K.api("GET", path("/audit-export"))];
      Promise.all(calls).then(function (got) {
        var keys = got[0], sign = got[1], sso = got[2], audit = got[3], exp = got[4];
        var o = D.S.org || {};
        D.head(p, "Security", "How people get in, what holds the keys, and what was refused.", [{ label: "Download the IT pack", onclick: function () { D.itPack(); } }]);
        var cur = (keys.ok && keys.body.current) || {};
        var holders = (sign.ok && sign.body.holders) || [];
        var company = sso.ok && sso.body && (sso.body.issuer || sso.body.domain);
        var cards = [];
        cards.push(scard("Signing in", company ? "Company sign-in" : "GitHub, Google or email", company ? "ok" : "",
          company ? "People sign in with their company account at " + (sso.body.domain || "your domain") + (sso.body.enforce ? ", and nothing else is accepted." : ".") : "Anyone you invite signs in with GitHub, Google or an email link. Company sign-in adds and removes people with your directory.",
          company ? "Sign-in settings" : "Set up company sign-in", function () { D.show("settings"); }));
        var stale = cur.state === "stale";
        cards.push(scard("Team data keys", stale ? "Renew" : "Current", stale ? "warn" : "ok",
          stale ? "Someone left since the key was made, so the computers they had can still read what was written before. Renewing takes a moment and nobody notices." : "Only " + nameOf() + "'s computers can read its team data. Version " + (cur.version || 1) + ", made " + K.day(cur.created_at) + ".",
          stale ? "Renew now" : "See the computers", stale ? function () { renew(p); } : function () { D.show("computers"); }));
        cards.push(scard("Signing releases", plural(holders.length, "person holds it", "people hold it"), holders.length > 1 ? "ok" : "warn",
          "Every release is signed as " + nameOf() + ". " + (holders.length ? holders.map(function (h) { return K.who(h.principal).label; }).join(", ") + (holders.length === 1 ? " holds" : " hold") + " the signature." : "Nobody holds it yet."),
          "Who approves releases", function () { D.show("settings"); }));
        cards.push(scard("Second approval", o.release_rule === "solo" ? "Not yet" : "On", o.release_rule === "solo" ? "warn" : "ok",
          o.release_rule === "solo" ? "Until someone else can approve, an owner releases their own apps alone. It turns on when a reviewer joins." : "Nobody releases their own version: a second person approves every one.",
          o.release_rule === "solo" ? "Invite a reviewer" : "See the queue", function () { D.show(o.release_rule === "solo" ? "people" : "waiting", o.release_rule === "solo" ? { invite: true } : null); }));
        var hooked = exp.ok && exp.body && exp.body.url;
        cards.push(scard("Activity export", hooked ? "On" : "Off", hooked ? "ok" : "",
          hooked ? "Every entry, signed, goes to " + exp.body.url + " as it happens." : "Send every entry, signed, to your logs or SIEM as it happens.",
          hooked ? "Change where it goes" : "Set it up", function () { D.show("settings"); }));
        cards.push(scard("Rules for every computer", "IT pack", "",
          "Your device manager sends Krate's rules to every computer: which hosts apps may reach, what they may never use, which hub they trust.",
          "Download the IT pack", function () { D.itPack(); }));
        p.appendChild(el("div", { class: "sgrid" }, cards));

        // Who got in, and who was turned away.
        var ACCESS = /^(sso\.join|invite\.accept|member\.add|member\.remove|key\.device\.add|signing\.device\.remove|key\.device\.remove)$/;
        var ev = audit.ok ? (audit.body.events || []).filter(function (e) { return ACCESS.test(e.action) || (e.result === "denied" && /^(org\.view|release\.download|candidate\.)/.test(e.action)); }).slice(-25).reverse() : [];
        var list = ev.length ? ev.map(function (e) {
          return row(K.eventWords(e, D.S.names), K.moment(e.at), stateDot(e.result === "denied" ? "Refused" : "Allowed", e.result === "denied" ? "bad" : "ok"), function () { D.inspectEvent(e); });
        }) : [empty("Nothing yet", "Who joins, signs in with company accounts, adds a computer or is refused shows here.")];
        p.appendChild(panel("Recent access", list, K.can(D.roleOf(), "audit.read") ? el("button", { class: "btn link sm", type: "button", text: "All activity", onclick: function () { D.show("activity"); } }) : null));
      });
    }
    function scard(title, badge, tone, body, act, go) {
      return el("section", { class: "card scard" + (tone === "warn" ? " warn" : "") }, [
        el("div", { class: "sc-h" }, [el("h3", { text: title }), stateDot(badge, tone)]),
        el("p", { text: body }),
        el("button", { class: "btn link sm", type: "button", text: act, onclick: go }),
      ]);
    }
    function renew(p) {
      D.device().then(function (dev) {
        if (!dev) { D.toast("Renewing needs a computer that holds the key: open Krate on an owner's computer, or this page in the browser that set " + nameOf() + " up."); return; }
        return Promise.resolve(dev.ready(slug, {})).then(function () { D.toast("The team data key is renewed. Computers that left cannot read anything new."); security(p); });
      }).catch(function (err) {
        // The hub's own words when it answered; the key message only when
        // this browser really does not hold the key (K-1120).
        if (err && err.reason && err.message) { D.toast("Renewing did not finish: " + err.message); return; }
        D.toast("This browser does not hold " + nameOf() + "'s key. Renew it from a computer that does.");
      });
    }

    // --------------------------------------------------------- connections

    function connections(p) {
      D.head(p, "Connections", K.can(D.roleOf(), "connection.manage")
        ? "Company systems your apps may reach, with the credential kept here, sealed: an app sends its requests through Krate and never holds it."
        : "The company systems an app may reach, by name. An app names one in its manifest and Krate adds the credential on the way.");
      if (D.C) D.C.settings(p);
    }

    // -------------------------------------------------------------- new app

    function build(p) {
      // Studio in the browser, opened for this organisation: its Send menu
      // offers it first, and a spreadsheet's records become its team data.
      var studio = "/app/?org=" + encodeURIComponent(slug);
      D.head(p, "New app", "Four ways to start. Whichever you pick, the app is signed for " + nameOf() + " and waits for a second person before anyone opens it.");
      p.appendChild(el("div", { class: "ways four" }, [
        way("Describe it", "Say what the team needs. Krate Studio builds it in the browser, and you send it to " + nameOf() + " from there.", [
          el("a", { class: "btn dark sm", href: studio, text: "Describe it" }),
          el("a", { class: "btn ghost sm", href: "/download/", text: "Get Studio for the desktop" }),
        ]),
        way("From a spreadsheet", "A .csv or .xlsx the team keeps today. Krate builds an app around its rows, kept as " + nameOf() + "'s team data.", [
          el("a", { class: "btn ghost sm", href: studio + "&start=sheet", text: "Start from a spreadsheet" }),
        ]),
        way("From a ready app", "Fifteen to pick from: help desk, leave, timesheets, a shift rota and more. Release one as it is, then change it to fit.", [
          el("button", { class: "btn ghost sm", type: "button", text: "Pick one", onclick: function () { D.releaseFlow(); } }),
        ]),
        way("From your code", "Rust, C or Go, to one .krate, sent from the terminal.", [
          el("pre", { class: "k", text: "krate publish app.krate --org " + slug }),
          el("a", { class: "btn link sm", href: "/docs/", text: "Docs and SDK" }),
        ]),
      ]));
    }
    function way(title, body, acts) {
      return el("section", { class: "card way" }, [el("h3", { text: title }), el("p", { text: body }), el("div", { class: "row2" }, acts)]);
    }

    // -------------------------------------------------------------- my apps

    function myapps(p) {
      Promise.all([K.api("GET", path("/candidates")), K.api("GET", path("/insights"))]).then(function (got) {
        var cand = got[0], ins = got[1];
        D.head(p, "My apps", "Every app you sent to " + nameOf() + ", where each one is, and who uses it.", [{ label: "New app", dark: true, onclick: function () { D.show("build"); } }]);
        if (!cand.ok) return D.failed(p, cand, { what: "see your apps" });
        var mine = (cand.body || []).filter(function (c) { return D.isMe(c.uploaded_by); });
        mine.forEach(function (c) { D.S.names[c.digest] = K.appName(c); });
        var usage = {};
        ((ins.ok && ins.body.developer && ins.body.developer.apps) || []).forEach(function (a) { usage[a.app_id] = a; });
        var byApp = {};
        mine.forEach(function (c) { var id = c.app_id || c.name; (byApp[id] = byApp[id] || []).push(c); });
        var ids = Object.keys(byApp);
        if (!ids.length) {
          p.appendChild(panel("", [empty("You have not sent an app yet", "Start one in New app: describe it, start from a ready one, or send your own .krate.")]));
          return;
        }
        var table = el("div", { class: "ptable" }, [el("div", { class: "pth four" }, [el("span", { text: "App" }), el("span", { text: "Where it is" }), el("span", { text: "People, 7 days" }), el("span", { text: "Refused, 30 days" })])].concat(ids.map(function (id) {
          var list = byApp[id].sort(function (a, b) { return (b.uploaded_at || 0) - (a.uploaded_at || 0); });
          var last = list[0], u = usage[id] || {};
          var where = last.state === "waiting-for-approval" ? ["With a reviewer", "warn"] : last.state === "declined" ? ["Declined" + (last.decline_reason ? ": " + last.decline_reason : ""), "bad"] : last.state === "withdrawn" ? ["Withdrawn", "bad"] : ["Released", "ok"];
          var refused = (u.refusals || []).reduce(function (s, r) { return s + (Number(r.n) || 0); }, 0);
          return el("button", { class: "ptr four opens", type: "button", onclick: function () { D.openApp(id); } }, [
            el("span", { class: "pc" }, [el("span", { class: "apptile sm", "aria-hidden": "true", text: String(last.name || id).slice(0, 1).toUpperCase() }), el("span", { class: "pr-t" }, [el("b", { text: last.name || id }), el("small", { class: "mono", text: last.version || "" })])]),
            el("span", {}, [stateDot(where[0], where[1])]),
            el("span", { text: u.people_week == null ? "-" : String(u.people_week) }),
            el("span", { text: u.refusals ? String(refused) : "-" }),
          ]);
        })));
        p.appendChild(el("section", { class: "card pnl flush" }, [table]));
      });
    }

    // ------------------------------------------------------------- requests

    var REQ_STATE = { open: ["Asked", ""], planned: ["Planned", "warn"], done: ["Made", "ok"], declined: ["Not planned", "bad"] };

    /// App requests: anyone asks in a sentence; the people who make apps
    /// answer with planned, made or not planned, and a line why.
    function requests(p) {
      var manage = K.can(D.roleOf(), "request.manage");
      K.api("GET", path("/requests")).then(function (res) {
        D.head(p, "Requests", manage
          ? "What people in " + nameOf() + " asked for. Answer each one so they know where it stands."
          : "Ask for an app in a sentence. The people who make " + nameOf() + "'s apps answer here.");
        if (!res.ok && res.status === 404 && (!res.body || res.body.error !== "no-such-org")) {
          p.appendChild(panel("", [empty("Not on this hub yet", "Requests arrive with the next hub update.")]));
          return;
        }
        if (!res.ok) return D.failed(p, res, { what: "see the requests" });
        var list = (res.body && res.body.requests) || [];

        var box = el("textarea", { class: "inp", rows: "2", placeholder: "A way to book the meeting rooms", "aria-label": "What do you need?" });
        var say = el("small", { class: "lc-say", role: "status" });
        var send = el("button", { class: "btn dark sm", type: "button", text: "Ask for it" });
        send.addEventListener("click", function () {
          var text = String(box.value || "").trim();
          if (text.length < 5) { say.textContent = "Say a little more about what it should do."; return; }
          send.disabled = true;
          K.api("POST", path("/requests"), { text: text }).then(function (out) {
            send.disabled = false;
            if (!out.ok) { say.textContent = (out.body && out.body.message) || "It did not send. Try again in a minute."; return; }
            D.toast("Sent");
            D.show("requests");
          });
        });
        // Those who answer requests make the apps: no box to ask themselves.
        if (!manage) p.appendChild(el("section", { class: "card pnl ask" }, [
          el("div", { class: "pnl-h" }, [el("h3", { text: "Ask for an app" })]),
          el("div", { class: "pask" }, [box, el("div", { class: "row2" }, [send]), say]),
        ]));

        var filter = "open";
        var seg = el("div", { class: "seg", role: "group", "aria-label": "Show" });
        var body = el("div", { class: "pnl-b" });
        var GROUPS = [["open", "Open"], ["planned", "Planned"], ["answered", "Answered"]];
        function inGroup(r, g) { return g === "answered" ? r.state === "done" || r.state === "declined" : r.state === g; }
        // Start on the first group that has something in it.
        filter = (GROUPS.filter(function (g) { return list.some(function (r) { return inGroup(r, g[0]); }); })[0] || GROUPS[0])[0];
        function draw() {
          seg.textContent = "";
          GROUPS.forEach(function (g) {
            var n = list.filter(function (r) { return inGroup(r, g[0]); }).length;
            seg.appendChild(el("button", { type: "button", "aria-pressed": filter === g[0] ? "true" : "false", text: g[1] + (n ? " " + n : ""), onclick: function () { filter = g[0]; draw(); } }));
          });
          body.textContent = "";
          var shown = list.filter(function (r) { return inGroup(r, filter); });
          if (!shown.length) { body.appendChild(empty(filter === "open" ? "Nothing open" : "None yet", filter === "open" ? (manage ? "When someone asks for an app, it shows here." : "Ask above and it shows here until someone answers.") : "Answered requests show here.")); return; }
          shown.forEach(function (r) {
            var st = REQ_STATE[r.state] || [r.state, ""];
            body.appendChild(row("\u201c" + r.text + "\u201d", (manage ? K.who(r.by).label + ", " : "") + ago(r.at) + (r.reply ? " \u00b7 " + K.who(r.replied_by).label + ": " + r.reply : ""), stateDot(st[0], st[1]), manage ? function () { answer(r); } : null));
          });
        }
        draw();
        p.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: manage ? "From the team" : "Yours" }), seg]), body]));
      });
    }

    /// One request on the right: who asked, and the answer.
    function answer(r) {
      var reply = el("textarea", { class: "inp", rows: "3", placeholder: "What happens next, in a line" });
      reply.value = r.reply || "";
      var say = el("small", { class: "lc-say", role: "status" });
      function set(state) {
        K.api("PUT", path("/requests/" + encodeURIComponent(r.id)), { state: state, reply: String(reply.value || "").trim() }).then(function (out) {
          if (!out.ok) { say.textContent = (out.body && out.body.message) || "It did not save. Try again."; return; }
          D.toast("Answered");
          D.show("requests");
        });
      }
      D.inspect("A request", [
        el("div", { class: "cr-sec" }, [el("p", { class: "cr-sub", text: "\u201c" + r.text + "\u201d" }), el("p", { class: "cr-sub", text: K.who(r.by).label + ", " + ago(r.at) })]),
        el("div", { class: "cr-sec" }, [el("h3", { text: "Your answer" }), reply]),
        el("div", { class: "row2" }, [
          el("button", { class: "btn dark sm", type: "button", text: "Planned", onclick: function () { set("planned"); } }),
          el("button", { class: "btn ghost sm", type: "button", text: "Made", onclick: function () { set("done"); } }),
          el("button", { class: "btn quiet sm", type: "button", text: "Not planned", onclick: function () { set("declined"); } }),
        ]),
        say,
      ]);
    }

    // ----------------------------------------------------------------- help

    function help(p) {
      D.head(p, "Help", "Opening " + nameOf() + "'s apps, and who to ask.");
      var owners = (D.S.members || []).filter(function (m) { return m.role === "owner" || m.role === "admin"; });
      p.appendChild(el("div", { class: "ways" }, [
        way("Get Krate", "Apps open in Krate, free on Mac, Windows and Linux. Install it once and every app opens with a double-click.", [el("a", { class: "btn dark sm", href: "/download/", text: "Download Krate" })]),
        way("Ask for an app", "Say in a sentence what would save you time. The people who make " + nameOf() + "'s apps answer in Requests.", [el("button", { class: "btn ghost sm", type: "button", text: "Open Requests", onclick: function () { D.show("requests"); } })]),
        way("An app will not open", "It says why in words. Usually: it was withdrawn, it is for another group, or offline access ran out. Connect to the internet once and it checks again.", []),
      ]));
      // The groups this person is in: some apps open only for certain groups.
      var groupsAt = el("div", { hidden: true });
      p.appendChild(groupsAt);
      if (D.groups) D.groups(groupsAt);
      p.appendChild(panel("Who to ask", owners.length ? owners.map(function (m) { return row(K.who(m).label, K.ROLE_NAME[m.role] || m.role); }) : [el("p", { class: "sub", style: "padding:0 0 16px", text: "Ask whoever invited you to " + nameOf() + "." })]));
    }

    return { computers: computers, security: security, connections: connections, build: build, myapps: myapps, requests: requests, help: help };
  };
})(typeof window !== "undefined" ? window : globalThis);
