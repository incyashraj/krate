/* The Dashboard: the first thing an organisation sees (/org/?o=<slug>).
 *
 * Four numbers that say how the organisation is doing this week, each with
 * which way it moved; people opening its apps, day by day, to point at;
 * what is used most; and what needs this person now. Everything opens its
 * detail in the right-hand panel, so nobody leaves the page to find out
 * more. Built from what Insights already counts (IC-959, GET
 * /orgs/<slug>/insights), so each role sees only its own numbers, and in
 * the organisation's own days (F-319).
 */
(function () {
  "use strict";
  var K = window.KrateOrg;
  var el = K.el;
  var FEWER = "<3";
  var MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

  window.KrateOrgHome = function (D) {
    var slug = D.slug;
    function nameOf() { return (D.S.org && D.S.org.name) || slug; }
    function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
    function num(v) { return v === FEWER ? 0 : Number(v) || 0; }
    function said(v) { return v === FEWER ? "fewer than 3" : String(Number(v) || 0); }
    function isoDay(iso) { var p = String(iso || "").split("-"); return p.length === 3 ? Number(p[2]) + " " + MONTHS[Number(p[1]) - 1] : ""; }
    function waited(s) {
      s = Math.max(0, Number(s) || 0);
      if (s < 3600) return Math.max(1, Math.round(s / 60)) + " min";
      if (s < 2 * 86400) return Math.round(s / 3600) + " h";
      return Math.round(s / 86400) + " days";
    }

    /// One number: its label, the number, which way it moved, and a click
    /// that opens what is behind it.
    function stat(o) {
      var trend = null;
      if (o.delta != null && isFinite(o.delta)) {
        var up = o.delta > 0, flat = o.delta === 0;
        trend = el("span", { class: "hs-d " + (flat ? "flat" : (up === !o.lowerIsBetter ? "good" : "bad")), text: (flat ? "same as" : (up ? "▲ " : "▼ ") + Math.abs(o.delta) + (o.pct ? "%" : "")) + (flat ? " " : " vs ") + (o.versus || "last week") });
      }
      return el("button", { class: "hstat" + (o.warn ? " warn" : ""), type: "button", onclick: o.open, "aria-label": o.label + ": " + o.value + (o.of ? " " + o.of : "") + ". Details" }, [
        el("span", { class: "hs-l", text: o.label }),
        el("span", { class: "hs-v" }, [el("b", { text: o.value }), o.of ? el("small", { text: o.of }) : null]),
        trend || el("span", { class: "hs-d flat", text: o.note || "" }),
      ]);
    }

    /// A day-by-day (or week-by-week) bar chart a person can point at:
    /// every bar is a button that says its number and opens its detail.
    function chart(series, opts) {
      var max = Math.max(1, Math.max.apply(null, series.map(function (d) { return num(d.n); })));
      var tip = el("div", { class: "hc-tip", role: "status", hidden: true });
      var bars = el("div", { class: "hc-bars", style: "--n:" + series.length }, series.map(function (d, i) {
        var h = Math.round(num(d.n) / max * 100);
        var words = d.label + ": " + (d.n === FEWER ? "fewer than 3 people" : plural(num(d.n), "person", "people"));
        var b = el("button", { class: "hc-b" + (d.n === FEWER ? " masked" : "") + (i === series.length - 1 ? " now" : ""), type: "button", "aria-label": words, onclick: function () { opts.open(d, i); } }, [
          el("span", { style: "height:" + Math.max(h, num(d.n) ? 4 : 1.5) + "%" }),
        ]);
        b.addEventListener("mouseenter", function () { tip.textContent = words; tip.hidden = false; tip.style.left = ((i + 0.5) / series.length * 100) + "%"; });
        b.addEventListener("focus", function () { tip.textContent = words; tip.hidden = false; tip.style.left = ((i + 0.5) / series.length * 100) + "%"; });
        b.addEventListener("mouseleave", function () { tip.hidden = true; });
        b.addEventListener("blur", function () { tip.hidden = true; });
        return b;
      }));
      var none = !series.some(function (d) { return num(d.n) > 0 || d.n === FEWER; });
      return el("div", { class: "hchart" + (none ? " empty" : "") }, [
        el("div", { class: "hc-plot" }, [
          el("span", { class: "hc-max", text: String(max) }), el("span", { class: "hc-zero", text: "0" }), bars, tip,
          none ? el("div", { class: "hc-empty", text: opts.empty || "Nothing in this time yet." }) : null,
        ]),
        el("div", { class: "hc-axis" }, [el("span", { text: series.length ? series[0].label : "" }), el("span", { text: series.length ? series[series.length - 1].label : "" })]),
      ]);
    }

    function card(title, sub, kids, cls, tool) {
      return el("section", { class: "card hcard" + (cls ? " " + cls : "") }, [
        el("div", { class: "hc-h" }, [el("div", {}, [el("h3", { text: title }), sub ? el("p", { class: "sub", text: sub }) : null]), tool || null]),
      ].concat(kids));
    }
    function go(tab, text, opts) { return el("button", { class: "btn ghost sm", type: "button", text: text, onclick: function () { D.show(tab, opts); } }); }

    // ------------------------------------------------------------ owners

    function orgView(p, b) {
      var o = b.org, a = o.adoption, pe = o.people, se = o.security, re = o.releases;
      var weeks = (a.trend || []).map(function (w) { return num(w.people); });
      var thisWeek = num(a.active_week), lastWeek = weeks.length > 1 ? weeks[weeks.length - 2] : null;
      var waitingN = num(re.waiting);
      var refused = num(se.refusals.week_total), usual = Math.round(se.refusals.weekly_average_before || 0);

      p.appendChild(el("div", { class: "hstats" }, [
        stat({
          label: "Active this week", value: String(thisWeek), of: "of " + a.members,
          delta: lastWeek == null ? null : thisWeek - lastWeek, versus: "last week",
          open: function () { inspectPeople(o); },
        }),
        stat({
          label: "Apps in use", value: String(a.apps_used), of: "of " + a.apps, note: "opened in the last 30 days",
          open: function () { inspectApps(o); },
        }),
        stat({
          label: "Waiting for approval", value: String(waitingN), warn: waitingN > 0,
          note: waitingN ? "the oldest for " + waited(b.now - re.oldest_waiting_since) : "nothing waiting",
          open: function () { inspectWaiting(b); },
        }),
        stat({
          label: "Refused this week", value: String(refused), warn: se.refusals.spike,
          delta: refused - usual, versus: "a usual week", lowerIsBetter: true,
          open: function () { inspectRefusals(o); },
        }),
      ]));
      // Getting started under the numbers and above the detail.
      if (D.placeStart) D.placeStart(p);

      // People opening apps: 30 days or 12 weeks.
      var mode = "days";
      var box = el("div");
      var seg = el("div", { class: "seg", role: "group", "aria-label": "Show by" }, [
        el("button", { type: "button", "aria-pressed": "true", text: "30 days", onclick: function () { mode = "days"; draw(); } }),
        el("button", { type: "button", "aria-pressed": "false", text: "12 weeks", onclick: function () { mode = "weeks"; draw(); } }),
      ]);
      function draw() {
        seg.querySelectorAll("button").forEach(function (x, i) { x.setAttribute("aria-pressed", (i === 0) === (mode === "days") ? "true" : "false"); });
        box.textContent = "";
        var series = mode === "days"
          ? (a.daily || []).map(function (d) { return { label: isoDay(d.day), n: d.people, day: d.day }; })
          : (a.trend || []).map(function (w) { return { label: isoDay(w.from) + " to " + isoDay(w.to), n: w.people, from: w.from }; });
        box.appendChild(chart(series, { empty: "Nobody opened an app in these " + (mode === "days" ? "30 days" : "12 weeks") + ".", open: function (d, i) { inspectDay(d, series, i, mode); } }));
      }
      draw();
      var top = a.top || [];
      var most = Math.max(1, top.length ? top[0].people : 1);
      p.appendChild(el("div", { class: "hgrid" }, [
        card("People opening apps", "Each day in " + K.zoneName(b.time_zone || "UTC") + ". Point at a bar for its number.", [box], "wide", seg),
        card("Most used", "Last 30 days", [top.length
          ? el("div", { class: "hbars" }, top.map(function (t) {
              return el("button", { class: "hbar", type: "button", onclick: function () { inspectApp(t, o); } }, [
                el("span", { class: "hb-t", text: t.name }), el("span", { class: "hb-n", text: plural(t.people, "person", "people") }),
                el("span", { class: "hb-fill", style: "width:" + Math.round(t.people / most * 100) + "%" }),
              ]);
            }))
          : el("p", { class: "sub", text: "Nobody opened an app in the last 30 days. Share an app's link with your team and this fills in." })]),
        card("Needs you", null, [needs(o, b)]),
      ]));
    }

    /// What this person could do now, most urgent first; each with its click.
    function needs(o, b) {
      var rows = [];
      var re = o.releases, pe = o.people, se = o.security, a = o.adoption;
      if (num(re.waiting)) rows.push([plural(num(re.waiting), "version is", "versions are") + " waiting for approval", "Review", function () { D.show("waiting"); }, true]);
      if (!se.two_person) rows.push(["Nobody else can approve releases yet", "Invite a reviewer", function () { D.show("people", { invite: true }); }]);
      if ((pe.invites || []).length) rows.push([plural(pe.invites.length, "invitation has", "invitations have") + " not been accepted", "See them", function () { D.show("people"); }]);
      if ((pe.inactive || []).length) rows.push([plural(pe.inactive.length, "person has", "people have") + " not been here in 30 days", "See who", function () { inspectPeople(o); }]);
      var silent = (se.devices || []).filter(function (d) { return d.silent; }).length;
      if (silent) rows.push([plural(silent, "computer has", "computers have") + " been silent for 60 days", "See them", function () { D.show("insights"); }]);
      if ((a.unused || []).length) rows.push([plural(a.unused.length, "app nobody opened", "apps nobody opened") + " in 30 days", "See them", function () { inspectApps(o); }]);
      if (se.refusals.spike) rows.push(["More refusals than usual this week", "See why", function () { inspectRefusals(o); }, true]);
      if (!rows.length) return el("p", { class: "hnone" }, [el("span", { class: "ok", "aria-hidden": "true" }), el("span", { text: "Nothing needs you. " + nameOf() + " is in good shape." })]);
      return el("div", { class: "hneeds" }, rows.map(function (r) {
        return el("div", { class: "hn" + (r[3] ? " urgent" : "") }, [el("span", { text: r[0] }), el("button", { class: "btn link sm", type: "button", text: r[1], onclick: r[2] })]);
      }));
    }

    // ------------------------------------------------- the right-hand panel

    function inspectPeople(o) {
      var pe = o.people, a = o.adoption;
      var roles = Object.keys(pe.by_role || {}).filter(function (r) { return pe.by_role[r]; });
      D.inspect("People", [
        el("div", { class: "cr-sec" }, [el("p", { class: "cr-big", text: a.active_week + " of " + a.members }), el("p", { class: "cr-sub", text: "opened one of " + nameOf() + "'s apps in the last 7 days; " + a.active_month + " in 30 days." })]),
        el("div", { class: "cr-sec" }, [el("h3", { text: "By role" }), el("div", { class: "cr-list" }, roles.map(function (r) {
          return el("div", {}, [el("b", { text: K.ROLE_NAME[r] || r }), el("small", { text: String(pe.by_role[r]) })]);
        }))]),
        el("div", { class: "cr-sec" }, [el("h3", { text: "Not seen for 30 days" }), (pe.inactive || []).length
          ? el("div", { class: "cr-list" }, pe.inactive.slice(0, 12).map(function (m) {
              return el("div", {}, [el("b", { text: K.who(m.principal).label }), el("small", { text: m.last_active ? "last on " + isoDay(m.last_active) : "never" })]);
            }))
          : el("p", { class: "cr-sub", text: "Everyone was here this month." })]),
        go("people", "Open People"),
      ]);
    }
    function inspectApps(o) {
      var a = o.adoption;
      D.inspect("Apps in use", [
        el("div", { class: "cr-sec" }, [el("p", { class: "cr-big", text: a.apps_used + " of " + a.apps }), el("p", { class: "cr-sub", text: "of " + nameOf() + "'s apps were opened in the last 30 days." })]),
        el("div", { class: "cr-sec" }, [el("h3", { text: "Most used" }), (a.top || []).length
          ? el("div", { class: "cr-list" }, a.top.map(function (t) { return el("div", {}, [el("b", { text: t.name }), el("small", { text: plural(t.people, "person", "people") })]); }))
          : el("p", { class: "cr-sub", text: "Nobody opened one yet." })]),
        (a.unused || []).length ? el("div", { class: "cr-sec" }, [el("h3", { text: "Nobody opened these" }), el("div", { class: "cr-list" }, a.unused.map(function (u) {
          return el("div", {}, [el("b", { text: u.name }), el("small", { text: "since " + K.day(u.released_at) })]);
        }))]) : null,
        go("apps", "Open Apps"),
      ]);
    }
    function inspectApp(t, o) {
      D.inspect(t.name, [
        el("div", { class: "cr-sec" }, [el("p", { class: "cr-big", text: String(t.people) }), el("p", { class: "cr-sub", text: plural(t.people, "person", "people") + " opened it in the last 30 days, of " + o.adoption.members + " in " + nameOf() + "." })]),
        el("div", { class: "row2" }, [go("releases", "Its releases"), go("insights", "Its numbers")]),
      ]);
    }
    function inspectWaiting(b) {
      var q = (b.reviewer && b.reviewer.queue) || [];
      D.inspect("Waiting for approval", [
        q.length
          ? el("div", { class: "cr-list" }, q.slice(0, 10).map(function (c) { return el("div", {}, [el("b", { text: c.name + " " + (c.version || "") }), el("small", { text: "waiting " + waited(c.waited_seconds) })]); }))
          : el("p", { class: "cr-sub", text: num(b.org && b.org.releases.waiting) ? "Reviewers approve them in Waiting for approval." : "Nothing is waiting. When someone sends a version, it shows here." }),
        D.tabs().indexOf("waiting") >= 0 ? go("waiting", "Open Waiting for approval") : null,
      ]);
    }
    function inspectRefusals(o) {
      var r = o.security.refusals;
      D.inspect("Refused this week", [
        el("div", { class: "cr-sec" }, [el("p", { class: "cr-big", text: String(r.week_total) }), el("p", { class: "cr-sub", text: "attempts the hub said no to in 7 days; about " + Math.round(r.weekly_average_before) + " in a usual week. Each one is in Activity." })]),
        (r.week || []).length ? el("div", { class: "cr-sec" }, [el("h3", { text: "Why" }), el("div", { class: "cr-list" }, r.week.map(function (x) {
          return el("div", {}, [el("b", { text: (K.refusal(x.reason, { org: nameOf() }).title) || x.reason }), el("small", { text: String(x.n) })]);
        }))]) : null,
        D.tabs().indexOf("activity") >= 0 ? go("activity", "Open Activity") : null,
      ]);
    }
    function inspectDay(d, series, i, mode) {
      var before = i > 0 ? series[i - 1] : null;
      D.inspect(d.label, [
        el("div", { class: "cr-sec" }, [el("p", { class: "cr-big", text: d.n === FEWER ? "<3" : String(num(d.n)) }), el("p", { class: "cr-sub", text: (d.n === FEWER ? "Fewer than 3 people" : plural(num(d.n), "person", "people")) + " opened one of " + nameOf() + "'s apps " + (mode === "days" ? "that day" : "that week") + "." })]),
        before ? el("p", { class: "cr-sub", text: "The " + (mode === "days" ? "day" : "week") + " before: " + said(before.n) + "." }) : null,
        go("insights", "More in Insights"),
      ]);
    }

    // ---------------------------------------------------- everyone else

    function greeting() {
      var h = new Date().getHours();
      var part = h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
      var me = D.S.me ? String(K.who(D.S.me).display || "").split("@")[0] : "";
      return part + (me ? ", " + me.charAt(0).toUpperCase() + me.slice(1) : "");
    }
    function when(at) {
      var d = Math.floor((Date.now() / 1000 - (Number(at) || 0)) / 86400);
      return d < 1 ? "today" : d < 2 ? "yesterday" : d < 7 ? d + " days ago" : "on " + K.day(at);
    }

    /// One app a person can open: its tile, when it changed, and Open.
    function launchCard(a, used) {
      var r = a.releases.filter(function (x) { return x.digest === a.current; })[0];
      var name = a.name || a.app_id;
      var say = el("small", { class: "lc-say", role: "status" });
      var open = el("button", { class: "btn dark sm", type: "button", text: "Open in Krate" });
      open.addEventListener("click", function () {
        open.disabled = true; open.textContent = "Getting it";
        var file = K.fileName(r.name);
        K.download(r.url, file).then(function (out) {
          open.disabled = false; open.textContent = "Open in Krate";
          say.textContent = out.ok ? "Saved as " + file + ". Double-click it to open it in Krate." : "It did not download. Try again, or ask the person who released it.";
        });
      });
      var copy = el("button", { class: "btn ghost sm", type: "button", text: "Copy link" });
      copy.addEventListener("click", function () { K.copy(r.url, copy); });
      var fresh = (Date.now() / 1000 - (Number(r.at) || 0)) < 7 * 86400;
      return el("div", { class: "lcard", "data-app": a.app_id }, [
        el("div", { class: "lc-h" }, [
          el("span", { class: "apptile lg", "aria-hidden": "true", text: String(name).slice(0, 1).toUpperCase() }),
          el("span", { class: "lc-t" }, [el("b", { text: name }), el("small", { class: fresh ? "fresh" : "", text: fresh ? "Updated " + when(r.at) : used ? "You opened it " + plural(used.days, "day", "days") + " this month" : "Version " + (r.version || "") })]),
        ]),
        el("div", { class: "lc-a" }, [open, copy]),
        say,
      ]);
    }

    /// Ask for an app in a sentence: it goes to the people who make them.
    function askCard() {
      var box = el("textarea", { class: "inp", rows: "2", placeholder: "A way to book the meeting rooms", "aria-label": "What do you need?" });
      var say = el("small", { class: "lc-say", role: "status" });
      var btn = el("button", { class: "btn dark sm", type: "button", text: "Ask for it" });
      btn.addEventListener("click", function () {
        var text = String(box.value || "").trim();
        if (text.length < 5) { say.textContent = "Say a little more about what it should do."; return; }
        btn.disabled = true;
        K.api("POST", "/orgs/" + slug + "/requests", { text: text }).then(function (res) {
          btn.disabled = false;
          if (res.ok) { box.value = ""; say.textContent = "Sent. You will see the answer in Requests."; return; }
          say.textContent = (res.body && res.body.message) || "It did not send. Try again in a minute.";
        });
      });
      return el("div", { class: "lcard ask" }, [
        el("b", { text: "Need something else?" }),
        el("p", { class: "sub", text: "Say what would save you time. It goes to the people who make " + nameOf() + "'s apps." }),
        box, el("div", { class: "lc-a" }, [btn]), say,
      ]);
    }

    /// A person who opens apps: every app they can open, ready to open.
    function launcher(p, b) {
      K.api("GET", "/orgs/" + slug + "/apps").then(function (res) {
        var apps = ((res.ok && res.body && res.body.apps) || []).filter(function (a) {
          return a.releases.some(function (r) { return r.digest === a.current && r.state === "released"; });
        });
        apps.forEach(function (a) { a.releases.forEach(function (r) { D.S.names[r.digest] = K.appName(r); }); });
        var used = {};
        ((b && b.yours && b.yours.apps) || []).forEach(function (x) { used[x.app_id || x.name] = x; });
        var fresh = apps.filter(function (a) {
          var r = a.releases.filter(function (x) { return x.digest === a.current; })[0];
          return (Date.now() / 1000 - (Number(r.at) || 0)) < 7 * 86400;
        }).length;
        D.head(p, greeting(), apps.length
          ? plural(apps.length, "app", "apps") + " from " + nameOf() + "." + (fresh ? " " + (fresh === 1 ? "One was" : fresh + " were") + " updated this week." : "")
          : nameOf() + " has not released an app for you yet.");
        if (!res.ok && res.status !== 404) return D.failed(p, res, { what: "see your apps" });
        p.appendChild(el("div", { class: "lgrid" }, apps.map(function (a) { return launchCard(a, used[a.app_id] || used[a.name]); }).concat([askCard()])));
        p.appendChild(el("p", { class: "sub hfoot" }, [
          el("span", { text: "Apps open in Krate, free on Mac, Windows and Linux. " }),
          el("a", { href: "/download/", text: "Get Krate" }),
        ]));
      });
    }

    /// A developer: start something, what they have sent and where it is,
    /// and what the team asked for.
    function devView(p, b) {
      var apps = (b.developer && b.developer.apps) || [];
      var people = apps.reduce(function (s, x) { return s + num(x.people_week); }, 0);
      var refused = apps.reduce(function (s, x) { return s + (x.refusals || []).reduce(function (t, r) { return t + (Number(r.n) || 0); }, 0); }, 0);
      var tiles = [
        stat({ label: "Your apps", value: String(apps.length), note: apps.filter(function (x) { return x.released; }).length + " released", open: function () { D.show("myapps"); } }),
        stat({ label: "People this week", value: String(people), note: "opened your apps", open: function () { D.show("insights"); } }),
        stat({ label: "Refused, 30 days", value: String(refused), warn: refused > 0, note: refused ? "see why on each app" : "nothing refused", open: function () { D.show("myapps"); } }),
      ];
      if (b.reviewer) {
        var rq = b.reviewer.queue || [];
        tiles.push(stat({ label: "Waiting for you", value: String(rq.length), warn: rq.length > 0, note: rq.length ? "the oldest for " + waited(rq[0].waited_seconds) : "nothing waiting", open: function () { D.show("waiting"); } }));
      }
      p.appendChild(el("div", { class: "hstats" }, tiles));
      if (D.placeStart) D.placeStart(p);

      p.appendChild(el("div", { class: "ways" }, [
        way("Describe it", "Say what it should do. Krate Studio writes it, you watch it run, then send it here.", el("a", { class: "btn dark sm", href: "/app/", text: "Open Studio" })),
        way("Start from a ready one", "A working app with sign-in and a sealed store, ready to change.", el("button", { class: "btn ghost sm", type: "button", text: "See starters", onclick: function () { if (D.releaseFlow) D.releaseFlow(); else D.show("build"); } })),
        way("Send your own", "Built it yourself? One command sends a .krate to " + nameOf() + ".", el("button", { class: "btn ghost sm", type: "button", text: "Show the command", onclick: function () { D.show("build"); } })),
      ]));

      var kids = [];
      kids.push(card("Your apps", "People who opened each one this week", [apps.length
        ? el("div", { class: "hbars" }, apps.map(function (x) {
            var most = Math.max(1, Math.max.apply(null, apps.map(function (y) { return num(y.people_week); })));
            return el("button", { class: "hbar", type: "button", onclick: function () { D.openApp(x.app_id); } }, [
              el("span", { class: "hb-t", text: x.name || x.app_id }), el("span", { class: "hb-n", text: said(x.people_week) }),
              el("span", { class: "hb-fill", style: "width:" + Math.round(num(x.people_week) / most * 100) + "%" }),
            ]);
          }))
        : el("p", { class: "sub", text: "You have not sent an app yet. Start one above." })]));
      var asks = el("div", {}, [el("p", { class: "sub", text: "Loading" })]);
      kids.push(card("Asked for by the team", null, [asks]));
      p.appendChild(el("div", { class: "hgrid" }, kids));
      K.api("GET", "/orgs/" + slug + "/requests").then(function (res) {
        asks.textContent = "";
        var list = ((res.ok && res.body && res.body.requests) || []).filter(function (r) { return r.state === "open" || r.state === "planned"; });
        if (!list.length) { asks.appendChild(el("p", { class: "sub", text: res.ok ? "Nobody has asked for an app. When someone does, it shows here." : "Requests are not on this hub yet." })); return; }
        asks.appendChild(el("div", { class: "hneeds" }, list.slice(0, 5).map(function (r) {
          return el("div", { class: "hn" }, [el("span", { text: "\u201c" + r.text + "\u201d, " + K.who(r.by).label }), el("small", { text: r.state === "planned" ? "Planned" : when(r.at) })]);
        })));
      });
    }
    function way(title, text, act) {
      return el("section", { class: "card way" }, [el("h3", { text: title }), el("p", { text: text }), el("div", { class: "row2" }, [act])]);
    }

    /// A reviewer: what waits, oldest first, and what they decided.
    function reviewerView(p, b) {
      var rq = (b.reviewer && b.reviewer.queue) || [];
      p.appendChild(el("div", { class: "hstats" }, [
        stat({ label: "Waiting for you", value: String(rq.length), warn: rq.length > 0, note: rq.length ? "the oldest for " + waited(rq[0].waited_seconds) : "nothing waiting", open: function () { D.show("waiting"); } }),
        stat({ label: "Approved by you", value: String(b.reviewer.approved_by_you_month), note: "in the last 30 days", open: function () { D.show("waiting"); } }),
      ]));
      p.appendChild(el("div", { class: "hgrid" }, [card("Waiting", "Oldest first", [rq.length
        ? el("div", { class: "hneeds" }, rq.slice(0, 8).map(function (c) {
            return el("div", { class: "hn" }, [el("span", { text: c.name + " " + (c.version || "") + ", waiting " + waited(c.waited_seconds) }), el("button", { class: "btn link sm", type: "button", text: "Review", onclick: function () { D.show("waiting"); } })]);
          }))
        : el("p", { class: "hnone" }, [el("span", { class: "ok", "aria-hidden": "true" }), el("span", { text: "Nothing waits for you." })])], "wide")]));
    }

    // ------------------------------------------------------------- pane

    function pane(p) {
      var role = D.roleOf();
      if (role !== "owner" && role !== "admin" && role !== "developer" && role !== "reviewer") {
        K.api("GET", "/orgs/" + slug + "/insights").then(function (res) { launcher(p, res.ok ? res.body : null); });
        return;
      }
      var acts = [];
      if (K.can(role, "candidate.publish")) acts.push({ label: role === "developer" ? "New app" : "Release an app", dark: true, onclick: function () { if (role === "developer") D.show("build"); else if (D.releaseFlow) D.releaseFlow(); else D.show("releases"); } });
      if (K.can(role, "invite.other") && role !== "developer") acts.push({ label: "Invite people", onclick: function () { D.show("people", { invite: true }); } });
      K.api("GET", "/orgs/" + slug + "/insights").then(function (res) {
        var org = role === "owner" || role === "admin";
        D.head(p, org ? "Overview" : greeting(), org
          ? "How " + nameOf() + " is doing this week. Click any number for what is behind it."
          : role === "developer" ? "What you are making for " + nameOf() + ", and who uses it." : "What waits for your approval in " + nameOf() + ".", acts);
        if (!res.ok) {
          if (D.placeStart) D.placeStart(p);
          // A hub without Insights still gets a useful first page.
          p.appendChild(el("div", { class: "card" }, [el("h3", { text: "Your apps" }), el("p", { class: "sub", text: "The numbers did not load. Company apps has every app " + nameOf() + " can open." }), go("releases", "Open Company apps")]));
          return;
        }
        var b = res.body;
        if (b.org) orgView(p, b);
        else if (b.developer) devView(p, b);
        else if (b.reviewer) reviewerView(p, b);
        else { launcher(p, b); return; }

        p.appendChild(el("p", { class: "sub hfoot", text: "Days run midnight to midnight, " + K.zoneName(b.time_zone || "UTC") + ". Counts only: Krate never sees what anyone does inside an app." }));
      });
    }

    return { pane: pane };
  };
})();
