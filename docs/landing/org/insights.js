/* The organisation dashboard's Insights tab (IC-959, F-318): each person
 * sees the numbers their decisions need, by role, in one answer from the
 * hub (GET /orgs/<slug>/insights):
 *
 *  - owners and admins: is the organisation getting value, and is it safe
 *    (adoption, people, security, releases, cost);
 *  - whoever sends apps: how each of their apps is doing;
 *  - whoever approves: what needs them;
 *  - everyone: their own computers and the apps they used.
 *
 * Every number has a line that says what it means and, where something
 * can be done about it, the link that does it. Every chart has its
 * numbers as text beside it. The hub decides what each role reads: names
 * only for owners and admins, and a count of 1 or 2 people arrives as
 * "<3", said here as "fewer than 3". Counts only -- the hub never sees
 * what anyone does inside an app.
 *
 * dashboard.js owns the page and hands this part what it needs (D).
 */
(function (root) {
  "use strict";

  var FEWER = "<3";
  var MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

  root.KrateOrgInsights = function (D) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var nameOf = function () { return (D.S.org && D.S.org.name) || slug; };
    var HERE = "/org/?o=" + encodeURIComponent(slug);

    // ------------------------------------------------------------ words

    function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
    /// "Active in the last 30 days": who has not opened an app, without
    /// counting a person who joined this week as a month away.
    function monthTile(a, inactive) {
      var missing = a.members - a.active_month;
      var fresh = Math.min(missing, a.joined_not_opened || 0);
      var settled = missing - fresh;
      var means;
      if (!missing) means = "everyone opened one this month";
      else if (!settled) means = plural(fresh, "person", "people") + " joined in the last 30 days and " + (fresh === 1 ? "has" : "have") + " not opened one yet";
      else means = plural(settled, "person has", "people have") + " not opened one in a month" + (fresh ? "; " + fresh + " more joined recently" : "");
      return { label: "Active in the last 30 days", value: String(a.active_month), of: "of " + a.members, means: means, act: settled && inactive.length ? jump("ins-inactive", "See who has not") : null };
    }
    function peopleSaid(v) {
      if (v === FEWER) return "fewer than 3 people";
      return plural(Number(v) || 0, "person", "people");
    }
    function numSaid(v) { return v === FEWER ? "fewer than 3" : String(v); }
    function isoDay(iso) {
      var p = String(iso || "").split("-");
      if (p.length !== 3) return "";
      return Number(p[2]) + " " + MONTHS[Number(p[1]) - 1];
    }
    function since(seconds, now) {
      var s = Math.max(0, (now || 0) - (seconds || 0));
      return waited(s);
    }
    /// "40 minutes", "2 hours", "3 days".
    function waited(s) {
      s = Math.max(0, Math.round(s || 0));
      if (s < 60) return "under a minute";
      if (s < 90 * 60) return plural(Math.round(s / 60), "minute", "minutes");
      if (s < 23.5 * 3600) return plural(Math.round(s / 3600), "hour", "hours");
      return plural(Math.round(s / 86400), "day", "days");
    }
    function when(seconds, now) {
      if (!seconds) return "";
      var d = Math.floor((now - seconds) / 86400);
      if (d <= 0 && now - seconds < 86400) return "today";
      if (d === 1) return "yesterday";
      return K.day(seconds);
    }
    function bytes(n) {
      n = Number(n) || 0;
      if (n < 1024) return plural(n, "byte", "bytes");
      if (n < 1024 * 1024) return (n / 1024).toFixed(n < 10240 ? 1 : 0) + " KB";
      return (n / 1024 / 1024).toFixed(n < 10 * 1024 * 1024 ? 1 : 0) + " MB";
    }
    function why(reason) {
      var w = K.whyShort ? K.whyShort(reason) : "";
      w = w || String(reason || "").replace(/-/g, " ");
      return w.charAt(0).toUpperCase() + w.slice(1);
    }

    // ---------------------------------------------------------- pieces

    /// A link that opens another tab of this page: where to act.
    function go(tab, text, extra) {
      return el("a", {
        class: "act", href: HERE + "&tab=" + tab,
        onclick: function (e) { if (e && e.preventDefault) e.preventDefault(); D.show(tab); },
        "aria-label": extra || null,
      }, [text]);
    }
    function jump(id, text) { return el("a", { class: "act", href: "#" + id }, [text]); }

    /// One number: what it is, the number, what it means, where to act.
    function tile(o) {
      return el("div", { class: "tile" + (o.warn ? " warn" : "") }, [
        el("span", { class: "lbl", text: o.label }),
        // A number reads large; a word ("2 hours", "Not yet") a size down, on one line.
        el("b", { class: "val" + (/^[\d$€£<]/.test(String(o.value)) && !/ /.test(String(o.value)) ? "" : " word") }, [o.value, o.of ? el("small", { text: " " + o.of }) : null]),
        el("small", { class: "means", text: o.means }),
        o.act || null,
      ]);
    }
    function tiles(list) {
      var kept = list.filter(Boolean);
      // Four numbers sit two by two, never three and an orphan.
      return el("div", { class: "tiles" + (kept.length === 4 ? " two" : "") }, kept);
    }

    function card(id, title, question, kids) {
      return el("section", { class: "card ins", id: id, "aria-labelledby": id + "-h" }, [
        el("h4", { id: id + "-h", text: title }),
        question ? el("p", { class: "q", text: question }) : null,
      ].concat(kids || []));
    }

    function sub(title, kids, id) {
      return el("div", { class: "isub", id: id || null }, [el("h5", { text: title })].concat(kids || []));
    }

    function rows(list) { return el("div", { class: "rows ilist" }, list); }

    /// A long list shows its first `n` (the ones that need acting on are
    /// put first by the caller) and a button for the rest: a 500-person
    /// organisation's page stays a page.
    function capped(list, n, draw, what) {
      var box = rows(list.slice(0, n).map(draw));
      if (list.length <= n) return box;
      var more = el("button", { class: "btn ghost sm more", type: "button", text: "Show all " + list.length + " " + what });
      var foot = el("div", { class: "row2" }, [more]);
      more.addEventListener("click", function () {
        list.slice(n).forEach(function (x) { box.appendChild(draw(x)); });
        foot.remove();
      });
      return el("div", {}, [box, foot]);
    }
    function row(main, small, right) {
      return el("div", { class: "rw flat" }, [
        el("div", { class: "t" }, [el("b", {}, [].concat(main)), small ? el("small", { text: small }) : null]),
        right ? el("div", { class: "ra" }, [].concat(right)) : null,
      ]);
    }
    function none(text) { return el("p", { class: "inone", text: text }); }

    /// A column chart with its numbers as text beside it. `items`:
    /// [{label, value}] oldest first; a value may be "<3".
    function chart(o) {
      var items = o.items || [];
      var nums = items.map(function (x) { return x.value === FEWER ? 2 : Number(x.value) || 0; });
      var max = Math.max.apply(null, [1].concat(nums));
      var few = items.some(function (x) { return x.value === FEWER; });
      var exact = items.filter(function (x) { return x.value !== FEWER; }).map(function (x) { return Number(x.value) || 0; });
      var top = exact.length ? Math.max.apply(null, exact) : 0;
      var last = items.length ? items[items.length - 1] : null;
      var said = o.what + ", " + o.span + ": most on one " + o.unit + " " + numSaid(top) + "; " + (o.lastWord || "the last") + " " + (last ? numSaid(last.value) : "0") + ".";
      var empty = !top && !few;
      var bars = el("div", { class: "bars" + (empty ? " zero" : ""), role: "img", "aria-label": said, "data-empty": "Nobody yet in this time." });
      bars.appendChild(el("span", { class: "top", "aria-hidden": "true", text: String(max === 1 && !top ? 1 : max) }));
      items.forEach(function (x) {
        var v = x.value === FEWER ? 2 : Number(x.value) || 0;
        var h = v ? Math.max(4, Math.round((v / max) * 100)) : 0;
        bars.appendChild(el("span", {
          class: "b" + (x.value === FEWER ? " few" : "") + (v ? "" : " z"),
          style: "height:" + h + "%",
          title: x.label + ": " + peopleSaid(x.value),
        }));
      });
      var table = el("table", {}, [
        el("thead", {}, [el("tr", {}, [el("th", { text: o.unitHead || "Day" }), el("th", { text: "People" })])]),
        el("tbody", {}, items.slice().reverse().map(function (x) {
          return el("tr", {}, [el("td", { text: x.label }), el("td", { text: numSaid(x.value) })]);
        })),
      ]);
      return el("figure", { class: "chart" }, [
        el("figcaption", { class: "ch" }, [el("span", { text: o.title }), el("span", { class: "rng", text: o.span })]),
        bars,
        el("div", { class: "axis", "aria-hidden": "true" }, [el("span", { text: items.length ? items[0].label : "" }), el("span", { text: last ? last.label : "" })]),
        el("p", { class: "cap", text: "Most on one " + o.unit + ": " + numSaid(top) + " · " + (o.lastWord || "Last") + ": " + (last ? numSaid(last.value) : "0") + (few ? " · Pale bars are fewer than 3 people, not shown exactly." : "") }),
        el("details", { class: "nums" }, [el("summary", { text: "The numbers" }), table]),
      ]);
    }

    function dailyItems(list) { return (list || []).map(function (d) { return { label: isoDay(d.day), value: d.people }; }); }
    function weekItems(list) { return (list || []).map(function (w) { return { label: isoDay(w.from) + " to " + isoDay(w.to), value: w.people }; }); }

    function meter(part, whole) {
      var pct = whole ? Math.min(100, Math.round((part / whole) * 100)) : 0;
      return el("div", { class: "meter", role: "img", "aria-label": pct + "%" }, [el("i", { style: "width:" + pct + "%" })]);
    }

    function mailTo(principal, subject) {
      var w = K.who(principal);
      if (w.kind !== "email") return null;
      return el("a", { class: "btn ghost sm", href: "mailto:" + w.display + "?subject=" + encodeURIComponent(subject), text: "Ask the owner", "aria-label": "Email " + w.display });
    }

    // ----------------------------------------------------- the sections

    function orgSection(o, body) {
      var now = body.now;
      var out = [];
      var a = o.adoption;
      var name = nameOf();

      // Adoption: is the organisation getting value?
      var adopt = [];
      if (!a.apps) {
        adopt.push(el("div", { class: "empty" }, [
          el("b", { text: name + " has no apps yet" }),
          el("p", { text: "Release one, a ready app or a .krate you have, and this shows who opens it every day." }),
          el("div", { class: "row2 center" }, [el("button", { class: "btn dark sm", type: "button", text: "Release an app", onclick: function () { D.show("releases"); } })]),
        ]));
      } else {
        adopt.push(tiles([
          tile({ label: "Active in the last 7 days", value: String(a.active_week), of: "of " + a.members, means: "people in " + name + " who opened one of its apps", act: go("people", "Invite your team") }),
          tile(monthTile(a, o.people.inactive || [])),
          tile({ label: "Apps in use", value: String(a.apps_used), of: "of " + a.apps, means: "apps someone opened in the last 30 days", act: go("apps", "See every app") }),
        ]));
        adopt.push(chart({ title: "People who opened an app, each day", span: "last 30 days", what: "People who opened an app each day", unit: "day", lastWord: "Today", items: dailyItems(a.daily) }));
        adopt.push(chart({ title: "People who opened an app, each week", span: "last 12 weeks", what: "People who opened an app each week", unit: "week", unitHead: "Week", lastWord: "This week", items: weekItems(a.trend) }));
        var topMax = a.top.length ? a.top[0].people : 1;
        adopt.push(sub("Most used, last 30 days", a.top.length
          ? [el("ol", { class: "toplist" }, a.top.map(function (t) {
              return el("li", {}, [
                el("span", { class: "nm", text: t.name }),
                el("span", { class: "ct", text: peopleSaid(t.people) }),
                meter(t.people, topMax),
              ]);
            })), el("div", { class: "row2" }, [go("apps", "See every app")])]
          : [none("Nobody opened an app in the last 30 days.")]));
        if (a.unused.length) {
          adopt.push(sub("Nobody opened these in 30 days", [
            el("p", { class: "q", text: "Withdraw what nobody needs, or ask its owner whether it is still wanted." }),
            capped(a.unused, 8, function (u) {
              var owner = u.owner ? K.who(u.owner.principal).label : "nobody";
              return row([u.name + " " + (u.version || "")], "Released " + K.day(u.released_at) + " · owned by " + owner, [
                mailTo(u.owner && u.owner.principal, "Is " + u.name + " still needed?") || go("apps", "See its owner"),
                go("releases", "Withdraw", "Withdraw " + u.name + " in Releases"),
              ]);
            }, "apps"),
          ], "ins-unused"));
        }
      }
      out.push(card("ins-adoption", "Adoption", "Is " + name + " getting value from its apps?", adopt));

      // People.
      var p = o.people;
      var company = p.company_sign_in;
      var roleLine = ["owner", "admin", "developer", "reviewer", "recipient"].filter(function (r) { return p.by_role[r]; }).map(function (r) {
        return plural(p.by_role[r], (K.ROLE_NAME[r] || r).toLowerCase(), (K.ROLE_NAME[r] || r).toLowerCase() + "s");
      }).join(", ");
      var sign = [["github", "GitHub"], ["email", "an email address"], ["company", "company sign-in"]].filter(function (k) { return p.by_sign_in[k[0]]; }).map(function (k) {
        return p.by_sign_in[k[0]] + " with " + k[1];
      }).join(", ");
      var ppl = [tiles([
        tile({ label: "Not seen for 30 days", value: String(p.inactive.length), means: "no app opened and no visit here: people you may want to remove", act: p.inactive.length ? jump("ins-inactive", "Review them") : null, warn: p.inactive.length > 0 }),
        company
          ? tile({ label: "Without company sign-in", value: String(p.without_company_sign_in.length), means: "people who sign in some other way than " + company.domain, act: go("settings", "Company sign-in settings") })
          : tile({ label: "Company sign-in", value: "Off", means: "people sign in with GitHub or an email address; company sign-in adds and removes them for you", act: go("settings", "Set up company sign-in") }),
        tile({ label: "Invitations waiting", value: String(p.invites.length), means: "sent and not yet accepted", act: go("people", "Manage invitations") }),
      ]), el("p", { class: "cap", text: "By role: " + (roleLine || "nobody") + "." + (sign ? " Signing in: " + sign + "." : "") })];
      ppl.push(sub("Not seen for 30 days", p.inactive.length
        ? [capped(p.inactive, 8, function (m) { return inactiveRow(m, now); }, "people")]
        : [none("Everyone in " + name + " was here in the last 30 days.")], "ins-inactive"));
      if (company && p.without_company_sign_in.length) {
        ppl.push(sub("Not signing in with " + company.domain, [capped(p.without_company_sign_in, 8, function (m) {
          return row([K.who(m.principal).label], (K.ROLE_NAME[m.role] || m.role));
        }, "people")]));
      }
      if (p.invites.length) {
        ppl.push(sub("Invitations waiting", [capped(p.invites, 6, function (i) {
          return row([K.who(i.principal).label], "Invited as " + (K.ROLE_NAME[i.role] || i.role).toLowerCase() + " by " + K.who(i.invited_by.principal).label + " · runs out " + K.day(i.expires_at));
        }, "invitations")]));
      }
      out.push(card("ins-people", "People", "Who is here, and who has stopped coming?", ppl));

      // Security.
      var s = o.security;
      var holding = s.devices.filter(function (d) { return d.holds_current; }).length;
      var silent = s.devices.filter(function (d) { return d.silent; }).length;
      var sec = [el("p", { class: "why", text: s.two_person
        ? "Every release of " + name + " is signed by " + name + ", and a second person approves it before anyone can open it."
        : "Every release of " + name + " is signed by " + name + ". Until a second person can approve, you release your own apps yourself." })];
      sec.push(tiles([
        tile({ label: "Computers with access", value: String(holding), means: "computers that can open " + name + "'s encrypted team data" + (silent ? "; " + plural(silent, "has", "have") + " been silent for 60 days" : ""), act: jump("ins-devices", "See them"), warn: silent > 0 }),
        tile({ label: "People who can send apps", value: String(s.publishers.length), means: "owners and developers", act: go("people", "Change roles") }),
        s.two_person
          ? tile({ label: "Second approval", value: "On", means: s.approvers < 2 ? "only " + plural(s.approvers, "person", "people") + " can approve, so a sender cannot always get one" : plural(s.approvers, "person", "people") + " can approve a release", act: go("people", s.approvers < 2 ? "Invite a reviewer" : "See who"), warn: s.approvers < 2 })
          : tile({ label: "Second approval", value: "Not yet", means: "nobody else can approve, so " + (s.solo_releases ? plural(s.solo_releases, "version was", "versions were") + " released by the person who sent it" : "you release your own apps") + "; it turns on when a reviewer joins", act: go("people", "Invite a reviewer"), warn: true }),
        tile({ label: "Refused in the last 7 days", value: String(s.refusals.week_total), means: s.refusals.spike ? "more than twice the usual " + Math.round(s.refusals.weekly_average_before) + " a week; the reasons are below" : "attempts the hub said no to; about " + Math.round(s.refusals.weekly_average_before) + " a week before", act: go("activity", "See the activity"), warn: s.refusals.spike }),
      ]));
      sec.push(sub("Computers with access", s.devices.length
        ? [capped(s.devices.slice().sort(function (x, y) { return Number(y.silent) - Number(x.silent) || Number(x.holds_current) - Number(y.holds_current); }), 8, function (d) {
            var seen = d.last_seen ? "last seen " + when(d.last_seen, now) : "not seen since it was added on " + K.day(d.added_at);
            return row([d.name || "A computer", d.silent ? el("span", { class: "tag warn", text: "Silent 60+ days" }) : null], K.who(d.principal).label + " · " + seen + (d.holds_current ? "" : " · waiting for another computer to share access"),
              d.silent ? [go("people", "Open People", "Open People to decide about " + K.who(d.principal).label)] : null);
          }, "computers"),
          el("details", { class: "nums" }, [el("summary", { text: "Details" }), el("small", { text: s.key ? "These computers hold version " + s.key.version + " of " + name + "'s organisation key" + (s.key.state === "stale" ? "; someone was removed since, so the next member computer online replaces it" : "") + "." : name + " has no organisation key yet." })])]
        : [none("No computer holds access yet. The first owner's computer makes it when Krate runs there signed in to " + name + ".")], "ins-devices"));
      sec.push(sub("Who can send apps", [capped(s.publishers.slice().sort(function (x, y) { return (y.last_sent_at || 0) - (x.last_sent_at || 0); }), 8, function (x) {
        return row([K.who(x.principal).label], (K.ROLE_NAME[x.role] || x.role) + " since " + K.day(x.since) + (x.last_sent_at ? " · last sent " + when(x.last_sent_at, now) : " · has not sent one"));
      }, "people")]));
      sec.push(sub("Refused in the last 7 days", s.refusals.week.length
        ? [rows(s.refusals.week.map(function (r) { return row([why(r.reason)], plural(r.n, "time", "times")); }))]
        : [none("Nothing was refused this week.")]));
      if (s.withdrawn_offline.length) {
        sec.push(sub("Withdrawn, still open offline", [
          el("p", { class: "q", text: "A computer that stays offline keeps what it has until its offline access runs out. Nothing can call it back sooner." }),
          rows(s.withdrawn_offline.map(function (w) {
            return row([w.name + " " + (w.version || "")], "Withdrawn " + K.day(w.withdrawn_at) + " · " + peopleSaid(w.people) + " can still open it offline until " + K.day(w.until));
          })),
        ]));
      }
      sec.push(sub("Sign-in, directory and activity export", [rows([
        health(s.sign_in.company ? "ok" : s.sign_in.domain_pending ? "warn" : "off", "Company sign-in",
          s.sign_in.company ? "On for " + s.sign_in.company.domain + (s.sign_in.company.enforced ? ", required for everyone there" : "") : s.sign_in.domain_pending ? "Waiting for the domain to be proved" : "Not set up", "settings"),
        health(!s.directory ? "off" : s.directory.week.failed || s.directory.week.refused ? "warn" : "ok", "Company directory",
          !s.directory ? "Not connected" : (s.directory.last_used_at ? "Last synced " + since(s.directory.last_used_at, now) + " ago" : "Never synced") + " · this week " + s.directory.week.ok + " worked, " + s.directory.week.failed + " failed" + (s.directory.week.refused ? ", " + s.directory.week.refused + " with a revoked token" : ""), "settings"),
        health(!s.audit_export ? "off" : s.audit_export.given_up || s.audit_export.last_error ? "warn" : "ok", "Activity export",
          !s.audit_export ? "Not set up" : plural(s.audit_export.delivered, "event", "events") + " delivered to " + s.audit_export.host + (s.audit_export.pending ? " · " + s.audit_export.pending + " waiting" : "") + (s.audit_export.given_up ? " · " + s.audit_export.given_up + " given up" : "") + (s.audit_export.last_error ? " · last problem: " + s.audit_export.last_error : ""), "settings"),
      ])]));
      out.push(card("ins-security", "Security", "Is " + name + " safe?", sec));

      // Releases.
      var r = o.releases;
      out.push(card("ins-releases", "Releases", "How fast do new versions reach people?", [tiles([
        tile({ label: "Approved, last 30 days", value: String(r.approved_month), means: "new versions people can open", act: go("releases", "See the releases") }),
        tile({ label: "Typical wait, last 90 days", value: r.median_wait_seconds == null ? "None yet" : waited(r.median_wait_seconds), means: r.approvals_counted ? "the middle of the last " + plural(r.approvals_counted, "approval", "approvals") : "no approvals in 90 days" }),
        tile({ label: "Waiting now", value: String(r.waiting), means: r.waiting ? "the oldest for " + since(r.oldest_waiting_since, now) : "nothing is waiting for a second person", act: r.waiting ? go("waiting", "Review them") : null, warn: r.waiting > 0 && now - r.oldest_waiting_since > 2 * 86400 }),
        tile({ label: "Taken back, last 30 days", value: String(r.withdrawn_month), of: "withdrawn", means: r.rollbacks_month ? "and " + plural(r.rollbacks_month, "rollback", "rollbacks") + " to an older version" : "and no rollbacks to an older version", act: go("releases", "See the versions") }),
      ])]));

      // Cost.
      var c = o.cost;
      var perHead = tile({ label: "Cost per active person", value: "Working it out", means: "the last invoice, over the " + plural(c.active_month, "person", "people") + " who opened an app in 30 days", act: go("billing", "See invoices") });
      out.push(card("ins-cost", "Cost", "What does " + name + " pay for what it uses?", [tiles([
        tile({ label: "Seats in use", value: String(c.seats_used), of: c.seats_paid ? "of " + c.seats_paid + " paid" : "", means: "owners, admins, developers and reviewers; people who only use apps are free" + (c.seats_paid ? "" : " · " + c.free_seats + " seats are free"), act: go("billing", "Open billing") }),
        perHead,
      ])]));
      // The amount is Billing's: the hub's Stripe invoices, read once.
      K.api("GET", path("/billing")).then(function (res) {
        var inv = res.ok && res.body && Array.isArray(res.body.invoices) ? res.body.invoices.filter(function (i) { return typeof i.amount === "number" && i.currency; })[0] : null;
        var val = perHead.children[1], means = perHead.children[2];
        if (inv && c.active_month) {
          val.textContent = K.money(Math.round(inv.amount / c.active_month), inv.currency);
          means.textContent = "the invoice of " + K.day(inv.at) + ", " + K.money(inv.amount, inv.currency) + ", over the " + plural(c.active_month, "person", "people") + " who opened an app in 30 days";
        } else {
          val.textContent = "Not yet";
          means.textContent = inv ? "nobody opened an app in the last 30 days" : "shown once " + name + " has a paid invoice; " + plural(c.active_month, "person", "people") + " opened an app in 30 days";
        }
      });
      return out;
    }

    function health(state, label, words, tab) {
      var tag = { ok: ["ok", "Working"], warn: ["warn", "Needs a look"], off: ["", "Off"] }[state];
      return row([label, " ", el("span", { class: "tag " + tag[0], text: tag[1] })], words, [go(tab, state === "off" ? "Set up" : "Settings", label + " settings")]);
    }

    function inactiveRow(m, now) {
      var w = K.who(m.principal);
      var last = m.last_active ? "last opened an app on " + isoDay(m.last_active) : "nothing since joining on " + K.day(m.since);
      var right = null;
      var me = D.S.me ? { principal: D.S.me.principal, role: D.S.me.role } : null;
      var owners = (D.S.members || []).filter(function (x) { return x.role === "owner" && (x.state || "active") === "active"; }).length;
      var acts = me ? K.memberActions(me, { principal: m.principal, role: m.role }, owners) : null;
      if (acts && acts.remove && !acts.removeWhy && me.principal !== m.principal) {
        right = el("button", { class: "btn quiet sm", type: "button", text: "Remove", "aria-label": "Remove " + w.label, onclick: function () { remove(m); } });
      }
      return row([w.label], (K.ROLE_NAME[m.role] || m.role) + " · " + last, right);
    }

    function remove(m) {
      var w = K.who(m.principal);
      var key = K.key();
      D.confirmSheet({
        title: "Remove " + w.label + " from " + nameOf() + "?",
        body: "Their access ends the next time they open an app. What they sent or approved stays on record, and you can invite them again.",
        yes: "Remove " + w.label,
        busy: "Removing " + w.label,
        ctx: { what: "remove " + w.label, whoCan: "an owner, or an admin for people who are not owners" },
        act: function () { return K.api("DELETE", path("/members/" + encodeURIComponent(w.principal)), null, { key: key }); },
        done: function () {
          D.toast(w.label + " was removed. Their next open is refused.");
          D.show("insights");
        },
      });
    }

    function developerSection(dv, body) {
      var now = body.now;
      if (!dv.apps.length) {
        return [card("ins-mine", "Your apps", "How is my app doing?", [el("div", { class: "empty" }, [
          el("b", { text: "You have not sent an app to " + nameOf() + " yet" }),
          el("p", { text: "Send one and this shows how many people open it each day, which versions they are on, and how long approval took." }),
          el("div", { class: "row2 center" }, [el("button", { class: "btn dark sm", type: "button", text: "Release an app", onclick: function () { D.show("releases"); } })]),
        ])])];
      }
      return dv.apps.map(function (a, i) {
        var kids = [];
        var refused = a.refusals.reduce(function (t, r) { return t + r.n; }, 0);
        var link = K.hub() + "/orgs/" + slug + "/apps/" + encodeURIComponent(a.app_id) + "/current";
        var copyBtn = el("button", { class: "act", type: "button", text: "Copy its link", onclick: function () { K.copy(link, copyBtn); } });
        kids.push(tiles([
          tile({ label: "People in the last 7 days", value: numSaid(a.people_week), means: a.released ? "who opened it, of everyone it is for" : "it has no approved version yet", act: a.released ? copyBtn : go("waiting", "See what is waiting") }),
          tile({ label: "Refused, last 30 days", value: String(refused), means: refused ? "times the hub said no to someone opening it; the reasons are below" : "nobody was turned away", act: refused ? go("releases", "Send a new version") : null }),
          a.data ? tile({ label: "Team data", value: bytes(a.data.bytes), of: "of " + bytes(a.data.quota), means: plural(a.data.writes_week, "change", "changes") + " in the last 7 days; encrypted, so not even Krate can read it" }) : null,
        ]));
        if (a.data) kids.push(meter(a.data.bytes, a.data.quota));
        var opened = (a.daily || []).concat(a.trend || []).some(function (d) { return d.people === FEWER || d.people > 0; });
        if (!a.released && !opened) {
          kids.push(none("Nobody can open it until a second person approves it, so there is nothing to count yet."));
        } else {
          kids.push(chart({ title: "People who opened it, each day", span: "last 30 days", what: "People who opened " + a.name + " each day", unit: "day", lastWord: "Today", items: dailyItems(a.daily) }));
          kids.push(chart({ title: "People who opened it, each week", span: "last 12 weeks", what: "People who opened " + a.name + " each week", unit: "week", unitHead: "Week", lastWord: "This week", items: weekItems(a.trend) }));
        }
        // The version that opens now first, then those still in use.
        var versions = (a.versions || []).slice().sort(function (x, y) { return Number(y.current) - Number(x.current); });
        if (a.released || versions.length) kids.push(sub("Versions in use, last 7 days", versions.length
          ? [rows(versions.map(function (v) {
              return row([v.version, v.current ? el("span", { class: "tag ok", text: "Opens now" }) : null], peopleSaid(v.people) + (v.current ? "" : " still on it; they get the new one the next time they open it online"));
            }))]
          : [none("Nobody opened it in the last 7 days.")]));
        if (a.refusals.length) {
          kids.push(sub("Why people were turned away", [rows(a.refusals.map(function (r) { return row([why(r.reason)], plural(r.n, "time", "times") + " in 30 days"); }))]));
        }
        if (a.connections.length) {
          kids.push(sub("Company connections it uses", [rows(a.connections.map(function (c) {
            return row([c.name], plural(c.uses_week, "use", "uses") + " in the last 7 days, by all of " + nameOf() + "'s apps; the app never holds the credential");
          }))]));
        }
        if (a.waits.length) {
          kids.push(sub("How long your versions waited for approval", [rows(a.waits.map(function (w) {
            return row([w.version || "A version"], w.state === "waiting" ? "Waiting for " + waited(w.waited_seconds) + " so far" : "Waited " + waited(w.waited_seconds) + (w.state === "withdrawn" ? " · since withdrawn" : ""), w.state === "waiting" ? [go("waiting", "See it")] : null);
          }))]));
        }
        return card("ins-app-" + i, a.name, (a.version ? "Version " + a.version + " opens now. " : "") + "How is it doing?", kids);
      });
    }

    function reviewerSection(rv, body) {
      var kids = [tiles([
        tile({ label: "Waiting for approval", value: String(rv.queue.length), means: rv.queue.length ? "the oldest for " + waited(rv.queue[0].waited_seconds) : "nothing needs you now", act: rv.queue.length ? go("waiting", "Review them") : null, warn: rv.queue.length > 0 }),
        tile({ label: "Approved by you, last 30 days", value: String(rv.approved_by_you_month), means: "of " + plural(rv.approved_month, "approval", "approvals") + " in " + nameOf() }),
        tile({ label: "Typical wait, last 30 days", value: rv.median_wait_seconds_month == null ? "None yet" : waited(rv.median_wait_seconds_month), means: "from sent to approved" }),
      ])];
      kids.push(sub("Waiting now", rv.queue.length
        ? [capped(rv.queue, 8, function (q) {
            var asks = q.added.filter(function (x) { return x.asks; }).map(function (x) { return x.words; });
            var change = q.first
              ? "First version" + (asks.length ? ": it can " + asks.join("; ") : ", asks for nothing beyond what every app may do")
              : (q.added.length ? "New: it can " + q.added.map(function (x) { return x.words; }).join("; ") : "Asks for nothing new") + (q.removed.length ? " · No longer: " + q.removed.map(function (x) { return x.words; }).join("; ") : "");
            var by = q.sent_by ? " · sent by " + K.who(q.sent_by.principal).label : "";
            return row([q.name + " " + (q.version || ""), !q.first && q.added.some(function (x) { return x.asks; }) ? el("span", { class: "tag warn", text: "Asks for more" }) : null],
              "Waiting " + waited(q.waited_seconds) + by + " · " + change + (q.yours ? " · you sent it, so someone else approves it" : ""),
              q.yours ? null : [go("waiting", "Review", "Review " + q.name)]);
          }, "versions")]
        : [none("Nothing is waiting. When someone sends a version, it shows here with what it asks for.")]));
      return [card("ins-review", "Approvals", "What needs me?", kids)];
    }

    function yoursSection(y, body) {
      var now = body.now;
      var kids = [];
      kids.push(sub("Your computers", y.devices.length
        ? [rows(y.devices.map(function (d) {
            return row([d.name || "A computer"], "Added " + K.day(d.added_at) + " · " + (d.last_seen ? "last seen " + when(d.last_seen, now) : "not seen since") + (d.holds_current ? "" : " · waiting for another computer to share access"));
          }))]
        : [none("Your computers show here once Krate runs on them signed in to " + nameOf() + ".")]));
      kids.push(sub("Apps you opened, last 30 days", y.apps.length
        ? [rows(y.apps.map(function (a) { return row([a.name], "On " + plural(a.days, "day", "days") + " · last " + (a.last === body.today ? "today" : isoDay(a.last))); }))]
        : [none("You have not opened any of " + nameOf() + "'s apps in 30 days."), el("div", { class: "row2" }, [go("releases", "See the apps you can open")])]));
      return [card("ins-yours", "Yours", "Only you see this part.", kids)];
    }

    var GROUP = { org: "The organisation", developer: "Your apps", reviewer: "Approvals", yours: "Yours" };

    function subtitle(role) {
      var name = nameOf();
      if (role === "owner" || role === "admin") return "Is " + name + " getting value from its apps, and is it safe? Counts only: Krate never sees what anyone does inside an app.";
      if (role === "developer") return "How your apps are doing. Counts only, never what anyone does inside them.";
      if (role === "reviewer") return "What needs you, and what you approved.";
      return "Your computers and the apps you used.";
    }

    // ---------------------------------------------------------------- pane

    /// Which day the numbers count by (F-319), and where to change it.
    function zoneLine(body) {
      if (!body.time_zone) return null;
      var role = D.roleOf();
      var kids = [el("span", { text: "Days run midnight to midnight, " + K.zoneName(body.time_zone) + "." })];
      if (role === "owner" || role === "admin") kids.push(go("settings", "Change", "Change " + nameOf() + "'s time zone"));
      return el("p", { class: "sub zone-line" }, kids);
    }

    function pane(p) {
      D.head(p, "Insights", subtitle(D.roleOf()));
      var wait = el("div", { class: "card ins-wait", role: "status", "aria-live": "polite" }, [
        el("p", { text: "Counting " + nameOf() + "'s numbers" }),
        el("div", { class: "bars ghost", "aria-hidden": "true" }, [10, 40, 25, 60, 35, 80, 50, 30, 70, 45, 55, 20].map(function (h) { return el("span", { class: "b", style: "height:" + h + "%" }); })),
      ]);
      p.appendChild(wait);
      K.api("GET", path("/insights")).then(function (res) {
        wait.remove();
        if (!res.ok) {
          var box = el("div", { class: "card" }, [K.refusalBlock(D.words(res, { what: "see the insights", whoCan: "members" }), "warn")]);
          if (res.body && res.body.error !== "not-a-member" && res.body.error !== "removed") {
            box.appendChild(el("div", { class: "row2" }, [el("button", { class: "btn ghost sm", type: "button", text: "Try again", onclick: function () { pane(p); } })]));
          }
          p.appendChild(box);
          return;
        }
        draw(p, res.body);
        try { root.performance.mark("krate-insights-drawn"); } catch (e) {}
      });
    }

    function draw(p, body) {
      var sections = body.sections || [];
      var zl = zoneLine(body);
      if (zl) p.appendChild(zl);
      // Partial: counting began after the organisation did (a store older
      // than the insights), so the first days of a window are missing.
      var born = (D.S.org && D.S.org.created_at) || 0;
      if (body.since && body.now - body.since < 30 * 86400 && body.since - born > 86400) {
        p.appendChild(el("div", { class: "call", role: "note" }, [el("div", { text: "Counting since " + K.day(body.since) + ". Days before that were not counted, so they show as 0." })]));
      }
      // The sections, pinned at the top of the panel while it scrolls, the
      // one in view marked.
      var nav = null;
      if (sections.length > 1) {
        nav = el("nav", { class: "inav", "aria-label": "Insights sections" }, sections.map(function (s, i) {
          return el("a", { class: "chip", href: "#ins-g-" + s, text: GROUP[s], "aria-current": i === 0 ? "true" : null });
        }));
        p.appendChild(nav);
      }
      var marks = [];
      sections.forEach(function (s) {
        var make = { org: orgSection, developer: developerSection, reviewer: reviewerSection, yours: yoursSection }[s];
        if (!make || !body[s]) return;
        var cards = make(body[s], body);
        // A section of one card already named for it needs no heading over it.
        var h = cards.length === 1 ? cards[0].querySelector("h4") : null;
        var named = Boolean(h) && h.textContent === GROUP[s];
        if (sections.length > 1) {
          if (named) { cards[0].id = "ins-g-" + s; marks.push(cards[0]); }
          else { var g = el("h3", { class: "igrp", id: "ins-g-" + s, text: GROUP[s] }); p.appendChild(g); marks.push(g); }
        }
        // Side by side on a wide screen, the first of a section across.
        var grid = el("div", { class: "igrid" + (cards.length > 1 ? " many" : "") });
        cards.forEach(function (c) { grid.appendChild(c); });
        p.appendChild(grid);
      });
      if (nav && marks.length && typeof IntersectionObserver === "function") {
        var scroller = document.querySelector && document.querySelector(".cmainwrap");
        var io = new IntersectionObserver(function (es) {
          es.forEach(function (e) {
            if (!e.isIntersecting) return;
            var id = "#" + e.target.id;
            Array.prototype.slice.call(nav.children).forEach(function (a) { a.setAttribute("aria-current", a.getAttribute("href") === id ? "true" : "false"); });
          });
        }, { root: scroller && getComputedStyle(scroller).overflowY === "auto" ? scroller : null, rootMargin: "0px 0px -70% 0px" });
        marks.forEach(function (m) { io.observe(m); });
      }
    }

    return { pane: pane };
  };
})(typeof window !== "undefined" ? window : globalThis);
