/* /org/?o=<slug>: one organisation's releases, approvals, people, activity
 * and settings (IC-947, CONTRACT.md Addendum 1 and 2).
 *
 * Every tab asks the organisation's store when it opens; nothing here is
 * trusted from an earlier answer. The role matrix only decides which
 * controls are drawn, so nobody is offered a click that is sure to be
 * refused. When the hub refuses anyway, the person reads why, in words.
 */
(function () {
  "use strict";
  var K = window.KrateOrg;
  var el = K.el;
  var $ = function (id) { return document.getElementById(id); };
  var q = new URLSearchParams(location.search);
  var slug = (q.get("o") || "").toLowerCase();
  var HERE = "/org/?o=" + encodeURIComponent(slug);
  var S = { org: null, me: null, members: [], tab: q.get("tab") || "", waiting: 0, names: {} };
  // The approval email's link names one version: ?tab=waiting&c=<digest>.
  var deepC = /^[0-9a-f]{64}$/.test(String(q.get("c") || "").toLowerCase()) ? String(q.get("c")).toLowerCase() : "";

  // The sidebar: each section's name, its group, and its icon (16px, drawn
  // with the stroke the rest of Krate uses).
  var ICON = {
    home: '<path d="M2.6 7.2L8 2.8l5.4 4.4V13a.9.9 0 0 1-.9.9H9.6V10H6.4v3.9H3.5a.9.9 0 0 1-.9-.9z"/>',
    releases: '<path d="M8 2.5v7.5M5 7l3 3 3-3"/><path d="M3 11v1.5A1.5 1.5 0 0 0 4.5 14h7a1.5 1.5 0 0 0 1.5-1.5V11"/>',
    apps: '<rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1.1"/><rect x="9" y="2.5" width="4.5" height="4.5" rx="1.1"/><rect x="2.5" y="9" width="4.5" height="4.5" rx="1.1"/><rect x="9" y="9" width="4.5" height="4.5" rx="1.1"/>',
    waiting: '<circle cx="8" cy="8" r="5.6"/><path d="M8 5v3.2l2 1.3"/>',
    insights: '<path d="M2.5 13.5h11"/><path d="M4.5 11V8M8 11V4.5M11.5 11V6.5"/>',
    people: '<circle cx="6" cy="5.5" r="2.3"/><path d="M2 13c.4-2.2 2-3.5 4-3.5s3.6 1.3 4 3.5"/><path d="M10.5 3.4a2.2 2.2 0 0 1 0 4.2M11.6 9.7c1.3.4 2.2 1.6 2.4 3.3"/>',
    activity: '<path d="M2 8.5h2.6l1.6-4 3.2 8 1.6-4H14"/>',
    billing: '<rect x="2" y="3.5" width="12" height="9" rx="1.6"/><path d="M2 6.5h12M4.5 10h2.5"/>',
    myapps: '<rect x="2.5" y="2.5" width="11" height="11" rx="2.4"/><path d="M5.5 8h5M8 5.5v5"/>',
    build: '<path d="M8 3v10M3 8h10"/>',
    computers: '<rect x="2" y="3" width="12" height="8" rx="1.4"/><path d="M5.5 13.5h5M8 11v2.5"/>',
    security: '<path d="M8 1.8l5 2v3.6c0 3.2-2.2 5.6-5 6.8-2.8-1.2-5-3.6-5-6.8V3.8z"/><path d="M5.8 8l1.6 1.6 3-3.2"/>',
    connections: '<path d="M6.5 9.5l3-3"/><path d="M5 7.5L3.6 8.9a2.5 2.5 0 0 0 3.5 3.5L8.5 11M11 8.5l1.4-1.4a2.5 2.5 0 0 0-3.5-3.5L7.5 5"/>',
    requests: '<path d="M3 3.5h10v7H7l-3 2.5v-2.5H3z"/><path d="M5.5 6.5h5M5.5 8.5h3"/>',
    help: '<circle cx="8" cy="8" r="5.8"/><path d="M6.4 6.3a1.7 1.7 0 1 1 2.4 1.6c-.5.3-.8.6-.8 1.2M8 11.2v.1"/>',
    settings: '<circle cx="8" cy="8" r="2.1"/><path d="M8 1.8v1.7M8 12.5v1.7M1.8 8h1.7M12.5 8h1.7M3.6 3.6l1.2 1.2M11.2 11.2l1.2 1.2M3.6 12.4l1.2-1.2M11.2 4.8l1.2-1.2"/>',
  };
  // Each section: its name for this role, and the group it sits in.
  var TAB = {
    home: { long: function (r) { return r === "owner" || r === "admin" ? "Overview" : r === "developer" || r === "reviewer" ? "Home" : "My apps"; }, group: "" },
    myapps: { long: "My apps", group: "" },
    build: { long: "New app", group: "" },
    apps: { long: "Apps", group: "" },
    waiting: { long: function (r) { return r === "reviewer" ? "Queue" : "Approvals"; }, group: "" },
    releases: { long: "Company apps", group: "Use" },
    people: { long: function (r) { return r === "owner" || r === "admin" ? "People and groups" : "People"; }, group: "Access" },
    computers: { long: "Computers", group: "Access" },
    security: { long: "Security", group: "Access" },
    connections: { long: "Connections", group: "Organisation" },
    insights: { long: "Insights", group: "Organisation" },
    activity: { long: "Activity", group: "Organisation" },
    billing: { long: "Billing", group: "Organisation" },
    settings: { long: "Settings", group: "Organisation" },
    requests: { long: "Requests", group: "" },
    help: { long: "Help", group: "" },
  };
  function tabName(id) { var t = TAB[id]; if (!t) return id; return typeof t.long === "function" ? t.long(roleOf()) : t.long; }
  var GROUPS = ["", "Use", "Access", "Organisation"];
  function icon(id) {
    var svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("fill", "none");
    svg.setAttribute("stroke", "currentColor");
    svg.setAttribute("stroke-width", "1.4");
    svg.setAttribute("stroke-linecap", "round");
    svg.setAttribute("stroke-linejoin", "round");
    svg.setAttribute("aria-hidden", "true");
    svg.innerHTML = ICON[id] || "";
    return svg;
  }
  // Filled in at the bottom: the Addendum 3 parts (parts.js).
  var P = null;
  function tabList() { var t = K.tabsFor(roleOf()); return P ? P.tabs(t) : t; }

  // --------------------------------------------------------------- frame
  function signedOut() {
    $("in").hidden = true;
    $("stop").hidden = true;
    var box = $("outChoices");
    box.textContent = "";
    box.appendChild(K.signInChoices(HERE));
    $("out").hidden = false;
  }

  /// A refusal that ends the page: not a member, removed, no such org.
  function stop(reason, message) {
    if (reason === "not-signed-in") { K.forget(); signedOut(); return; }
    $("in").hidden = true;
    var box = $("stop");
    box.textContent = "";
    var w = K.refusal(reason, { org: slug, slug: slug, here: HERE, message: message });
    box.appendChild(el("div", { class: "panel signedout" }, [
      el("img", { class: "mk", src: "/krate-mark-3d-96.png", alt: "" }),
      el("h1", { text: w.title }),
      el("p", { text: w.body }),
      w.action ? el("div", { class: "stack" }, [el("a", { class: "btn dark", href: w.action.href, text: w.action.label })]) : null,
    ]));
    box.hidden = false;
    document.title = w.title;
  }

  /// A refusal inside a pane or a row: `ctx.what` is what was tried.
  function words(res, ctx) {
    ctx = ctx || {};
    ctx.org = ctx.org || (S.org && S.org.name) || slug;
    ctx.slug = slug;
    ctx.here = HERE + "&tab=" + S.tab;
    ctx.message = res.body && res.body.message;
    // What a withdrawn version and an app for other groups say (IC-949, R).
    if (res.body && res.body.withdrawn) ctx.withdrawn = res.body.withdrawn;
    if (res.body && res.body.groups) ctx.groups = res.body.groups;
    // A version with a pilot group first says which version is theirs.
    if (res.body && res.body.for_you) ctx.for_you = res.body.for_you;
    return K.refusal(res.body ? res.body.error : "hub-error", ctx);
  }

  function toast(text) {
    var t = $("toast");
    t.textContent = "";
    t.appendChild(el("div", { class: "call ok", role: "status" }, [el("div", { text: text })]));
    t.hidden = false;
    clearTimeout(toast.t);
    toast.t = setTimeout(function () { t.hidden = true; }, 6000);
  }

  function me() { return K.who(S.me); }
  function isMe(p) { return K.who(p).principal === me().principal; }
  function roleOf() { return S.me ? S.me.role : ""; }

  function drawTabs() {
    var bar = $("tabs");
    bar.textContent = "";
    var list = tabList();
    GROUPS.forEach(function (g) {
      var mine = list.filter(function (id) { return (TAB[id] ? TAB[id].group : "") === g; });
      if (!mine.length) return;
      if (g) bar.appendChild(el("p", { class: "cs-label", text: g }));
      mine.forEach(drawTab);
    });
    function drawTab(id) {
      var t = TAB[id];
      var b = el("button", {
        class: "tab", type: "button", role: "tab", id: "tab-" + id,
        "aria-label": tabName(id) + (id === "waiting" && S.waiting ? ", " + S.waiting + " waiting" : "") + (id === "people" && S.pubWaiting ? ", " + S.pubWaiting + " waiting to publish" : ""),
        "aria-selected": (S.under || S.tab) === id ? "true" : "false",
        onclick: function () { show(id); },
      }, [
        icon(id),
        el("span", { class: "lb", text: tabName(id) }),
        id === "waiting" && S.waiting ? el("span", { class: "n", text: String(S.waiting) }) : null,
        // Publishers an owner's computer has not approved yet (IC-958 A).
        id === "people" && S.pubWaiting ? el("span", { class: "n", text: String(S.pubWaiting) }) : null,
      ]);
      bar.appendChild(b);
    }
  }

  // The sidebar is a drawer on a narrow screen.
  function drawer(open) {
    var shell = $("in"), opener = $("csOpen");
    if (!shell || !opener || !shell.classList) return;
    var was = shell.classList.contains("drawer");
    shell.classList.toggle("drawer", open);
    opener.setAttribute("aria-expanded", open ? "true" : "false");
    if (open) $("csClose").focus();
    else if (was) opener.focus();
  }
  if ($("csOpen")) {
    $("csOpen").addEventListener("click", function () { drawer(true); });
    $("csClose").addEventListener("click", function () { drawer(false); });
    $("csScrim").addEventListener("click", function () { drawer(false); });
    if (typeof document.addEventListener === "function") document.addEventListener("keydown", function (e) { if (e.key === "Escape") drawer(false); });
  }
  function setText(id, text) { var n = $(id); if (n) n.textContent = text; }
  /// The panel's top line: the organisation, then where you are in it.
  function drawCrumb(here, deeper) {
    var c = $("crumb");
    if (!c) return;
    c.textContent = "";
    c.appendChild(document.createTextNode((S.org && S.org.name) || slug));
    c.appendChild(el("i", { text: "/" }));
    if (deeper) {
      c.appendChild(el("a", { href: HERE + "&tab=" + (S.under || S.tab), text: here, style: "color:inherit", onclick: function (e) { if (e && e.preventDefault) e.preventDefault(); show(S.under || S.tab); } }));
      c.appendChild(el("i", { text: "/" }));
      c.appendChild(el("b", { text: deeper }));
    } else {
      c.appendChild(el("b", { text: here }));
    }
  }

  function show(id, opts) {
    var tabs = tabList();
    // A developer's own app opens its page, under My apps, without the Apps list.
    var ownApp = id === "apps" && tabs.indexOf("apps") < 0 && tabs.indexOf("myapps") >= 0 && ((opts && opts.app) || (firstLoad && q.get("app")));
    if (tabs.indexOf(id) < 0 && !ownApp) id = tabs[0];
    S.opts = opts || {};
    S.under = ownApp ? "myapps" : "";
    // Settings and Billing open in the settings window, over the section
    // that was open; the page's pane moves into it and back.
    var dlg = $("setDlg");
    var inWindow = (id === "settings" || id === "billing") && dlg && typeof dlg.showModal === "function";
    if (inWindow) { openSettings(id); return; }
    if (dlg && isOpen(dlg)) closeSettings(true);
    S.tab = id;
    // One version on its own, from the approval email or the bell.
    S.focus = id === "waiting" ? (S.opts.c !== undefined ? S.opts.c : deepC) : "";
    deepC = "";
    try { history.replaceState(null, "", HERE + "&tab=" + id + (S.focus ? "&c=" + S.focus : "")); } catch (e) {}
    drawTabs();
    setText("cbarTab", tabName(id));
    drawCrumb(tabName(id));
    drawer(false);
    // The panel scrolls by itself on a wide screen, the page on a narrow one.
    var wrap = document.querySelector && document.querySelector(".cmainwrap");
    if (wrap) wrap.scrollTop = 0;
    if (window.scrollTo) window.scrollTo(0, 0);
    if (P) P.banner($("banner"));
    // Getting started (IC-958), under the Dashboard heading, for those who
    // run it. One box, kept here: head() moves it into the pane, and a pane
    // that redraws would otherwise take it out of the page for good.
    var start = startBox;
    if (Setup && id === "home" && S.org && S.org.state !== "closed") Setup.checklist(start, { S: S, show: show, refresh: refresh, compact: true });
    else { start.hidden = true; start.textContent = ""; }
    if (Pub) Pub.notice($("pubNote"), id);
    var pane = $("pane");
    pane.textContent = "";
    pane.setAttribute("aria-labelledby", "tab-" + id);
    pane.appendChild(el("p", { class: "loading", text: "Loading" }));
    railHome();
    // One app's page lives under Apps: ?tab=apps&app=<id>.
    var appId = id === "apps" ? (S.opts.app || (S.opts.app === undefined && firstLoad ? q.get("app") : "")) : "";
    firstLoad = false;
    if (appId && A) {
      S.app = appId;
      try { history.replaceState(null, "", HERE + "&tab=apps&app=" + encodeURIComponent(appId) + (S.opts.appTab ? "&view=" + S.opts.appTab : "")); } catch (e) {}
      A.pane(pane, appId, S.opts.appTab || (q.get("view") || ""));
      return;
    }
    S.app = "";
    var G_ = G || {};
    var draw = ({ home: home, releases: releases, apps: apps, insights: insights, waiting: waiting, people: people, activity: activity, settings: settings, billing: P && P.panes.billing,
       computers: G_.computers, security: G_.security, connections: G_.connections, build: G_.build, myapps: G_.myapps, requests: G_.requests, help: G_.help })[id];
    if (draw) { draw(pane); return; }
    head(pane, tabName(id), "");
    pane.appendChild(el("div", { class: "card" }, [el("p", { text: "This page did not load completely. Reload it." })]));
  }
  var firstLoad = true;
  /// Open one app's page, at one of its tabs.
  function openApp(id, tab) { show("apps", { app: id, appTab: tab || "" }); }

  // ------------------------------------------------------ settings window
  function openSettings(id) {
    var dlg = $("setDlg"), pane = $("pane"), slot = $("setSlot");
    if (!isOpen(dlg)) {
      S.back = S.tab && S.tab !== "settings" && S.tab !== "billing" ? S.tab : tabList()[0];
      // What was behind stays drawn, still, while the pane is away.
      var shadow = pane.cloneNode ? pane.cloneNode(true) : null;
      if (shadow) {
        // A copy for the eye only: no ids, or a lookup would find the copy.
        shadow.querySelectorAll("[id]").forEach(function (n) { n.removeAttribute("id"); });
        shadow.id = "paneShadow"; shadow.setAttribute("aria-hidden", "true"); shadow.setAttribute("inert", "");
        pane.parentNode.insertBefore(shadow, pane);
      }
      slot.appendChild(pane);
      dlg.showModal();
      // Start reading at the top, not on the close button.
      if (slot.setAttribute) slot.setAttribute("tabindex", "-1");
      if (slot.focus) { try { slot.focus({ preventScroll: true }); } catch (e) { slot.focus(); } }
    }
    S.tab = id;
    try { history.replaceState(null, "", HERE + "&tab=" + id); } catch (e) {}
    drawTabs();
    pane.textContent = "";
    pane.appendChild(el("p", { class: "loading", text: "Loading" }));
    (id === "billing" ? P.panes.billing : settings)(pane);
    settingsNav(id);
  }
  function isOpen(dlg) { return !!(dlg && dlg.getAttribute && dlg.getAttribute("open") !== null && dlg.getAttribute("open") !== undefined); }
  function closeSettings(quiet) {
    var dlg = $("setDlg"), pane = $("pane"), shadow = $("paneShadow");
    if (shadow && shadow.parentNode) { shadow.parentNode.insertBefore(pane, shadow); shadow.remove(); }
    else { var inner = document.querySelector && document.querySelector(".cmain .inner"); if (inner) inner.appendChild(pane); }
    if (isOpen(dlg)) dlg.close();
    if (!quiet) show(S.back || tabList()[0]);
  }
  /// The window's left side: a row per section of the open page (its cards'
  /// headings, kept up to date as they arrive), and Billing beside Settings.
  function settingsNav(id) {
    var nav = $("setNav"), pane = $("pane");
    if (!nav) return;
    // One list whichever part is open: Settings' sections (remembered from
    // the last time Settings drew them) and Billing.
    function draw() {
      nav.textContent = "";
      var tabs = tabList();
      var group = null;
      var sections = [];
      if (id === "settings") {
        Array.prototype.slice.call(pane.children || []).forEach(function (c) {
          var h = c.querySelector && c.querySelector("h3");
          if (!h || !c.classList || !c.classList.contains("card")) return;
          sections.push({ title: h.textContent, card: c });
        });
        S.navSections = sections.map(function (x) { return x.title; });
      } else if (tabs.indexOf("settings") >= 0) {
        sections = (S.navSections || ["Organisation"]).map(function (t) { return { title: t, card: null }; });
      }
      // In group order, each group once, keeping the page's order inside it.
      var ORDER = ["Organisation", "Connected to", "Data", "More"];
      sections = sections.map(function (x, i) { return { x: x, i: i, g: ORDER.indexOf(navName(x.title)[1]) }; })
        .sort(function (a, b) { return a.g - b.g || a.i - b.i; }).map(function (y) { return y.x; });
      sections.forEach(function (x) {
        var named = navName(x.title);
        if (named[1] !== group) { group = named[1]; nav.appendChild(el("p", { class: "sd-group", text: group })); }
        var nb = el("button", { type: "button", text: named[0], onclick: function () {
          if (!x.card) { S.navGo = x.title; show("settings"); return; }
          Array.prototype.slice.call(nav.querySelectorAll("button")).forEach(function (b) { b.removeAttribute("aria-current"); });
          this.setAttribute("aria-current", "true");
          if (x.card.scrollIntoView) x.card.scrollIntoView({ block: "start" });
        } });
        nb.card = x.card;
        nav.appendChild(nb);
      });
      spy();
      // Arrived from Billing for one section: go to it once it is drawn.
      if (id === "settings" && S.navGo) {
        var want = sections.filter(function (x) { return x.title === S.navGo; })[0];
        if (want) { S.navGo = null; if (want.card.scrollIntoView) want.card.scrollIntoView({ block: "start" }); }
      }
      if (tabs.indexOf("billing") >= 0) {
        nav.appendChild(el("p", { class: "sd-group", text: "Plan" }));
        var bill = el("button", { type: "button", text: "Billing", onclick: function () { show("billing"); } });
        if (id === "billing") bill.setAttribute("aria-current", "true");
        nav.appendChild(bill);
      }
    }
    // The section in view is the one marked, as the window scrolls.
    function spy() {
      var slot = $("setSlot");
      var btns = Array.prototype.slice.call(nav.querySelectorAll("button")).filter(function (b) { return b.card; });
      if (!btns.length || !slot) return;
      var top = (slot.scrollTop || 0) + 60, at = btns[0];
      btns.forEach(function (b) { if ((b.card.offsetTop || 0) <= top) at = b; });
      if (slot.scrollTop + slot.clientHeight >= slot.scrollHeight - 4) at = btns[btns.length - 1];
      btns.forEach(function (b) { if (b === at) b.setAttribute("aria-current", "true"); else b.removeAttribute("aria-current"); });
    }
    var slotEl = $("setSlot");
    if (slotEl && !slotEl.spying && slotEl.addEventListener) { slotEl.spying = true; slotEl.addEventListener("scroll", function () { if (S.navSpy) S.navSpy(); }, { passive: true }); }
    S.navSpy = spy;
    draw();
    // Sections arrive after the window opens: Settings redraws this list
    // when it has drawn its own, and an observer catches the later ones.
    S.navRedraw = draw;
    if (typeof MutationObserver === "function") {
      if (settingsNav.mo) settingsNav.mo.disconnect();
      settingsNav.mo = new MutationObserver(draw);
      settingsNav.mo.observe(pane, { childList: true });
    }
  }
  /// A Settings section's short name and its group, from its card heading.
  var NAV = [
    [/^Organisation$/, "General", "Organisation"],
    [/signature$/, "Signature", "Organisation"],
    [/^Send an app/, "Sending apps", "Organisation"],
    [/^Who approves/, "Approvals", "Organisation"],
    [/^Approval rules/, "Approval rules", "Organisation"],
    [/^Offline/, "Offline access", "Organisation"],
    [/^Company connections/, "Connections", "Connected to"],
    [/^Company sign-in by SAML/, "SAML sign-in", "Connected to"],
    [/^Company sign-in/, "Company sign-in", "Connected to"],
    [/SCIM/, "Directory", "Connected to"],
    [/activity to your logs/, "Activity export", "Connected to"],
    [/^Slack and Teams/, "Slack and Teams", "Connected to"],
    [/^Export/, "Download everything", "Data"],
    [/^Close /, "Close the organisation", "Data"],
  ];
  function navName(text) {
    for (var i = 0; i < NAV.length; i++) if (NAV[i][0].test(text)) return [NAV[i][1], NAV[i][2]];
    return [text, "More"];
  }
  if ($("setClose") && $("setDlg").addEventListener) {
    $("setClose").addEventListener("click", function () { closeSettings(); });
    // Escape closes a dialog by itself; the pane must still come home.
    $("setDlg").addEventListener("cancel", function (e) { e.preventDefault(); closeSettings(); });
  }

  /// A section's header: its title, what it is for, and at most one or
  /// two actions, `acts` being [{label, onclick, dark}].
  function head(pane, title, sub, acts) {
    pane.textContent = "";
    pane.appendChild(el("div", { class: "phd" }, [
      el("div", { class: "pt" }, [el("h2", { text: title }), sub ? el("p", { text: sub }) : null]),
      acts && acts.length ? el("div", { class: "pa" }, acts.map(function (a) {
        return el("button", { class: "btn sm " + (a.dark ? "dark" : "ghost"), type: "button", text: a.label, onclick: a.onclick });
      })) : null,
    ]));
  }
  /// Getting started (IC-958) goes where the Dashboard puts it: under the
  /// numbers. Moved, not copied: it is the one box show() fills.
  function placeStart(pane) { if (S.tab === "home" && startBox) pane.appendChild(startBox); }

  // ---------------------------------------------------------- right panel
  /// What was clicked, in detail, on the right: `nodes` under `title`.
  /// Beside the work on a wide screen, over it on a narrow one.
  function inspect(title, nodes) { rail(title, nodes, true); }
  function rail(title, nodes, detail) {
    var body = $("crBody");
    if (!body) return;
    setText("crTitle", title);
    body.textContent = "";
    (Array.isArray(nodes) ? nodes : [nodes]).forEach(function (n) { if (n) body.appendChild(n); });
    var back = $("crBack");
    if (back) back.hidden = !detail;
    var shell = $("in");
    if (shell && shell.classList) shell.classList.toggle("inspect", !!detail);
  }
  function uninspect() { railHome(); }
  /// With nothing picked: the quick actions and what changed lately.
  function railHome() {
    var role = roleOf();
    var acts = [];
    var act = function (label, svg, go) {
      return el("button", { class: "cr-act", type: "button", onclick: go }, [icon(svg), el("span", { text: label })]);
    };
    if (K.can(role, "candidate.publish")) acts.push(act("Release an app", "releases", releaseFlow));
    if (K.can(role, "invite.other")) acts.push(act("Invite people", "people", function () { show("people", { invite: true }); }));
    if (S.waiting && K.can(role, "candidate.approve")) acts.push(act("Review " + S.waiting + " waiting", "waiting", function () { show("waiting"); }));
    if (role && role !== "recipient") acts.push(act("Settings", "settings", function () { show("settings"); }));
    var feed = el("div", { class: "cr-feed" }, [el("p", { class: "cr-sub", text: "Loading" })]);
    // On Activity itself the list is the page: the panel says the week.
    if (S.tab === "activity") {
      rail((S.org && S.org.name) || slug, [
        acts.length ? el("div", { class: "cr-sec" }, [el("h3", { text: "Quick actions" }), el("div", { class: "cr-acts" }, acts)]) : null,
        el("div", { class: "cr-sec" }, [el("h3", { text: "This week" }), feed]),
      ], false);
      K.api("GET", "/orgs/" + slug + "/audit").then(function (res) {
        feed.textContent = "";
        var weekAgo = Date.now() / 1000 - 7 * 86400;
        var ev = res.ok ? (res.body.events || []).filter(function (e) { return e.at >= weekAgo; }) : [];
        var denied = ev.filter(function (e) { return e.result === "denied"; }).length;
        feed.appendChild(el("div", { class: "cr-list" }, [
          el("div", {}, [el("b", { text: "Entries" }), el("small", { text: String(ev.length) })]),
          el("div", {}, [el("b", { text: "Refused" }), el("small", { text: String(denied) })]),
          el("div", {}, [el("b", { text: "Chain" }), el("small", { text: res.ok && res.body.chain_ok ? "intact" : "check" })]),
        ]));
      });
      return;
    }
    rail((S.org && S.org.name) || slug, [
      acts.length ? el("div", { class: "cr-sec" }, [el("h3", { text: "Quick actions" }), el("div", { class: "cr-acts" }, acts)]) : null,
      el("div", { class: "cr-sec" }, [el("h3", { text: K.can(role, "audit.read") ? "Lately" : "Your apps" }), feed]),
    ], false);
    if (K.can(role, "audit.read")) {
      // App names first, so an entry says "Team Notes", not its digest.
      var named = K.api("GET", "/orgs/" + slug + "/candidates").then(function (res) {
        if (res.ok && Array.isArray(res.body)) res.body.forEach(function (c) { S.names[c.digest] = K.appName(c); });
      }, function () {});
      Promise.all([named, K.api("GET", "/orgs/" + slug + "/audit")]).then(function (both) {
        var res = both[1];
        feed.textContent = "";
        var ev = res.ok ? (res.body.events || []).slice(-8).reverse() : [];
        if (!ev.length) { feed.appendChild(el("p", { class: "cr-sub", text: res.ok ? "Nothing yet." : "Activity did not load." })); return; }
        ev.forEach(function (e) {
          var row = el("button", { class: "cr-ev " + (e.result || ""), type: "button", style: "text-align:left", onclick: function () { inspectEvent(e); } }, [
            el("i", { "aria-hidden": "true" }), el("span", { text: K.eventWords(e, S.names) }), el("small", { text: K.moment(e.at) }),
          ]);
          feed.appendChild(row);
        });
        feed.appendChild(el("button", { class: "btn link sm", type: "button", text: "All activity", onclick: function () { show("activity"); } }));
      });
    } else {
      K.api("GET", "/orgs/" + slug + "/releases").then(function (res) {
        feed.textContent = "";
        var list = res.ok && Array.isArray(res.body) ? res.body.slice(0, 6) : [];
        if (!list.length) { feed.appendChild(el("p", { class: "cr-sub", text: "Nothing released yet." })); return; }
        feed.appendChild(el("div", { class: "cr-list" }, list.map(function (r) {
          return el("a", { href: K.joinUrl ? r.url : r.url }, [el("b", { text: K.appName(r) }), el("small", { text: r.version || "" })]);
        })));
      });
    }
  }
  /// One audit entry, in full.
  function inspectEvent(e) {
    inspect(e.result === "denied" ? "Refused" : "What happened", [
      el("p", { class: "cr-lead", text: K.eventWords(e, S.names) }),
      el("dl", { class: "cr-kv" }, [
        el("dt", { text: "When" }), el("dd", { text: K.moment(e.at) }),
        el("dt", { text: "Who" }), el("dd", { text: K.who(e.actor).label }),
        el("dt", { text: "Result" }), el("dd", { text: e.result === "denied" ? "Refused" : "Allowed" }),
        e.reason ? el("dt", { text: "Why" }) : null, e.reason ? el("dd", { text: (K.refusal(e.reason, { org: (S.org && S.org.name) || slug }).title) || e.reason }) : null,
      ]),
      K.can(roleOf(), "audit.read") ? el("button", { class: "btn ghost sm", type: "button", text: "All activity", onclick: function () { show("activity"); } }) : null,
    ]);
  }
  if ($("crBack")) {
    $("crBack").addEventListener("click", uninspect);
    $("crClose").addEventListener("click", function () { var sh = $("in"); if (sh && sh.classList) sh.classList.remove("inspect"); });
    $("crScrim").addEventListener("click", function () { var sh = $("in"); if (sh && sh.classList) sh.classList.remove("inspect"); });
  }
  var startBox = $("start");

  function failed(pane, res, ctx) {
    var c = el("div", { class: "card" }, [K.refusalBlock(words(res, ctx))]);
    pane.appendChild(c);
  }

  // ------------------------------------------------------------- sheet
  /// The confirmation sheet. `act` runs on Yes and answers a hub result;
  /// a refusal stays on the sheet in words, a success closes it.
  function confirmSheet(opts) {
    var d = $("sheet");
    $("sheetH").textContent = opts.title;
    $("sheetP").textContent = opts.body;
    // Points under the sentence, one consequence each; `key` ones stand out.
    var pts = $("sheetPts");
    pts.textContent = "";
    (opts.points || []).forEach(function (pt) {
      if (pt) pts.appendChild(el("li", { class: pt.key ? "key" : "", text: pt.text || pt }));
    });
    pts.hidden = !pts.children.length;
    var dg = $("sheetDigest");
    dg.textContent = "";
    var box = $("sheetIn"), inp = $("sheetInput");
    box.hidden = !opts.input;
    inp.value = "";
    if (opts.input) { $("sheetInLabel").textContent = opts.input.label; inp.placeholder = opts.input.placeholder || ""; }
    if (opts.digest) {
      // The exact bytes' fingerprint, for whoever checks it, folded away
      // (K-1114: no sha256 on the default path).
      dg.appendChild(el("details", { class: "cr-tech" }, [
        el("summary", { text: "Technical details" }),
        el("span", { text: opts.digestLabel || "Fingerprint of these exact bytes" }),
        document.createTextNode(opts.digest),
      ]));
      dg.hidden = false;
    } else dg.hidden = true;
    var why = $("sheetWhy");
    why.hidden = true;
    why.textContent = "";
    var yes = $("sheetYes");
    yes.textContent = opts.yes;
    yes.disabled = false;
    yes.onclick = function () {
      // A check in the page first (a required reason): it keeps the sheet
      // open with Yes still there.
      var said = opts.check ? opts.check(inp.value) : null;
      if (said) {
        why.textContent = "";
        why.appendChild(K.refusalBlock(said, "warn"));
        why.hidden = false;
        inp.focus();
        return;
      }
      yes.disabled = true;
      yes.textContent = opts.busy || "Working";
      opts.act(inp.value).then(function (res) {
        yes.disabled = false;
        yes.textContent = opts.yes;
        if (res.ok) { d.close(); opts.done(res); return; }
        if (res.body.error === "not-signed-in") { d.close(); K.forget(); signedOut(); return; }
        why.appendChild(K.refusalBlock(words(res, opts.ctx), "bad"));
        why.hidden = false;
        yes.hidden = true;
      });
    };
    yes.hidden = false;
    $("sheetNo").textContent = opts.no || "Cancel";
    $("sheetNo").onclick = function () { d.close(); };
    if (typeof d.showModal === "function") d.showModal(); else d.setAttribute("open", "");
    if (opts.input) inp.focus();
  }

  // ---------------------------------------------------------- releases
  /// Releases by app, with the current version, history, withdraw, set
  /// current and audience (releases.js, IC-949 R); the flat list below is
  /// what a hub without those routes still answers.
  function releases(pane) {
    if (R) { R.pane(pane); return; }
    flatReleases(pane);
  }

  function flatReleases(pane) {
    K.api("GET", "/orgs/" + slug + "/releases").then(function (res) {
      head(pane, "Releases", "Approved apps. Everyone in " + (S.org.name || slug) + " can open them; a copied link alone opens nothing.");
      if (!res.ok) return failed(pane, res, { what: "see the releases" });
      var list = Array.isArray(res.body) ? res.body : [];
      var card = el("div", { class: "card rows" });
      pane.appendChild(card);
      if (!list.length) {
        emptyReleases(card);
        return;
      }
      list.forEach(function (r) {
        S.names[r.digest] = K.appName(r);
        var note = el("p", { class: "note", hidden: true });
        var offline = P ? P.releaseOffline(r, note) : null;
        var open = el("button", { class: "btn dark sm", type: "button", text: "Open in Krate" });
        var copyBtn = el("button", { class: "btn ghost sm", type: "button", text: "Copy link" });
        open.addEventListener("click", function () {
          open.disabled = true;
          open.textContent = "Getting it";
          var file = K.fileName(r.name);
          K.download(r.url, file).then(function (out) {
            open.disabled = false;
            open.textContent = "Open in Krate";
            note.textContent = "";
            if (out.ok) {
              note.appendChild(document.createTextNode("Saved as " + file + ". Double-click it to open it in Krate. No Krate yet? "));
              note.appendChild(el("a", { href: "/open/", text: "Get it here" }));
              note.appendChild(document.createTextNode("."));
            } else {
              var w = words({ body: out.body }, { what: "open it" });
              note.textContent = w.title + ". " + w.body;
            }
            note.hidden = false;
          });
        });
        copyBtn.addEventListener("click", function () { K.copy(r.url, copyBtn); });
        card.appendChild(el("div", { class: "rw flat" }, [
          el("div", { class: "t" }, [
            el("b", { text: K.appName(r) }),
            el("small", { text: r.solo
              ? "Released by its sender, " + K.who(r.uploaded_by).label + " (no second approver yet) · " + K.day(r.at)
              : "Sent by " + K.who(r.uploaded_by).label + " · approved by " + K.who(r.approved_by).label + " · " + K.day(r.at) }),
            note,
          ]),
          el("div", { class: "ra" }, [open, copyBtn, offline]),
        ]));
      });
    });
  }

  /// Nothing released yet: for whoever may send one, the first app right
  /// here (a ready app or their own .krate); for everyone else, who does.
  function emptyReleases(card) {
    var mayPublish = K.can(roleOf(), "candidate.publish") === "yes";
    // The owners and admins have it in Getting started just above.
    var inList = Boolean(Setup) && (roleOf() === "owner" || roleOf() === "admin") && !Setup.mem().hidden;
    if (mayPublish && Setup && !inList) {
      var box = el("div", { class: "empty-app" });
      card.appendChild(box);
      Setup.firstApp(box, { h: "h3", onDone: function () { refresh(); }, onWaiting: function () { show("waiting"); } });
      return;
    }
    card.appendChild(el("div", { class: "empty" }, [
      el("b", { text: "Nothing released yet" }),
      el("p", { text: mayPublish
        ? (inList ? "Release one from Getting started above, or send it" : "Send one") + " from Krate Studio with Publish to " + (S.org.name || slug) + "."
        : "When someone in " + (S.org.name || slug) + " releases an app, it appears here, ready to open." }),
    ]));
  }

  // ----------------------------------------------------------- waiting
  function countWaiting() {
    if (K.tabsFor(roleOf()).indexOf("waiting") < 0) return;
    K.api("GET", "/orgs/" + slug + "/candidates").then(function (res) {
      if (!res.ok || !Array.isArray(res.body)) return;
      res.body.forEach(function (c) { S.names[c.digest] = K.appName(c); });
      S.waiting = res.body.filter(function (c) { return c.state === "waiting-for-approval"; }).length;
      drawTabs();
      // The panel's quick actions offer the review once the count is known,
      // unless it is showing something the person picked.
      var sh = $("in");
      if (S.waiting && !(sh && sh.classList && sh.classList.contains("inspect")) && $("crBack") && $("crBack").hidden) railHome();
    });
  }

  function waiting(pane) {
    K.api("GET", "/orgs/" + slug + "/candidates").then(function (res) {
      head(pane, "Waiting for approval", "Apps sent to " + (S.org.name || slug) + " that nobody can open yet. A reviewer or an owner approves the exact bytes; nobody approves their own.");
      if (!res.ok) return failed(pane, res, { what: "see what is waiting", whoCan: "owners, admins, developers and reviewers" });
      var all = Array.isArray(res.body) ? res.body : [];
      all.forEach(function (c) { S.names[c.digest] = K.appName(c); });
      var list = all.filter(function (c) { return c.state === "waiting-for-approval"; });
      S.waiting = list.length;
      drawTabs();
      var approver = K.can(roleOf(), "candidate.approve") === "yes";
      var closing = S.org && S.org.state === "closing";
      if (S.focus && Ap) {
        Ap.focus(pane, S.focus, all, function (c) { return waitCard(c, approver, closing, Date.now() / 1000, { focus: true }); });
        return;
      }
      if (!list.length) {
        var none = el("div", { class: "card rows" });
        pane.appendChild(none);
        none.appendChild(el("div", { class: "empty" }, [
          el("b", { text: "Nothing is waiting" }),
          el("p", { text: "When someone sends an app from Releases or from Krate Studio, it waits here until a second person approves it." }),
        ]));
        decided(pane, all);
        return;
      }
      // While the organisation is closing nothing new is released
      // (Addendum 3 D), so Approve is not offered then.
      var now = Date.now() / 1000;
      // Oldest first: the one that has waited longest is the one to do.
      list.sort(function (x, y) { return (x.uploaded_at || 0) - (y.uploaded_at || 0); });
      var forMe = list.filter(function (c) { return approver && !isMe(c.uploaded_by) && !closing && !(c.approval && c.approval.you && !c.approval.you.can); });
      var others = list.filter(function (c) { return forMe.indexOf(c) < 0; });
      pane.appendChild(el("div", { class: "ptools" }, [el("p", { class: "sub", text: plural(list.length, "version is", "versions are") + " waiting · the oldest for " + age(now - list[0].uploaded_at) + (forMe.length ? " · " + forMe.length + " for you" : "") })]));
      function group(title, items) {
        if (!items.length) return;
        if (forMe.length && others.length) pane.appendChild(el("h3", { class: "igrp", text: title }));
        items.forEach(function (c) { pane.appendChild(waitCard(c, approver, closing, now)); });
      }
      group("Waiting for you", forMe);
      group("Waiting for someone else", others);
      decided(pane, all);
    });
  }
  function age(sec) {
    sec = Math.max(0, sec || 0);
    if (sec < 3600) return plural(Math.max(1, Math.round(sec / 60)), "minute", "minutes");
    if (sec < 2 * 86400) return plural(Math.round(sec / 3600), "hour", "hours");
    return plural(Math.round(sec / 86400), "day", "days");
  }
  /// One version waiting: what it is, how long it has waited, what it can
  /// do and what changed, and the one thing to do about it.
  function waitCard(c, approver, closing, now, opts) {
    opts = opts || {};
    var mine = isMe(c.uploaded_by);
    var acts = [], note = null, btn = null;
    // What the approval rules say about this person (org-approvals.js).
    var you = c.approval && c.approval.you;
    // What it can do, and what changed (IC-950). Built first so the
    // Approve button can wait on its ticks.
    var review = capsReview(c, approver && !mine && !closing);
    if (closing) {
      note = el("p", { class: "note", text: "Approvals stop while " + (S.org.name || slug) + " is closing. If an owner cancels the closing, this can be approved again." });
    } else if (approver && !mine && you && !you.can) {
      note = el("p", { class: "note", text: you.approved
        ? "You approved this. " + (c.approval.words || "It is released once the rest are in") + "."
        : "Your approval would not count: " + you.why + "." });
      acts.push(el("button", { class: "btn ghost sm", type: "button", text: "Decline", onclick: function () { declineBox.hidden = !declineBox.hidden; if (!declineBox.hidden && why.focus) why.focus(); } }));
    } else if (approver && !mine) {
      btn = el("button", {
        class: "btn dark sm", type: "button", text: "Approve",
        onclick: function () { approve(c, review.accepted()); },
      });
      acts.push(btn);
      acts.push(el("button", { class: "btn ghost sm", type: "button", text: "Decline", onclick: function () { declineBox.hidden = !declineBox.hidden; if (!declineBox.hidden && why.focus) why.focus(); } }));
    } else {
      note = el("p", { class: "note", text: mine
        ? "You sent this one, so someone else approves it."
        : "A reviewer or an owner approves this." });
    }
    acts.push(el("button", { class: "btn ghost sm", type: "button", text: "Details", onclick: function () { inspectCandidate(c); } }));
    // Declining says why, in a sentence the sender reads.
    var why = el("textarea", { class: "inp", rows: "2", placeholder: "What would make it fine", "aria-label": "Why, for " + K.who(c.uploaded_by).label });
    var whySay = el("small", { class: "lc-say", role: "status" });
    var declineBox = el("div", { class: "wdecline", hidden: true }, [
      el("label", { class: "sub", text: "Tell " + K.who(c.uploaded_by).label + " why. The version stays in the app's history." }), why,
      el("div", { class: "row2" }, [el("button", { class: "btn dark sm", type: "button", text: "Decline it", onclick: function () {
        var reason = String(why.value || "").trim();
        if (!reason) { whySay.textContent = "Say why in a few words, so they know what to change."; return; }
        K.api("POST", "/orgs/" + slug + "/candidates/" + c.digest + "/decline", { reason: reason }).then(function (out) {
          if (!out.ok) { whySay.textContent = words(out, { what: "decline versions" }).title || "It did not go through. Try again."; return; }
          toast(K.appName(c) + " was declined. " + K.who(c.uploaded_by).label + " sees why.");
          show("waiting");
          countWaiting();
        });
      } })]),
      whySay,
    ]);
    var waited = now - (c.uploaded_at || now);
    var update = c.changes && c.changes.against;
    var name = K.appName(c);
    var card = el("div", { class: "card wcard" }, [
      el("div", { class: "wc-h" }, [
        el("span", { class: "apptile", "aria-hidden": "true", text: String(c.name || name).slice(0, 1).toUpperCase() }),
        el("div", { class: "t" }, [
          el("b", {}, [name, " ", el("span", { class: "tag", text: update ? "Update from " + (typeof update === "string" ? update : update.version || "the version that opens now") : "New app" })]),
          el("small", { text: "Sent by " + K.who(c.uploaded_by).label + " · " + K.moment(c.uploaded_at) }),
        ]),
        el("span", { class: "tag" + (waited > 2 * 86400 ? " warn" : ""), text: "Waiting " + age(waited) }),
      ]),
      c.note ? el("blockquote", { class: "wnote", text: "\u201c" + c.note + "\u201d, " + K.who(c.uploaded_by).label }) : null,
      review.node,
      Ap ? Ap.standing(c) : null,
      Ap ? Ap.codeBox(c, { focus: opts.focus }) : null,
      note,
      el("div", { class: "row2" }, acts),
      declineBox,
    ]);
    if (btn) review.gate(btn);
    return card;
  }
  /// What was decided lately: the last few versions released or withdrawn.
  function decided(pane, all) {
    var done = all.filter(function (c) { return c.state !== "waiting-for-approval" && c.approved_at; })
      .sort(function (x, y) { return (y.approved_at || 0) - (x.approved_at || 0); }).slice(0, 5);
    if (!done.length) return;
    pane.appendChild(el("div", { class: "card rows" }, [el("p", { class: "grp", text: "Recently decided" })].concat(done.map(function (c) {
      return el("div", { class: "rw flat opens", onclick: function () { inspectCandidate(c); } }, [el("div", { class: "t" }, [
        el("b", { text: K.appName(c) }),
        el("small", { text: (c.state === "withdrawn" ? "Withdrawn" : "Approved") + " by " + (c.approved_with && c.approved_with.length ? c.approved_with : [c.approved_by]).map(function (p) { return K.who(p).label; }).join(" and ") + " · " + K.moment(c.approved_at) + (c.approved_by === c.uploaded_by ? " · released by its sender alone" : "") }),
      ])]);
    }))));
  }
  /// One version, in full, in the right-hand panel.
  function inspectCandidate(c) {
    var caps = Array.isArray(c.capabilities) ? c.capabilities : [];
    inspect(K.appName(c), [
      el("dl", { class: "cr-kv" }, [
        el("dt", { text: "State" }), el("dd", { text: c.state === "waiting-for-approval" ? "Waiting for approval" : c.state === "withdrawn" ? "Withdrawn" : "Released" }),
        el("dt", { text: "Sent by" }), el("dd", { text: K.who(c.uploaded_by).label }),
        el("dt", { text: "Sent" }), el("dd", { text: K.moment(c.uploaded_at) }),
        c.approved_by ? el("dt", { text: "Approved by" }) : null, c.approved_by ? el("dd", { text: K.who(c.approved_by).label }) : null,
        c.changes && c.changes.against ? el("dt", { text: "Replaces" }) : null, c.changes && c.changes.against ? el("dd", { text: typeof c.changes.against === "string" ? c.changes.against : c.changes.against.version || c.changes.against.digest }) : null,
      ]),
      caps.length ? el("div", { class: "cr-sec" }, [el("h3", { text: "It can" }), el("div", { class: "cr-list" }, caps.map(function (x) { return el("div", {}, [el("b", { text: x.words || x.cap })]); }))]) : null,
      el("details", { class: "cr-tech" }, [el("summary", { text: "Technical details" }), el("dl", { class: "cr-kv" }, [el("dt", { text: "Fingerprint" }), el("dd", { class: "dg", text: c.digest })])]),
    ]);
  }

  /// What a waiting candidate can do, and what it changes against the
  /// version that opens now (IC-950). A new capability a person is asked
  /// for gets a tick; Approve waits until every one is ticked, and the
  /// hub refuses an approval that leaves one out.
  function capsReview(c, deciding) {
    var caps = Array.isArray(c.capabilities) ? c.capabilities : [];
    var ch = c.changes || { against: null, added: caps, removed: [], kept: 0 };
    var box = el("div", { class: "caps" });
    var ticks = [];
    var line = function (x, extra) {
      return el("li", {}, [
        el("span", { text: x.words || x.cap }),
        x.required ? null : el("span", { class: "opt", text: " (optional)" }),
        x.rationale ? el("small", { text: "Its maker says: " + x.rationale }) : null,
        extra || null,
      ]);
    };
    if (!caps.length && !ch.removed.length) {
      box.appendChild(el("p", { class: "cl", text: "It asks for nothing beyond what every app may do." }));
    } else if (!ch.against) {
      var asked = caps.filter(function (x) { return x.asks; });
      var basic = caps.filter(function (x) { return !x.asks; });
      box.appendChild(el("p", { class: "cl", text: asked.length ? "What it can do once someone allows it:" : "It asks for nothing a person is asked about." }));
      if (asked.length) box.appendChild(el("ul", {}, asked.map(function (x) { return line(x); })));
      if (basic.length) box.appendChild(el("p", { class: "cl sub", text: "And what every app may do: " + basic.map(function (x) { return x.words || x.cap; }).join(", ") + "." }));
    } else {
      var since = ch.against.version
        ? "Compared with " + ch.against.version + ", the version that opens now:"
        : "Compared with the version that opens now:";
      box.appendChild(el("p", { class: "cl", text: since }));
      var newAsked = ch.added.filter(function (x) { return x.asks; });
      var newBasic = ch.added.filter(function (x) { return !x.asks; });
      if (newAsked.length) {
        box.appendChild(el("p", { class: "cl new", text: "New, check each before approving:" }));
        box.appendChild(el("ul", { class: "new" }, newAsked.map(function (x) {
          var tick = el("input", { type: "checkbox", "aria-label": "I checked: " + (x.words || x.cap) });
          tick.disabled = !deciding;
          ticks.push({ cap: x.cap, box: tick });
          return el("li", {}, [el("label", { class: "tick" }, [tick, el("span", { text: x.words || x.cap })]),
            x.required ? null : el("span", { class: "opt", text: " (optional)" }),
            x.rationale ? el("small", { text: "Its maker says: " + x.rationale }) : null]);
        })));
      }
      if (newBasic.length) box.appendChild(el("p", { class: "cl sub", text: "Also new, not asked about: " + newBasic.map(function (x) { return x.words || x.cap; }).join(", ") + "." }));
      if (ch.removed.length) box.appendChild(el("p", { class: "cl sub", text: "No longer asks to: " + ch.removed.map(function (x) { return x.words || x.cap; }).join(", ") + "." }));
      if (!ch.added.length && !ch.removed.length) box.appendChild(el("p", { class: "cl sub", text: "It asks for exactly the same as before." }));
      else if (ch.kept) box.appendChild(el("p", { class: "cl sub", text: "Everything else is as before (" + ch.kept + (ch.kept === 1 ? " permission" : " permissions") + ")." }));
    }
    return {
      node: box,
      accepted: function () { return ticks.filter(function (t) { return t.box.checked; }).map(function (t) { return t.cap; }); },
      newOnes: function () { return ch.against ? ch.added.filter(function (x) { return x.asks; }) : []; },
      gate: function (btn) {
        if (!ticks.length) return;
        var sync = function () {
          var all = ticks.every(function (t) { return t.box.checked; });
          btn.disabled = !all;
          btn.title = all ? "" : "Tick each new capability first";
        };
        ticks.forEach(function (t) { t.box.addEventListener("change", sync); });
        sync();
      },
    };
  }

  function approve(c, accepted) {
    var key = K.key();
    var added = (c.changes && c.changes.against ? c.changes.added : []).filter(function (x) { return x.asks; });
    var you = c.approval && c.approval.you;
    var partial = you && you.can && you.releases === false;
    confirmSheet({
      title: "Approve " + K.appName(c) + "?",
      body: partial
        ? "Your approval is recorded, and it keeps waiting: " + String(you.after || "it needs more approvals").replace(/^Needs/, "it needs") + ". It is released once the rules are met."
        : "This releases exactly these bytes to everyone in " + (S.org.name || slug) + ". If the app changes, the new version waits for its own approval.",
      points: added.map(function (x) { return { key: true, text: "New: it can " + (x.words || x.cap) }; }),
      digest: c.digest,
      yes: partial ? "Approve" : "Approve and release",
      busy: partial ? "Approving" : "Releasing",
      ctx: { what: "approve releases", whoCan: "owners and reviewers" },
      act: function () { return K.api("POST", "/orgs/" + slug + "/candidates/" + c.digest + "/approve", { accept: accepted || [] }, { key: key }); },
      done: function (res) {
        if (res.status === 202) {
          var a = (res.body && res.body.approval) || {};
          toast("Your approval is in. " + K.appName(c) + " " + String(a.words || "needs more approvals").replace(/^Needs/, "needs") + ".");
          show("waiting", { c: S.focus || "" });
          countWaiting();
          return;
        }
        toast(K.appName(c) + " is released. Everyone in " + (S.org.name || slug) + " can open it now.");
        show("releases");
        countWaiting();
      },
    });
  }

  // ------------------------------------------------------------ people
  function people(pane) {
    K.api("GET", "/orgs/" + slug).then(function (res) {
      if (!res.ok) { head(pane, "People"); return failed(pane, res, {}); }
      take(res.body);
      var role = roleOf();
      head(pane, "People", role === "recipient"
        ? "You receive " + (S.org.name || slug) + "'s apps. Recipients see only themselves here."
        : "Who is in " + (S.org.name || slug) + ", and what each person may do.");
      var roles = K.invitable(role);
      if (roles.length) {
        var phd = pane.querySelector(".phd");
        if (phd) phd.appendChild(el("div", { class: "pa" }, [el("button", { class: "btn sm dark", type: "button", text: "Invite people", onclick: function () { openInvite(); } })]));
      }
      if (Pub) Pub.peopleCard(pane);
      var groupsAt = el("div", { hidden: true });

      var active = S.members.filter(function (m) { return (m.state || "active") === "active"; });
      var gone = S.members.filter(function (m) { return m.state === "removed"; });
      var owners = active.filter(function (m) { return m.role === "owner"; }).length;
      var team = active.filter(function (m) { return m.role !== "recipient"; });
      var recips = active.filter(function (m) { return m.role === "recipient"; });
      // Who is here, first: a count, and a search once the list is long.
      var tools = el("div", { class: "ptools" }, [
        el("p", { class: "sub", text: plural(active.length, "person", "people") + (recips.length && team.length ? ": " + team.length + " who run apps, " + recips.length + " who use them" : "") }),
      ]);
      var card = el("div", { class: "card rows", id: "peopleList" });
      if (active.length > 5) {
        var find = el("input", { class: "inp psearch", type: "search", placeholder: "Search people", "aria-label": "Search people", autocomplete: "off" });
        find.addEventListener("input", function () {
          var q = find.value.trim().toLowerCase();
          Array.prototype.slice.call(card.querySelectorAll(".rw")).forEach(function (r) { r.hidden = !!q && r.textContent.toLowerCase().indexOf(q) < 0; });
        });
        tools.appendChild(find);
      }
      if (role !== "recipient") pane.appendChild(tools);
      pane.appendChild(card);
      if (team.length) {
        if (recips.length) card.appendChild(el("p", { class: "grp", text: "Members" }));
        team.forEach(function (m) { card.appendChild(memberRow(m, owners)); });
      }
      if (recips.length) {
        if (role !== "recipient") card.appendChild(el("p", { class: "grp", text: "Recipients: they open the apps, and nothing else" }));
        recips.forEach(function (m) { card.appendChild(memberRow(m, owners)); });
      }
      var goneCard = null;
      if (gone.length) {
        var g = goneCard = el("div", { class: "card rows" }, [el("p", { class: "grp", text: "Removed. What they sent and approved stays on record." })]);
        gone.forEach(function (m) {
          var w = K.who(m);
          g.appendChild(el("div", { class: "rw flat" }, [
            el("span", { class: "who", "aria-hidden": "true", text: w.display.slice(0, 1).toUpperCase() }),
            el("div", { class: "t" }, [
              el("b", { text: w.label }),
              el("small", { text: "Was " + (K.ROLE_NAME[m.role] || m.role).toLowerCase() + (m.removed_at ? " · removed " + K.day(m.removed_at) : "") + (m.removed_by ? " by " + K.who(m.removed_by).label : "") }),
            ]),
          ]));
        });
      }
      // Inviting: many by email, or one by GitHub with any role, in one
      // card under the people it adds to.
      if (roles.length) {
        var inv = el("div", { class: "card", id: "inviteCard" }, [
          el("h3", { text: "Invite people" }),
          el("p", { text: "Each gets an email with one button to join " + (S.org.name || slug) + "." }),
        ]);
        var bulkBox = el("div", { id: "bulkCard" });
        var one = inviteCard(roles, true);
        if (Setup) {
          var mode = el("div", { class: "seg inv-mode", role: "group", "aria-label": "Invite" }, [
            el("button", { type: "button", "aria-pressed": "true", text: "Emails", onclick: function () { pick(true); } }),
            el("button", { type: "button", "aria-pressed": "false", text: "One person, any role", onclick: function () { pick(false); } }),
          ]);
          var pick = function (bulk) {
            mode.children[0].setAttribute("aria-pressed", bulk ? "true" : "false");
            mode.children[1].setAttribute("aria-pressed", bulk ? "false" : "true");
            bulkBox.hidden = !bulk;
            one.hidden = bulk;
          };
          inv.appendChild(mode);
          inv.appendChild(bulkBox);
          Setup.invite(bulkBox, {});
          one.hidden = true;
        }
        inv.appendChild(one);
        pane.appendChild(inv);
        if (S.opts && S.opts.invite) openInvite();
      }
      if (gone.length) pane.appendChild(goneCard);
      // Groups (IC-949, R), below the people they are made of.
      pane.appendChild(groupsAt);
      if (R) R.groupsCard(groupsAt, active);
    });
  }
  function openInvite() {
    var c = $("inviteCard");
    if (!c) { show("people", { invite: true }); return; }
    if (c.scrollIntoView) c.scrollIntoView({ block: "start", behavior: "smooth" });
    var f = c.querySelector("textarea, input");
    if (f && f.focus) setTimeout(function () { f.focus(); }, 250);
  }
  function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
  /// The IT pack (kit/org-setup.js): installers and the policy files, in the panel.
  function itPackPanel() {
    if (!Setup || !Setup.itPack) { show("settings"); return; }
    var box = el("div");
    inspect("The IT pack", [box]);
    Setup.itPack(box, {});
  }
  /// Releasing an app from anywhere: the first-app flow, in the panel.
  function releaseFlow() {
    if (!Setup) { show("releases"); return; }
    var box = el("div", { class: "cr-release" });
    inspect("Release an app", [box]);
    Setup.firstApp(box, {
      title: false,
      onDone: function () { toast("Released. Everyone it is for can open it now."); show("releases"); },
      onWaiting: function (label) { toast(label + " was sent. It waits for a second person to approve it."); show("waiting"); },
    });
  }
  /// One person, in the right-hand panel.
  function inspectMember(m) {
    var w = K.who(m);
    inspect(w.label, [
      el("div", { class: "cr-sec" }, [
        el("span", { class: "tag" + (m.role === "owner" ? " blue" : ""), text: K.ROLE_NAME[m.role] || m.role }),
        el("p", { class: "cr-sub", style: "margin-top:8px", text: K.ROLE_SAYS[m.role] || "" }),
      ]),
      el("dl", { class: "cr-kv" }, [
        el("dt", { text: "Signs in with" }), el("dd", { text: w.kind === "email" ? "Email, " + w.display : "GitHub, @" + w.display.replace(/^@/, "") }),
        m.since ? el("dt", { text: "Joined" }) : null, m.since ? el("dd", { text: K.day(m.since) }) : null,
        m.added_by ? el("dt", { text: "Added by" }) : null, m.added_by ? el("dd", { text: K.who(m.added_by).label }) : null,
      ]),
      el("p", { class: "cr-sub", text: "Change their role or remove them from the row in People." }),
    ]);
  }

  function memberRow(m, owners) {
    var w = K.who(m);
    var mine = isMe(m.principal);
    var a = K.memberActions({ principal: me().principal, role: roleOf() }, { principal: w.principal, role: m.role }, owners);
    var note = el("p", { class: "note", hidden: true });
    var acts = [];
    if (a.roles.length) {
      var sel = el("select", { class: "sel", "aria-label": "Role for " + w.label });
      a.roles.forEach(function (r) {
        var o = el("option", { value: r, text: K.ROLE_NAME[r] });
        if (r === m.role) o.selected = true;
        sel.appendChild(o);
      });
      sel.addEventListener("change", function () { changeRole(m, sel, note); });
      acts.push(sel);
    } else {
      acts.push(el("span", { class: "tag" + (m.role === "owner" ? " blue" : ""), text: K.ROLE_NAME[m.role] || m.role }));
    }
    if (a.remove) {
      var rm = el("button", { class: "btn quiet sm" + (a.removeWhy ? " soft-off" : ""), type: "button", text: a.removeLabel });
      if (a.removeWhy) {
        rm.setAttribute("aria-disabled", "true");
        rm.title = a.removeWhy;
        rm.addEventListener("click", function () { note.textContent = a.removeWhy; note.hidden = false; });
      } else {
        rm.addEventListener("click", function () { removeMember(m, mine); });
      }
      acts.push(rm);
    }
    var row = el("div", { class: "rw opens" }, [
      el("span", { class: "who", "aria-hidden": "true", text: w.display.slice(0, 1).toUpperCase() }),
      el("div", { class: "t" }, [
        el("b", {}, [w.label + (mine ? " (you)" : ""), el("span", { class: "tag kind", text: w.kind === "email" ? "Email" : "GitHub" })]),
        el("small", { text: K.ROLE_SAYS[m.role] + (m.since ? " Since " + K.day(m.since) + "." : "") }),
        note,
      ]),
      el("div", { class: "ra" }, acts),
    ]);
    // A click on the row, not on its controls, opens the person on the right.
    row.addEventListener("click", function (e) {
      var t = e && e.target;
      if (t && t.closest && t.closest("select, button, a, input")) return;
      inspectMember(m);
    });
    return row;
  }

  function changeRole(m, sel, note) {
    var was = m.role, to = sel.value, w = K.who(m);
    sel.disabled = true;
    K.api("PUT", "/orgs/" + slug + "/members/" + encodeURIComponent(w.principal), { role: to }, { key: K.key() }).then(function (res) {
      sel.disabled = false;
      if (res.ok) {
        toast(w.label + " is now " + (K.ROLE_NAME[to] || to).toLowerCase() + ".");
        show("people");
        return;
      }
      if (res.body.error === "not-signed-in") { K.forget(); signedOut(); return; }
      sel.value = was;
      var x = words(res, { what: "change that role" });
      note.textContent = x.title + ". " + x.body;
      note.hidden = false;
    });
  }

  function removeMember(m, mine) {
    var w = K.who(m), name = S.org.name || slug;
    var key = K.key();
    confirmSheet({
      title: mine ? "Leave " + name + "?" : "Remove " + w.label + " from " + name + "?",
      body: mine
        ? "Its apps stop opening for you at once. Your own apps and your account stay as they are. To come back you need a new invitation."
        : "Their access ends at once, the next time they open an app. Their own apps and account are untouched, and what they sent or approved stays on record.",
      yes: mine ? "Leave" : "Remove",
      busy: mine ? "Leaving" : "Removing",
      ctx: { what: "remove " + w.label, whoCan: "an owner, or an admin for people who are not owners" },
      act: function () { return K.api("DELETE", "/orgs/" + slug + "/members/" + encodeURIComponent(w.principal), null, { key: key }); },
      done: function () {
        if (mine) { location.href = "/orgs/"; return; }
        toast(w.label + " was removed. Their next open is refused.");
        show("people");
      },
    });
  }

  function inviteCard(roles, bare) {
    var by = "github";
    var card = el("div", { class: bare ? "inv-one" : "card" });
    var seg = el("div", { class: "seg", role: "group", "aria-label": "Invite by" });
    var input = el("input", { class: "inp", id: "invWho", autocomplete: "off", autocapitalize: "none", spellcheck: "false", "aria-label": "GitHub username", placeholder: "GitHub username, like octocat" });
    var sel = el("select", { class: "sel", id: "invRole", "aria-label": "Role" });
    var says = el("p", { class: "rsays" });
    var out = el("div", { id: "invOut" });
    function setBy(v) {
      by = v;
      [].forEach.call(seg.children, function (b) { b.setAttribute("aria-pressed", b.dataset.by === v ? "true" : "false"); });
      input.type = v === "email" ? "email" : "text";
      input.placeholder = v === "email" ? "Work email, like sam@acme.com" : "GitHub username, like octocat";
      input.setAttribute("aria-label", v === "email" ? "Work email" : "GitHub username");
      input.value = "";
      input.focus();
    }
    [["github", "GitHub username"], ["email", "Work email"]].forEach(function (p) {
      var b = el("button", { type: "button", text: p[1], "aria-pressed": p[0] === by ? "true" : "false" });
      b.dataset.by = p[0];
      b.addEventListener("click", function () { setBy(p[0]); });
      seg.appendChild(b);
    });
    roles.forEach(function (r) {
      var o = el("option", { value: r, text: K.ROLE_NAME[r] });
      if (r === "developer") o.selected = true;
      sel.appendChild(o);
    });
    var sayRole = function () { says.textContent = K.ROLE_NAME[sel.value] + ": " + K.ROLE_SAYS[sel.value]; };
    sel.addEventListener("change", sayRole);
    sayRole();
    var go = el("button", { class: "btn dark", type: "submit", text: "Create invitation" });
    var form = el("form", { novalidate: true }, [el("div", { class: "invrow" }, [input, sel, says, go])]);
    var pending = null;
    form.addEventListener("submit", function (e) {
      e.preventDefault();
      var v = input.value.trim();
      var body = { role: sel.value };
      if (by === "github") {
        v = v.replace(/^@/, "").toLowerCase();
        if (!/^[a-z0-9](?:[a-z0-9-]{0,38})$/.test(v)) return hint("That is not a GitHub username", "A username is letters, digits and dashes, like octocat. It is the name in their github.com address.");
        body.github = v;
      } else {
        v = v.toLowerCase();
        if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v)) return hint("That is not an email address", "Use the address they sign in with, like sam@acme.com.");
        body.email = v;
      }
      var sig = JSON.stringify(body);
      if (!pending || pending.sig !== sig) pending = { sig: sig, key: K.key() };
      go.disabled = true;
      go.textContent = "Creating";
      K.api("POST", "/orgs/" + slug + "/invites", body, { key: pending.key }).then(function (res) {
        go.disabled = false;
        go.textContent = "Create invitation";
        out.textContent = "";
        if (!res.ok) {
          if (res.body.error === "not-signed-in") { K.forget(); signedOut(); return; }
          out.appendChild(K.refusalBlock(words(res, { what: "invite a " + body.role, whoCan: body.role === "owner" || body.role === "admin" ? "an owner" : "an owner or an admin" }), "bad"));
          return;
        }
        pending = null;
        input.value = "";
        out.appendChild(inviteResult(res.body, body));
      });
    });
    function hint(t, b) {
      out.textContent = "";
      out.appendChild(K.refusalBlock({ title: t, body: b, action: null }, "warn"));
      input.focus();
    }
    if (!bare) {
      card.appendChild(el("h3", { text: Setup ? "Invite one person, by GitHub or with any role" : "Invite someone" }));
      card.appendChild(el("p", { text: "By GitHub username, or by the work email they sign in with. You get a link to send them; it works once, only for them." }));
    } else {
      card.appendChild(el("p", { class: "sub", text: "By GitHub username or work email, with any role you may give. You get a link to send them; it works once, only for them." }));
    }
    card.appendChild(el("div", { style: "margin-top:14px" }, [seg]));
    card.appendChild(form);
    card.appendChild(out);
    return card;
  }

  function inviteResult(ans, sent) {
    var whoLabel = sent.github ? "@" + sent.github : sent.email;
    var link = K.joinUrl(ans);
    var box = el("div", { class: "invres" });
    var inp = el("input", { class: "inp", readonly: true, value: link, "aria-label": "Join link" });
    var cp = el("button", { class: "btn ghost", type: "button", text: "Copy link", onclick: function () { inp.select(); K.copy(link, cp); } });
    var withdraw = el("button", { class: "btn link", type: "button", text: "Withdraw this invitation" });
    withdraw.addEventListener("click", function () {
      withdraw.disabled = true;
      K.api("DELETE", "/orgs/" + slug + "/invites/" + encodeURIComponent(ans.code), null, { key: K.key() }).then(function (res) {
        withdraw.disabled = false;
        box.textContent = "";
        if (res.ok) {
          box.appendChild(el("p", { text: "Withdrawn. The link for " + whoLabel + " no longer works." }));
          return;
        }
        box.appendChild(K.refusalBlock(words(res, { what: "withdraw invitations" }), "bad"));
      });
    });
    // Mailed or not (Addendum 3 A): say which, and why not.
    var mail = P.inviteMail(ans, sent);
    box.appendChild(el("p", { class: "invfor", text: "Invitation for " + whoLabel + " as " + (K.ROLE_NAME[sent.role] || sent.role).toLowerCase() }));
    box.appendChild(el("b", { text: mail.title }));
    box.appendChild(el("p", { text: mail.body }));
    box.appendChild(el("div", { class: "linkbox" }, [inp, cp]));
    box.appendChild(withdraw);
    return box;
  }

  // ---------------------------------------------------------- activity
  function activity(pane) {
    var names = K.api("GET", "/orgs/" + slug + "/candidates").then(function (res) {
      if (res.ok && Array.isArray(res.body)) res.body.forEach(function (c) { S.names[c.digest] = K.appName(c); });
    });
    var audit = K.api("GET", "/orgs/" + slug + "/audit");
    Promise.all([names, audit]).then(function (both) {
      var res = both[1];
      head(pane, "Activity", "Every change and every refused attempt, newest first. Each entry is chained to the one before it.");
      if (!res.ok) return failed(pane, res, { what: "read the activity", whoCan: "owners and admins" });
      var ev = (res.body.events || []).slice().reverse();
      var ok = res.body.chain_ok === true;
      var badge = el("div", { class: "card" }, [
        el("div", { class: "row", style: "display:flex;align-items:center;gap:12px;flex-wrap:wrap" }, [
          el("span", { class: "tag chain " + (ok ? "ok" : "warn"), text: ok ? "Chain intact" : "Chain broken" }),
          el("span", { class: "sub", style: "margin:0", text: ok
            ? "The hub rechecked all " + ev.length + " entries just now. None was changed after it was written."
            : "An entry was changed after it was written. The hub checks the whole chain every time it is read; tell Krate support." }),
        ]),
      ]);
      pane.appendChild(badge);
      // Narrow it down: a kind of entry, and words to look for.
      var KINDS = [
        ["all", "All", function () { return true; }],
        ["denied", "Refused", function (e) { return e.result === "denied"; }],
        ["people", "People", function (e) { return /^(member|invite|scim|sso|saml|group)\./.test(e.action); }],
        ["apps", "Releases", function (e) { return /^(candidate|release|lease|offline|app|publisher)\./.test(e.action); }],
        ["setup", "Settings and security", function (e) { return /^(key|signing|root-key|release-key|settings|review|connection|audit-export|billing|org|data)\./.test(e.action) || /^(org\.|billing\.)/.test(e.action); }],
      ];
      var kind = "all", words = "";
      var bar = el("div", { class: "evtools" });
      var chips = el("div", { class: "chips", role: "group", "aria-label": "Show" }, KINDS.map(function (k) {
        var n = ev.filter(k[2]).length;
        return el("button", { class: "chip", type: "button", "aria-pressed": k[0] === "all" ? "true" : "false", text: k[1] + (k[0] === "all" ? "" : " " + n), onclick: function () {
          kind = k[0];
          Array.prototype.slice.call(chips.children).forEach(function (c, i) { c.setAttribute("aria-pressed", KINDS[i][0] === kind ? "true" : "false"); });
          redraw();
        } });
      }));
      var find = el("input", { class: "inp psearch", type: "search", placeholder: "Search the activity", "aria-label": "Search the activity", autocomplete: "off" });
      find.addEventListener("input", function () { words = find.value.trim().toLowerCase(); redraw(); });
      bar.appendChild(chips);
      bar.appendChild(find);
      pane.appendChild(bar);
      var card = el("div", { class: "card rows" });
      pane.appendChild(card);
      var shown = 0, list = ev;
      function redraw() {
        var test = KINDS.filter(function (k) { return k[0] === kind; })[0][2];
        list = ev.filter(function (e) { return test(e) && (!words || K.eventWords(e, S.names).toLowerCase().indexOf(words) >= 0 || K.who(e.actor).label.toLowerCase().indexOf(words) >= 0); });
        card.textContent = "";
        shown = 0;
        if (!list.length) { card.appendChild(el("p", { class: "sub", style: "padding:16px 0", text: "Nothing matches. Try All, or other words." })); return; }
        more();
      }
      function more() {
        list.slice(shown, shown + 60).forEach(function (e) {
          var denied = e.result === "denied";
          card.appendChild(el("button", { class: "ev opens" + (denied ? " denied" : ""), type: "button", onclick: function () { inspectEvent(e); } }, [
            el("p", {}, [denied ? el("span", { class: "tag warn", text: "Refused" }) : null, K.eventWords(e, S.names)]),
            el("time", { datetime: new Date(e.at * 1000).toISOString(), text: K.moment(e.at) }),
          ]));
        });
        shown += 60;
        if (shown < list.length) {
          var b = el("button", { class: "btn ghost sm", type: "button", text: "Show older", style: "margin:12px 0" });
          b.addEventListener("click", function () { b.remove(); more(); });
          card.appendChild(b);
        }
      }
      more();
    });
  }

  // ---------------------------------------------------------- settings
  function settings(pane) {
    K.api("GET", "/orgs/" + slug).then(function (res) {
      head(pane, "Settings", (S.org.name || slug) + "'s signature, how apps are sent to it, and how your people sign in.");
      if (!res.ok) return failed(pane, res, {});
      take(res.body);
      var o = S.org, owner = roleOf() === "owner";
      if (o.state === "closed") {
        head(pane, "Settings", (o.name || slug) + " is closed. Its export is what remains.");
        P.settings(pane, null);
        return;
      }
      var name = o.name || slug;
      // The click path first; the commands stay, for the terminal, folded
      // under one line (IC-958: nobody is told to run a key command).
      var terminal = function (lines) {
        return el("details", { class: "term" }, [
          el("summary", { text: "For developers who use the terminal" }),
          el("div", { class: "tb" }, lines.map(function (s) {
            return el("div", {}, [el("p", { class: "sub", text: s[0] }), el("pre", { class: "k", text: s[1] })]);
          })),
        ]);
      };
      // The organisation itself first: its name and the day it counts by.
      pane.appendChild(el("div", { class: "card" }, [
        el("h3", { text: "Organisation" }),
        el("dl", { class: "kv" }, [
          el("dt", { text: "Name" }), el("dd", { text: o.name || slug }),
          el("dt", { text: "Short name" }), el("dd", { text: o.slug || slug }),
          el("dt", { text: "Created" }), el("dd", { text: K.day(o.created_at) + (o.created_by ? " by " + K.who(o.created_by).label : "") }),
        ]),
        logoSetting(o, name),
        o.time_zone ? zoneSetting(o, name) : null,
      ]));
      var key = el("div", { class: "card", id: "sigCard" }, [el("h3", { text: name + "'s signature" })]);
      if (o.root_key) {
        key.appendChild(el("p", { text: "Every app " + name + " releases is signed by " + name + ", and every computer checks it before the app opens. Compare this fingerprint with an owner's copy." }));
        key.appendChild(el("p", { class: "fp", text: K.fingerprint(o.root_key) }));
        key.appendChild(el("dl", { class: "kv" }, [
          el("dt", { text: "Made" }), el("dd", { text: (K.moment(o.root_key_set_at) || "Unknown") + (o.root_key_set_by ? " by " + K.who(o.root_key_set_by).label : "") }),
        ]));
      } else {
        key.appendChild(el("p", { text: name + " gets its signature when its first app is released, from Releases in this browser or from Krate Studio. Until then nothing can be sent to " + name + "." }));
        key.appendChild(el("div", { class: "row2" }, [el("button", { class: "btn dark sm", type: "button", text: "Release an app", onclick: function () { show("releases"); } })]));
      }
      // The key itself, and replacing it with one an owner keeps in a
      // file, are for those who look for them: under Technical details.
      var tech = el("details", { class: "term tech" }, [el("summary", { text: "Technical details" })]);
      var techBody = el("div", { class: "tb" });
      tech.appendChild(techBody);
      if (o.root_key) techBody.appendChild(el("dl", { class: "kv" }, [el("dt", { text: "Full public key" }), el("dd", { class: "dg", text: o.root_key })]));
      if (owner) {
        techBody.appendChild(el("p", { class: "sub", text: o.root_key
          ? "Replace it from a terminal with the new key's file (Activity records the change):"
          : "Or make it from a terminal, on the computer that keeps it:" }));
        techBody.appendChild(el("pre", { class: "k", text: "krate org root-key " + slug + " " + slug + "-root.pkcs8" + (o.root_key ? "" : " --new") }));
      }
      if (o.root_key || owner) key.appendChild(tech);
      pane.appendChild(key);

      pane.appendChild(el("div", { class: "card" }, [
        el("h3", { text: "Send an app to " + name }),
        el("p", { text: "From this browser: Release an app in Releases, a ready app or a .krate you have. From Krate Studio: Publish to " + name + " in the app's menu. Either way the app is signed for " + name + " on the way." }),
        el("div", { class: "row2" }, [
          el("button", { class: "btn ghost sm", type: "button", text: "Release an app", onclick: function () { show("releases"); } }),
          el("a", { class: "btn ghost sm", href: "/download/", text: "Get Krate Studio" }),
        ]),
        terminal([
          ["Once, an owner gives each developer a release key:", "krate delegate \\\n  --root " + slug + "-root.pkcs8 \\\n  --key release.pkcs8 \\\n  --namespace '" + slug + "/*' \\\n  -o release-" + slug + ".json"],
          ["The developer marks the app as " + name + "'s and signs it:", "krate sign app.krate --org " + slug + " \\\n  --key release.pkcs8 \\\n  --delegation release-" + slug + ".json"],
          ["And sends it here, where it waits for a second person:", "krate publish app.krate --org " + slug],
        ]),
      ]));
      // Solo release and new publishers (IC-958): drawn only when the hub
      // has them.
      if (Pub) Pub.settings(pane);
      if (Ap) Ap.rulesCard(pane);


      P.settings(pane, techBody);
      if (S.navRedraw) S.navRedraw();
    });
  }

  /// The organisation's tile at the top of the sidebar: its logo when it
  /// has one, its initial when not.
  function drawTile() {
    var t = $("oTile");
    if (!t) return;
    t.textContent = "";
    var name = (S.org && S.org.name) || slug;
    if (S.org && S.org.logo) {
      t.className = "otile logo";
      t.appendChild(el("img", { src: S.org.logo, alt: "" }));
    } else {
      t.className = "otile";
      t.textContent = name.slice(0, 1).toUpperCase();
    }
  }

  /// Settings, General: the organisation's logo. The picture is shrunk
  /// here to a square of at most 160 pixels and sent as a small PNG or
  /// WebP, whatever was chosen; an SVG is drawn first, so no script in it
  /// can travel.
  function logoSetting(o, name) {
    var may = roleOf() === "owner" || roleOf() === "admin";
    var view = el("span", { class: "lg-view" + (o.logo ? " has" : "") }, o.logo ? [el("img", { src: o.logo, alt: name + "'s logo" })] : [name.slice(0, 1).toUpperCase()]);
    var note = el("p", { class: "note", hidden: true });
    var file = el("input", { type: "file", id: "logoFile", class: "vh", accept: "image/png,image/jpeg,image/webp,image/svg+xml", "aria-label": "Choose a logo" });
    var pick = el("label", { class: "btn ghost sm", for: "logoFile", text: o.logo ? "Change logo" : "Upload logo" });
    var drop = el("button", { class: "btn quiet sm", type: "button", text: "Remove", hidden: !o.logo });
    function shown(v) {
      o.logo = v; S.org.logo = v;
      view.textContent = "";
      view.className = "lg-view" + (v ? " has" : "");
      if (v) view.appendChild(el("img", { src: v, alt: name + "'s logo" })); else view.textContent = name.slice(0, 1).toUpperCase();
      pick.textContent = v ? "Change logo" : "Upload logo";
      drop.hidden = !v;
      drawTile();
    }
    file.addEventListener("change", function () {
      var f = file.files && file.files[0];
      note.hidden = true;
      if (!f) return;
      shrinkLogo(f).then(function (data) {
        return K.api("PUT", "/orgs/" + slug + "/logo", { data_url: data }, { key: K.key() }).then(function (r) {
          if (!r.ok) throw r;
          shown(r.body.logo);
          toast(name + "'s logo is set. Everyone in " + name + " sees it.");
        });
      }).catch(function (e) {
        note.textContent = e && e.body ? K.refusal(e.body.error, { message: e.body.message }).body || e.body.message : "That picture could not be used. Try a PNG or JPEG.";
        note.hidden = false;
      });
      file.value = "";
    });
    drop.addEventListener("click", function () {
      K.api("DELETE", "/orgs/" + slug + "/logo", {}, { key: K.key() }).then(function (r) { if (r.ok) { shown(null); toast("The logo is removed. " + name + "'s initial shows instead."); } });
    });
    return el("div", { class: "zone lg" }, [
      el("span", { class: "lbl", text: "Logo" }),
      el("div", { class: "lg-row" }, [view, may ? el("div", { class: "row2", style: "margin:0" }, [pick, file, drop]) : null]),
      el("p", { class: "sub", text: may ? "Shown at the top of " + name + "'s pages for everyone in it. A square picture works best." : "Owners and admins choose it." }),
      note,
    ]);
  }
  function shrinkLogo(f) {
    return new Promise(function (resolve, reject) {
      var url = URL.createObjectURL(f);
      var img = new Image();
      img.onload = function () {
        var side = 160, w = img.naturalWidth || side, h = img.naturalHeight || side, k = Math.min(1, side / Math.max(w, h));
        var c = document.createElement("canvas");
        c.width = Math.max(1, Math.round(w * k)); c.height = Math.max(1, Math.round(h * k));
        c.getContext("2d").drawImage(img, 0, 0, c.width, c.height);
        URL.revokeObjectURL(url);
        var out = c.toDataURL("image/webp", 0.9);
        if (!/^data:image\/webp/.test(out) || out.length > 46000) out = c.toDataURL("image/png");
        if (out.length > 46000) out = c.toDataURL("image/jpeg", 0.85);
        resolve(out);
      };
      img.onerror = function () { URL.revokeObjectURL(url); reject(null); };
      img.src = url;
    });
  }

  /// Settings, Organisation: where its days start (F-319). Owners and
  /// admins pick a zone; everyone else reads it.
  function zoneSetting(o, name) {
    var mayChange = roleOf() === "owner" || roleOf() === "admin";
    var said = el("p", { class: "sub", id: "zoneSaid", text: "Insights count each day from midnight to midnight, " + K.zoneName(o.time_zone) + "." });
    if (!mayChange) return el("div", { class: "zone" }, [said]);
    var zones = [];
    try { zones = Intl.supportedValuesOf("timeZone").slice(); } catch (e) {}
    [o.time_zone, K.zone(), "UTC"].forEach(function (z) { if (z && zones.indexOf(z) < 0) zones.push(z); });
    zones.sort();
    var pick = el("select", { id: "zonePick", "aria-label": name + "'s time zone" }, zones.map(function (z) {
      var opt = el("option", { value: z, text: z.replace(/_/g, " ") + (z === K.zone() ? " (this computer)" : "") });
      if (z === o.time_zone) opt.selected = true;
      return opt;
    }));
    pick.addEventListener("change", function () {
      var z = pick.value;
      pick.disabled = true;
      K.api("PUT", "/orgs/" + slug + "/time-zone", { time_zone: z }, { key: K.key() }).then(function (r) {
        pick.disabled = false;
        if (!r.ok) { pick.value = o.time_zone; toast(K.refusal(r.body.error, r.body.message).body || "That time zone was not saved. Try again."); return; }
        o.time_zone = r.body.time_zone;
        o.time_zone_chosen = true;
        said.textContent = "Insights count each day from midnight to midnight, " + K.zoneName(o.time_zone) + ".";
        toast("From now on " + name + "'s days start at midnight, " + K.zoneName(o.time_zone) + ". Days already counted keep theirs.");
      });
    });
    return el("div", { class: "zone" }, [
      el("label", { class: "lbl", for: "zonePick", text: "Time zone" }),
      pick,
      said,
    ]);
  }

  // -------------------------------------------------------------- start
  function take(body) {
    S.org = body.org || S.org;
    S.me = body.me || S.me;
    S.members = Array.isArray(body.members) ? body.members : S.members;
    if (P) P.banner($("banner"));
  }

  /// Ask the store again after a change to the organisation itself
  /// (closing, cancelling, an objection): the banner and tabs follow.
  function refresh() {
    K.api("GET", "/orgs/" + slug).then(function (res) {
      if (!res.ok) { stop(res.body.error, res.body.message); return; }
      take(res.body);
      show(S.tab);
    });
  }

  // Setting the organisation up from here (kit/org-setup.js, IC-958) and
  // the publishers waiting to be approved (publishers.js); absent, the
  // page is as it was.
  var Setup = window.KrateOrgSetup
    ? window.KrateOrgSetup({ slug: slug, here: HERE, org: function () { return S.org; }, role: roleOf, members: function () { return S.members; } })
    : null;
  var Pub = window.KrateOrgPublishers
    ? window.KrateOrgPublishers({ S: S, slug: slug, words: words, toast: toast, roleOf: roleOf, drawTabs: function () { drawTabs(); }, show: function (id) { show(id); } })
    : null;

  // Release control and groups (releases.js); absent, the flat list.
  var R = null;
  P = window.KrateOrgParts({
    S: S, slug: slug, words: words, confirmSheet: confirmSheet, head: head, failed: failed,
    toast: toast, show: function (id) { show(id); }, refresh: refresh, roleOf: roleOf, isMe: isMe,
  });
  if (window.KrateOrgReleases) {
    R = window.KrateOrgReleases({
      S: S, slug: slug, words: words, confirmSheet: confirmSheet, head: head, failed: failed,
      toast: toast, show: function (id) { show(id); }, roleOf: roleOf, P: P, flat: flatReleases, empty: emptyReleases,
      inspect: inspect, releaseFlow: releaseFlow, openApp: function (id, t) { openApp(id, t); },
    });
  }
  // The Dashboard (home.js): where an organisation starts.
  var H = window.KrateOrgHome
    ? window.KrateOrgHome({ S: S, slug: slug, head: head, placeStart: placeStart, releaseFlow: releaseFlow, openApp: openApp, isMe: isMe, show: function (id, o) { show(id, o); }, roleOf: roleOf, inspect: inspect, tabs: tabList })
    : null;
  function home(pane) {
    if (H) { H.pane(pane); return; }
    head(pane, "Dashboard", "");
    pane.appendChild(el("div", { class: "card" }, [el("p", { text: "This page did not load completely. Reload it." })]));
  }
  // The console's own sections (pages.js) and one app's page (app.js).
  var G = window.KrateOrgPages
    ? window.KrateOrgPages({ S: S, slug: slug, head: head, failed: failed, show: function (id, o) { show(id, o); }, roleOf: roleOf, isMe: isMe,
        inspect: inspect, inspectEvent: inspectEvent, toast: toast, releaseFlow: releaseFlow, itPack: itPackPanel, openApp: openApp,
        groups: function (at) { if (R) R.groupsCard(at, (S.members || []).filter(function (m) { return (m.state || "active") === "active"; })); },
        device: function () { return Setup ? Setup.device() : Promise.resolve(null); }, get C() { return C; } })
    : null;
  var A = null;
  // The Apps tab (inventory.js, IC-954).
  var I = window.KrateOrgInventory
    ? window.KrateOrgInventory({ S: S, slug: slug, words: words, head: head, failed: failed, toast: toast, show: function (id) { show(id); }, roleOf: roleOf, inspect: inspect, releaseFlow: releaseFlow, openApp: openApp })
    : null;
  // The code behind an approval, the rules, one version on its own (approvals.js).
  var Ap = window.KrateOrgApprovals
    ? window.KrateOrgApprovals({ S: S, slug: slug, roleOf: roleOf, show: function (id, o) { show(id, o); }, words: words, toast: toast })
    : null;
  if (window.KrateOrgApp) A = window.KrateOrgApp({ S: S, slug: slug, head: head, crumb: drawCrumb, tabName: tabName, show: function (id, o) { show(id, o); }, openApp: openApp, roleOf: roleOf, inspect: inspect, waitCard: waitCard, R: R, I: I });
  // Search and the bell in the top bar (find.js).
  if (window.KrateOrgFind) window.KrateOrgFind({ S: S, slug: slug, tabs: tabList, tabName: tabName, show: function (id, o) { show(id, o); }, openApp: openApp, roleOf: roleOf, inspect: inspect, releaseFlow: releaseFlow, toast: toast }).wire($("cfind"), $("cbell"));
  function apps(pane) {
    if (I) { I.pane(pane); return; }
    head(pane, "Apps", "");
    pane.appendChild(el("div", { class: "card" }, [el("p", { text: "This page did not load completely. Reload it." })]));
  }
  // The Insights tab (insights.js, IC-959).
  var N = window.KrateOrgInsights
    ? window.KrateOrgInsights({ S: S, slug: slug, words: words, head: head, failed: failed, toast: toast, confirmSheet: confirmSheet, show: function (id) { show(id); }, roleOf: roleOf })
    : null;
  function insights(pane) {
    if (N) { N.pane(pane); return; }
    head(pane, "Insights", "");
    pane.appendChild(el("div", { class: "card" }, [el("p", { text: "This page did not load completely. Reload it." })]));
  }
  // Company connections (connections.js); absent, Settings has no card.
  var C = window.KrateOrgConnections
    ? window.KrateOrgConnections({ S: S, slug: slug, words: words, confirmSheet: confirmSheet, toast: toast, roleOf: roleOf })
    : null;

  if (!slug) { location.replace("/orgs/"); return; }
  if (!/^[a-z][a-z0-9-]{1,38}$/.test(slug)) { stop("no-such-org"); return; }
  if (!K.token()) { signedOut(); return; }

  K.api("GET", "/orgs/" + slug).then(function (res) {
    if (!res.ok) { stop(res.body.error, res.body.message); return; }
    take(res.body);
    var name = S.org.name || slug;
    document.title = name + " on Krate";
    $("oName").textContent = name;
    setText("cbarOrg", name);
    drawTile();
    var r = K.ROLE_NAME[roleOf()] || roleOf();
    $("oMeta").textContent = "You are " + (/^[AEIOU]/.test(r) ? "an " : "a ") + r.toLowerCase();
    var who = me();
    setText("csWho", who.label || "Account");
    setText("csAv", String(who.label || "?").replace(/^@/, "").slice(0, 1).toUpperCase());
    $("in").hidden = false;
    if (q.get("new") === "1") toast(name + " is ready, and you are its owner. Getting started has what is left to do.");
    // Sent while the two-person rule holds: who approves it now.
    if (q.get("sent")) toast(q.get("sent").slice(0, 80) + " was sent to " + name + ". It waits here until a second person approves it.");
    // Back from a company sign-in: what it did, in words.
    var signedVia = K.takeSsoNotice(undefined, name);
    if (signedVia) toast(signedVia);
    // An organisation made without a time zone (by the terminal, or before
    // F-319) takes its owner's, once: its days then start at their midnight.
    // The panes wait for it, so Insights counts by the new zone at once.
    var zoned = roleOf() === "owner" && S.org.time_zone_chosen === false && K.zone()
      ? K.api("PUT", "/orgs/" + slug + "/time-zone", { time_zone: K.zone() }, { key: K.key() }).then(function (r) {
          if (r.ok) { S.org.time_zone = r.body.time_zone; S.org.time_zone_chosen = true; }
        }, function () {})
      : Promise.resolve();
    zoned.then(function () {
      if (Pub) Pub.load();
      show(S.tab);
      countWaiting();
    });
  });
})();
