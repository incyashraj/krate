/* Find anything, and what is new (the org console's top bar, 2026-10-16).
 *
 * Search (the button, or Cmd/Ctrl+K): every page this person has, the
 * organisation's apps and its people, and the few things people come to
 * do, narrowed as they type; arrows and Enter, or a click. The bell: what
 * waits on this person and what happened to what they sent, from
 * GET /orgs/<slug>/notifications, opened in the right-hand panel; the page
 * remembers when it last looked, so the dot means something new.
 */
(function (root) {
  "use strict";
  var K = root.KrateOrg;
  var el = K.el;

  root.KrateOrgFind = function (D) {
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var apps = null;

    // --------------------------------------------------------------- search

    function things() {
      var role = D.roleOf();
      var out = D.tabs().map(function (id) { return { kind: "Page", text: D.tabName(id), go: function () { D.show(id); } }; });
      if (K.can(role, "invite.other")) out.push({ kind: "Do", text: "Invite people", go: function () { D.show("people", { invite: true }); } });
      if (K.can(role, "candidate.publish") && D.releaseFlow) out.push({ kind: "Do", text: "Release an app", go: function () { D.releaseFlow(); } });
      if (D.tabs().indexOf("build") >= 0) out.push({ kind: "Do", text: "Make a new app", go: function () { D.show("build"); } });
      out.push({ kind: "Do", text: "Ask for an app", go: function () { D.show("requests"); } });
      var own = D.tabs().indexOf("apps") >= 0 || D.tabs().indexOf("myapps") >= 0;
      (apps || []).forEach(function (a) {
        out.push({ kind: "App", text: a.name || a.app_id, go: function () { if (own) D.openApp(a.app_id); else D.show("home"); } });
      });
      if (D.tabs().indexOf("people") >= 0) (D.S.members || []).forEach(function (m) {
        if ((m.state || "active") !== "active") return;
        out.push({ kind: "Person", text: K.who(m).label, sub: K.ROLE_NAME[m.role] || m.role, go: function () { D.show("people"); } });
      });
      return out.filter(function (t) { return t.go; });
    }

    var dlg = null, input = null, list = null, shown = [], at = 0;
    function build() {
      input = el("input", { class: "inp", type: "search", placeholder: "Search pages, apps and people", "aria-label": "Search", autocomplete: "off" });
      list = el("div", { class: "fd-list", role: "listbox", "aria-label": "Results" });
      dlg = el("dialog", { class: "fdlg", id: "findDlg", "aria-label": "Search" }, [el("div", { class: "fd-in" }, [input]), list]);
      input.addEventListener("input", draw);
      input.addEventListener("keydown", function (e) {
        if (e.key === "ArrowDown") { at = Math.min(shown.length - 1, at + 1); mark(); e.preventDefault(); }
        else if (e.key === "ArrowUp") { at = Math.max(0, at - 1); mark(); e.preventDefault(); }
        else if (e.key === "Enter" && shown[at]) { e.preventDefault(); pick(shown[at]); }
      });
      dlg.addEventListener("click", function (e) { if (e && e.target === dlg) close(); });
      document.body.appendChild(dlg);
    }
    function draw() {
      var q = String(input.value || "").trim().toLowerCase();
      var all = things();
      shown = (q ? all.filter(function (t) { return (t.text + " " + (t.sub || "") + " " + t.kind).toLowerCase().indexOf(q) >= 0; }) : all).slice(0, 12);
      at = 0;
      list.textContent = "";
      if (!shown.length) { list.appendChild(el("p", { class: "fd-none", text: "Nothing called that in " + ((D.S.org && D.S.org.name) || slug) + "." })); return; }
      shown.forEach(function (t, i) {
        list.appendChild(el("button", { class: "fd-row", type: "button", role: "option", onclick: function () { pick(t); }, onmouseenter: function () { at = i; mark(); } }, [
          el("span", { class: "fd-k", text: t.kind }), el("b", { text: t.text }), t.sub ? el("small", { text: t.sub }) : null,
        ]));
      });
      mark();
    }
    function mark() {
      Array.prototype.slice.call(list.children).forEach(function (n, i) { if (n.setAttribute) n.setAttribute("aria-selected", i === at ? "true" : "false"); });
    }
    function pick(t) { close(); t.go(); }
    function close() { if (dlg && dlg.close) dlg.close(); }
    function open() {
      if (!dlg) build();
      input.value = "";
      draw();
      if (dlg.showModal) dlg.showModal();
      if (input.focus) input.focus();
      if (apps === null) {
        apps = [];
        K.api("GET", path("/apps")).then(function (res) {
          apps = (res.ok && res.body && res.body.apps) || [];
          if (dlg.getAttribute && dlg.getAttribute("open") !== null) draw();
        });
      }
    }

    // ---------------------------------------------------------------- bell

    var KEY = "krate-org-seen-" + slug;
    function seen() { try { return Number(root.localStorage.getItem(KEY)) || 0; } catch (e) { return 0; } }
    function look() { try { root.localStorage.setItem(KEY, String(Math.floor(Date.now() / 1000))); } catch (e) {} }
    var bell = null;
    function dot(n) {
      if (!bell) return;
      bell.classList[n ? "add" : "remove"]("has");
      bell.setAttribute("aria-label", n ? "Notifications, " + n + " new" : "Notifications");
    }
    function check() {
      K.api("GET", path("/notifications?since=" + seen())).then(function (res) {
        dot(res.ok && res.body && res.body.counts ? Number(res.body.counts.unread_hint) || 0 : 0);
      });
    }
    var WORD = { "approval-waiting": "Waiting for you", "version-decided": "Your version", request: "A request", security: "Security" };
    function notices() {
      var since = seen();
      K.api("GET", path("/notifications?since=" + since)).then(function (res) {
        if (!res.ok) {
          D.inspect("Notifications", [el("p", { class: "cr-sub", text: res.status === 404 ? "Notifications arrive with the next hub update." : "They did not load. Try again in a minute." })]);
          return;
        }
        var items = (res.body && res.body.items) || [];
        look();
        dot(0);
        D.inspect("Notifications", [items.length
          ? el("div", { class: "nlist" }, items.map(function (n) {
              return el("button", { class: "nrow" + (n.at > since ? " new" : ""), type: "button", onclick: function () { D.show(D.tabs().indexOf(n.tab) >= 0 ? n.tab : "home"); } }, [
                el("small", { text: (WORD[n.kind] || "") + " · " + K.moment(n.at) }), el("span", { text: n.text }),
              ]);
            }))
          : el("p", { class: "cr-sub", text: "Nothing new. When a version waits for you, or something you sent is decided, it shows here." }), prefs()]);
      });
    }

    /// What is also emailed: only what the hub mails, to whom it mails it.
    var MAILED = [
      ["approval-waiting", "A version waits for my approval", "candidate.approve"],
      ["version-decided", "My version was approved or declined", "candidate.publish"],
      ["request", "Someone asks for an app", "request.manage"],
    ];
    function prefs() {
      var role = D.roleOf();
      var mine = MAILED.filter(function (m) { return K.can(role, m[2]); });
      if (!mine.length) return null;
      var box = el("div", { class: "cr-sec" }, [el("h3", { text: "Also email me when" })]);
      K.api("GET", path("/me/notify")).then(function (res) {
        if (!res.ok) { box.hidden = true; return; }
        var p = (res.body && res.body.prefs) || {};
        mine.forEach(function (m) {
          var tick = el("input", { type: "checkbox", "aria-label": m[1] });
          tick.checked = !!(p[m[0]] && p[m[0]].email);
          tick.addEventListener("change", function () {
            var body = {}; body[m[0]] = { email: !!tick.checked };
            K.api("PUT", path("/me/notify"), { prefs: body }).then(function (out) {
              if (!out.ok) { tick.checked = !tick.checked; D.toast && D.toast("That did not save. Try again."); }
            });
          });
          box.appendChild(el("label", { class: "ntick" }, [tick, el("span", { text: m[1] })]));
        });
      });
      return box;
    }

    function wire(findBtn, bellBtn) {
      if (findBtn) findBtn.addEventListener("click", open);
      if (bellBtn) { bell = bellBtn; bellBtn.addEventListener("click", notices); check(); }
      if (root.document && root.document.addEventListener) root.document.addEventListener("keydown", function (e) {
        if ((e.metaKey || e.ctrlKey) && String(e.key).toLowerCase() === "k") { e.preventDefault(); open(); }
      });
    }

    return { wire: wire, open: open, notices: notices };
  };
})(typeof window !== "undefined" ? window : globalThis);
