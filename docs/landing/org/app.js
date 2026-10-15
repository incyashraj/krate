/* One app's page (/org/?o=<slug>&tab=apps&app=<id>): everything about one
 * app in one place, in tabs -- Overview, Versions, Usage, Permissions, Team
 * data and Settings. It reads what the hub already keeps about the app:
 * its releases (release control), its record (the inventory), the versions
 * waiting or declined (candidates) and its numbers (Insights), each only
 * where the person's role may see it.
 */
(function (root) {
  "use strict";
  var K = root.KrateOrg;
  var el = K.el;
  var MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

  root.KrateOrgApp = function (D) {
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    function nameOf() { return (D.S.org && D.S.org.name) || slug; }
    function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
    function isoDay(iso) { var p = String(iso || "").split("-"); return p.length === 3 ? Number(p[2]) + " " + MONTHS[Number(p[1]) - 1] : ""; }
    function bytes(n) { n = Number(n) || 0; if (n < 1024) return n + " B"; if (n < 1048576) return (n / 1024).toFixed(1) + " KB"; return (n / 1048576).toFixed(1) + " MB"; }
    function num(v) { return v === "<3" ? 0 : Number(v) || 0; }
    var TABS = [["overview", "Overview"], ["versions", "Versions"], ["usage", "Usage"], ["permissions", "Permissions"], ["data", "Team data"], ["settings", "Settings"]];

    function pane(p, appId, tab) {
      var role = D.roleOf();
      var calls = [
        K.api("GET", path("/apps")),
        K.can(role, "inventory.view") ? K.api("GET", path("/inventory")) : Promise.resolve({ ok: false }),
        K.api("GET", path("/candidates")),
        K.api("GET", path("/insights")),
      ];
      Promise.all(calls).then(function (got) {
        var rel = got[0].ok ? ((got[0].body && got[0].body.apps) || []).filter(function (a) { return a.app_id === appId; })[0] : null;
        var inv = got[1].ok ? ((got[1].body && got[1].body.apps) || []).filter(function (a) { return a.app_id === appId; })[0] : null;
        var cands = got[2].ok ? (got[2].body || []).filter(function (c) { return (c.app_id || c.name) === appId; }) : [];
        var use = got[3].ok && got[3].body.developer ? (got[3].body.developer.apps || []).filter(function (a) { return a.app_id === appId; })[0] : null;
        cands.forEach(function (c) { D.S.names[c.digest] = K.appName(c); });
        var a = { id: appId, rel: rel, inv: inv, cands: cands, use: use };
        a.name = (rel && rel.name) || (inv && inv.name) || (cands[0] && cands[0].name) || appId;
        var cur = rel && rel.releases ? rel.releases.filter(function (r) { return r.digest === rel.current; })[0] : null;
        a.cur = cur;
        a.waiting = cands.filter(function (c) { return c.state === "waiting-for-approval"; });
        if (!rel && !inv && !cands.length) {
          D.head(p, a.name, "");
          p.appendChild(el("div", { class: "card" }, [el("div", { class: "empty" }, [el("b", { text: "No such app here" }), el("p", { text: "It may have been sent under another name. Apps lists every app " + nameOf() + " has." })])]));
          return;
        }
        D.crumb(D.tabName(K.tabsFor(D.roleOf()).indexOf("apps") >= 0 ? "apps" : "myapps"), a.name);
        p.textContent = "";
        p.appendChild(header(a));
        var bar = el("div", { class: "atabs", role: "tablist", "aria-label": a.name });
        var body = el("div", { class: "abody" });
        tab = TABS.some(function (t) { return t[0] === tab; }) ? tab : "overview";
        TABS.forEach(function (t) {
          bar.appendChild(el("button", { class: "atab", type: "button", role: "tab", "aria-selected": t[0] === tab ? "true" : "false", text: t[1], onclick: function () { D.openApp(appId, t[0]); } }));
        });
        p.appendChild(bar);
        p.appendChild(body);
        ({ overview: overview, versions: versions, usage: usage, permissions: permissions, data: data, settings: settings })[tab](body, a);
      });
    }

    function header(a) {
      var cur = a.cur;
      var bits = [];
      if (cur) bits.push((cur.version || "") + " opens now");
      if (a.rel && a.rel.releases && a.rel.releases[0]) bits.push(D.R ? D.R.audienceWords(cur || a.rel.releases[0]).replace(/^For /, "for ") : "");
      if (a.inv && a.inv.owner) bits.push("owned by " + K.who(a.inv.owner.principal).label);
      if (a.inv && a.inv.built) bits.push("made with " + a.inv.built_words);
      var acts = [];
      if (cur && cur.url) {
        var copy = el("button", { class: "btn ghost sm", type: "button", text: "Copy link" });
        copy.addEventListener("click", function () { K.copy(cur.url, copy); });
        acts.push(copy);
        acts.push(el("a", { class: "btn dark sm", href: "/open/?a=" + cur.digest, text: "Open" }));
      }
      return el("div", { class: "ahead" }, [
        el("span", { class: "apptile lg", "aria-hidden": "true", text: String(a.name).slice(0, 1).toUpperCase() }),
        el("div", { class: "pt" }, [el("h2", { text: a.name }), el("p", { text: bits.filter(Boolean).join(" · ") || "Nothing released yet" })]),
        el("div", { class: "pa" }, acts),
      ]);
    }

    function tiles(list) {
      return el("div", { class: "hstats" }, list.map(function (t) {
        return el("div", { class: "hstat static" }, [el("span", { class: "hs-l", text: t[0] }), el("span", { class: "hs-v" }, [el("b", { text: t[1] })]), el("span", { class: "hs-d flat", text: t[2] })]);
      }));
    }

    function overview(b, a) {
      a.waiting.forEach(function (c) {
        // Someone who cannot approve it (its sender) reads what changed in full.
        if (K.tabsFor(D.roleOf()).indexOf("waiting") < 0 && D.waitCard) { b.appendChild(D.waitCard(c, false, false, Date.now() / 1000)); return; }
        var added = (c.changes && c.changes.against ? c.changes.added : []).filter(function (x) { return x.asks; });
        b.appendChild(el("div", { class: "call warn row-call" }, [
          el("div", { text: (c.version || "A new version") + " waits for approval" + (added.length ? ": it newly asks to " + added.map(function (x) { return x.words || x.cap; }).join(", ") + "." : ". It asks for nothing new.") + " Sent by " + K.who(c.uploaded_by).label + "." }),
          K.tabsFor(D.roleOf()).indexOf("waiting") >= 0 ? el("button", { class: "btn dark sm", type: "button", text: "Review", onclick: function () { D.show("waiting"); } }) : null,
        ]));
      });
      var u = a.use || {};
      var refused = (u.refusals || []).reduce(function (s, r) { return s + (Number(r.n) || 0); }, 0);
      b.appendChild(tiles([
        ["People, 7 days", u.people_week == null ? (a.inv && a.inv.people ? String(a.inv.people.month || 0) : "-") : String(num(u.people_week)), u.people_week == null ? "in 30 days" : "who opened it"],
        ["Versions", String(((a.rel && a.rel.releases) || []).length + a.waiting.length), a.waiting.length ? a.waiting.length + " waiting" : "released"],
        ["Refused, 30 days", u.refusals ? String(refused) : "-", refused ? "people turned away" : "nobody turned away"],
        ["Team data", u.data ? bytes(u.data.bytes) : "-", u.data ? "of " + bytes(u.data.quota) : "not used yet"],
      ]));
      var side = el("div", { class: "acol" }, [
        box("Who opens it", [
          el("p", { text: a.cur && D.R ? D.R.audienceWords(a.cur) : "Nobody yet: nothing of it is released." }),
          a.cur && a.cur.offline ? el("p", { class: "sub", text: K.offlineName ? K.offlineName(a.cur.offline) : a.cur.offline }) : null,
        ]),
        box("What it can do", [el("p", { text: a.inv && D.I ? D.I.canWords(a.inv) : "Its permissions are in the Permissions tab." })]),
        a.inv ? box("Made", [el("p", { text: (a.inv.built ? "With " + a.inv.built_words : "By hand") + (a.inv.built && a.inv.built.prompt ? ", from the request “" + a.inv.built.prompt + "”" : "") + ". Every version is signed by " + nameOf() + "." })]) : null,
      ]);
      var main = el("div", { class: "acol wide" }, [versionList(a, 4)]);
      b.appendChild(el("div", { class: "agrid" }, [main, side]));
    }
    function box(title, kids) { return el("section", { class: "card abox" }, [el("h3", { text: title })].concat(kids)); }

    /// Every version, newest first: released ones with their state, the
    /// one waiting, and those declined or withdrawn.
    function allVersions(a) {
      var list = [];
      ((a.rel && a.rel.releases) || []).forEach(function (r) { list.push({ v: r.version, at: r.at, note: r.note, who: r.solo ? "Released by its sender alone" : "Approved by " + K.who(r.approved_by).label, state: r.state === "withdrawn" ? "Withdrawn" : r.digest === (a.rel && a.rel.current) ? "Opens now" : "Released", tone: r.state === "withdrawn" ? "bad" : r.digest === (a.rel && a.rel.current) ? "ok" : "" }); });
      a.cands.forEach(function (c) {
        if (c.state === "waiting-for-approval") list.push({ v: c.version, at: c.uploaded_at, note: c.note, who: "Sent by " + K.who(c.uploaded_by).label, state: "Waiting", tone: "warn" });
        else if (c.state === "declined") list.push({ v: c.version, at: c.declined_at || c.uploaded_at, note: c.decline_reason ? "Declined: " + c.decline_reason : c.note, who: "Declined by " + K.who(c.declined_by).label, state: "Declined", tone: "bad" });
      });
      return list.sort(function (x, y) { return (y.at || 0) - (x.at || 0); });
    }
    function versionList(a, max) {
      var list = allVersions(a);
      var rows = list.slice(0, max || list.length).map(function (v) {
        return el("div", { class: "vrow" }, [
          el("span", { class: "mono", text: v.v || "" }),
          el("span", { class: "pr-t" }, [el("span", { text: v.note || "No notes" }), el("small", { text: v.who + " · " + K.day(v.at) })]),
          el("span", { class: "pdot " + v.tone }, [el("i", { "aria-hidden": "true" }), v.state]),
        ]);
      });
      return el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "Versions" }), max && list.length > max ? el("button", { class: "btn link sm", type: "button", text: "All " + list.length, onclick: function () { D.openApp(a.id, "versions"); } }) : null])].concat(rows.length ? rows : [el("div", { class: "empty" }, [el("b", { text: "No versions yet" })])]));
    }

    function versions(b, a) {
      b.appendChild(versionList(a));
      // Who opens each one, which opens now, and taking one back: the
      // release controls, where the person may use them.
      if (a.rel && D.R) {
        b.appendChild(el("h3", { class: "igrp", text: "Released versions" }));
        b.appendChild(D.R.appCard(a.rel, b));
      }
    }

    function usage(b, a) {
      var u = a.use;
      if (!u) { b.appendChild(el("div", { class: "card" }, [el("div", { class: "empty" }, [el("b", { text: "Usage is for its makers and the people who run " + nameOf() }), el("p", { text: "Insights has the organisation's numbers." })])])); return; }
      var max = Math.max(1, Math.max.apply(null, (u.daily || []).map(function (d) { return num(d.people); })));
      b.appendChild(el("section", { class: "card hcard" }, [
        el("div", { class: "hc-h" }, [el("h3", { text: "People who opened it, each day" }), el("p", { class: "sub", text: "Last 30 days" })]),
        el("div", { class: "hchart" }, [el("div", { class: "hc-plot" }, [el("span", { class: "hc-max", text: String(max) }), el("div", { class: "hc-bars", style: "--n:" + (u.daily || []).length }, (u.daily || []).map(function (d) {
          return el("span", { class: "hc-b", title: isoDay(d.day) + ": " + d.people }, [el("span", { style: "height:" + Math.max(num(d.people) / max * 100, num(d.people) ? 4 : 1.5) + "%" })]);
        }))]), el("div", { class: "hc-axis" }, [el("span", { text: u.daily && u.daily[0] ? isoDay(u.daily[0].day) : "" }), el("span", { text: "Today" })])]),
      ]));
      b.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "Versions in use, last 7 days" })])].concat((u.versions || []).map(function (v) {
        return el("div", { class: "prow" }, [el("span", { class: "pr-t" }, [el("b", { class: "mono", text: v.version }), el("small", { text: v.current ? "Opens now" : "An older version" })]), el("span", { class: "pr-s", text: v.people === "<3" ? "fewer than 3" : plural(num(v.people), "person", "people") })]);
      }))));
      if ((u.refusals || []).length) {
        b.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "Turned away, last 30 days" })])].concat(u.refusals.map(function (r) {
          return el("div", { class: "prow" }, [el("span", { class: "pr-t" }, [el("b", { text: (K.refusal(r.reason, { org: nameOf() }).title) || r.reason })]), el("span", { class: "pr-s", text: String(r.n) })]);
        }))));
      }
    }

    function permissions(b, a) {
      var caps = (a.inv && a.inv.capabilities) || (a.waiting[0] && a.waiting[0].capabilities) || [];
      var rows = caps.map(function (c) {
        return el("div", { class: "prow" }, [el("span", { class: "pr-t" }, [el("b", { text: c.words || c.cap }), el("small", { text: (c.required ? "Needed" : "Optional") + (c.rationale ? " · “" + c.rationale + "”" : "") })])]);
      });
      b.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "What it can do" })])].concat(rows.length ? rows : [el("p", { class: "sub", style: "padding:0 0 14px", text: a.inv && D.I ? D.I.canWords(a.inv) : "Its permissions show to the people who may see the organisation's apps." })])));
      var conns = (a.inv && a.inv.connections) || [];
      if (conns.length) b.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "Company connections it reaches" })])].concat(conns.map(function (n) { return el("div", { class: "prow" }, [el("span", { class: "pr-t" }, [el("b", { class: "mono", text: n }), el("small", { text: "The app never holds the credential" })])]); }))));
    }

    function data(b, a) {
      var d = a.use && a.use.data;
      if (!d) { b.appendChild(el("div", { class: "card" }, [el("div", { class: "empty" }, [el("b", { text: "No team data yet" }), el("p", { text: "When the app keeps data shared with the team, its size shows here: encrypted end to end, so only " + nameOf() + "'s computers can read it." })])])); return; }
      b.appendChild(tiles([
        ["Size", bytes(d.bytes), "of " + bytes(d.quota)],
        ["Writes, 7 days", String(d.writes_week || 0), "changes saved"],
        ["Encrypted", "End to end", "not even Krate can read it"],
      ]));
    }

    function settings(b, a) {
      if (a.inv && D.I && K.can(D.roleOf(), "app.owner")) {
        b.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "Owner" })]), D.I.ownerRow(a.inv, b)]));
      }
      if (a.rel && D.R) {
        b.appendChild(el("h3", { class: "igrp", text: "Who opens it, offline access, and taking versions back" }));
        b.appendChild(D.R.appCard(a.rel, b));
      }
      b.appendChild(el("section", { class: "card pnl" }, [el("div", { class: "pnl-h" }, [el("h3", { text: "Technical details" })]),
        el("dl", { class: "cr-kv", style: "padding:0 0 14px" }, [el("dt", { text: "App id" }), el("dd", { class: "dg", text: a.id })].concat(a.cur ? [el("dt", { text: "Fingerprint" }), el("dd", { class: "dg", text: a.cur.digest })] : [])),
      ]));
    }

    return { pane: pane };
  };
})(typeof window !== "undefined" ? window : globalThis);
