/* Publishers, release rule (IC-958 A and the one-minute addendum), the
 * dashboard's side:
 *
 *  - a developer waiting for an owner's computer to approve them as a
 *    publisher is told so, with who can, and a note to send them;
 *  - owners and admins see who is waiting (a count on People), and, when
 *    the organisation approves new publishers by hand, approve each in
 *    one click (signed by that owner's computer the next time it runs
 *    Krate);
 *  - Settings: "Approve new publishers by hand", and whether an owner may
 *    release their own apps while nobody else can approve (solo release).
 *
 * The routes are the signing ring's, built beside this page:
 *   GET  /orgs/<slug>/publishers                 {by_hand, waiting[], approvers[], me?}
 *   POST /orgs/<slug>/publishers/<principal>/approve
 *   PUT  /orgs/<slug>/publishers/by-hand         {by_hand}
 *   PUT  /orgs/<slug>/release-rule               {two_person}
 * and org.release_rule ("solo" | "two-person") on GET /orgs/<slug>. A hub
 * without them answers 404 or leaves the field out, and nothing here is
 * drawn.
 */
(function (root) {
  "use strict";

  root.KrateOrgPublishers = function (D) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var nameOf = function () { return (D.S.org && D.S.org.name) || slug; };
    var high = function () { var r = D.roleOf(); return r === "owner" || r === "admin"; };
    var state = null; // the hub's answer, or null where it has none
    var loaded = null;

    /// "Alice and 2 others", "Alice or Bob".
    function whoCan(list) {
      var names = (list || []).map(function (p) { return K.who(p).label; });
      if (!names.length) return "an owner";
      if (names.length === 1) return names[0];
      if (names.length === 2) return names[0] + " or " + names[1];
      return names[0] + " and " + (names.length - 1) + " others";
    }

    function load() {
      return (loaded = K.api("GET", path("/publishers")).then(function (res) {
        state = res.ok && res.body && typeof res.body === "object" ? res.body : null;
        var waiting = state && Array.isArray(state.waiting) ? state.waiting.filter(function (w) { return !w.approved_by; }) : [];
        D.S.pubWaiting = high() ? waiting.length : 0;
        D.drawTabs();
        notice(document.getElementById("pubNote"), D.S.tab);
        signWaiting();
        return state;
      }));
    }

    /// An owner's or admin's browser that holds the signing key does what
    /// their computer running Krate does: approve the people waiting to
    /// publish (G5, so a team that only uses the website is not stuck).
    /// Quiet; said only when it approved someone. Never for a browser that
    /// is not already one of the organisation's devices.
    var signing = false;
    var tried = {};
    function signWaiting() {
      if (signing || !state || !high() || !D.device) return;
      // Each waiting person once per page: one this browser cannot sign
      // for is left to a computer that can, not asked about in a loop.
      var due = (state.waiting || []).filter(function (w) { return (!state.by_hand || w.approved_by) && !tried[w.principal]; });
      if (!due.length) return;
      due.forEach(function (w) { tried[w.principal] = true; });
      signing = true;
      Promise.resolve(D.device()).then(function (dev) {
        return dev && typeof dev.duties === "function" ? dev.duties(slug) : null;
      }).then(function (did) {
        signing = false;
        var names = (did && did.approved) || [];
        if (!names.length) return;
        D.toast(names.length === 1 ? K.who(names[0]).label + " can publish to " + nameOf() + " now." : names.length + " people can publish to " + nameOf() + " now.");
        load();
      }, function () { signing = false; });
    }

    /// Above the pane: the developer's own wait, or the owners' count.
    function notice(box, tab) {
      if (!box) return;
      box.textContent = "";
      box.hidden = true;
      if (!state || (tab !== "home" && tab !== "releases" && tab !== "waiting")) return;
      var me = state.me;
      if (me && me.state === "waiting") {
        var msg = "Could you open Krate on your computer? It approves me as a publisher for " + nameOf() + " by itself.";
        var cp = el("button", { class: "btn ghost sm", type: "button", text: "Copy a note to send them" });
        cp.addEventListener("click", function () { K.copy(msg, cp); });
        box.appendChild(el("div", { class: "call warn org-why banner", role: "status" }, [el("div", {}, [
          el("b", { text: "Waiting for an owner's computer to approve you as a publisher" }),
          el("p", { text: state.by_hand
            ? whoCan(state.approvers) + " can approve you here in the dashboard; their computer signs it the next time it runs Krate. Your app goes out right after."
            : whoCan(state.approvers) + " can approve you. It happens by itself the next time one of them opens Krate or this console, and your app goes out right after." }),
          el("div", { class: "bacts" }, [cp]),
        ])]));
        box.hidden = false;
        return;
      }
      if (high() && D.S.pubWaiting) {
        var n = D.S.pubWaiting;
        var open = el("button", { class: "btn ghost sm", type: "button", text: "See who", onclick: function () { D.show("people"); } });
        box.appendChild(el("div", { class: "call org-why banner", role: "status" }, [el("div", {}, [
          el("b", { text: n === 1 ? "1 person is waiting to publish" : n + " people are waiting to publish" }),
          el("p", { text: state.by_hand
            ? nameOf() + " approves new publishers by hand: say yes in People, and your computer signs it the next time it runs Krate."
            : "Any owner's or admin's computer running Krate approves them by itself. Opening Krate Studio is enough." }),
          el("div", { class: "bacts" }, [open]),
        ])]));
        box.hidden = false;
      }
    }

    /// On People, for owners and admins: who is waiting, and Approve when
    /// approving is by hand.
    function peopleCard(pane) {
      var slot = el("div");
      pane.appendChild(slot);
      (loaded || load()).then(function () { waitingCard(slot); });
    }

    function waitingCard(pane) {
      if (!state || !high()) return;
      var waiting = (state.waiting || []).filter(function (w) { return !w.approved_by; });
      var approved = (state.waiting || []).filter(function (w) { return w.approved_by; });
      if (!waiting.length && !approved.length) return;
      var card = el("div", { class: "card rows", id: "pubCard" }, [el("p", { class: "grp", text: "Waiting to publish" })]);
      waiting.concat(approved).forEach(function (w) {
        var p = K.who(w.principal);
        var note = el("p", { class: "note", hidden: true });
        var acts = null;
        var said = w.approved_by
          ? "Approved by " + K.who(w.approved_by).label + ". Their computer signs it the next time it runs Krate."
          : state.by_hand ? "Asked " + K.moment(w.asked_at) + "." : "Asked " + K.moment(w.asked_at) + ". An owner's computer approves it by itself.";
        if (!w.approved_by && state.by_hand) {
          var b = el("button", { class: "btn dark sm", type: "button", text: "Approve " + p.label });
          b.addEventListener("click", function () {
            b.disabled = true;
            b.textContent = "Approving";
            K.api("POST", path("/publishers/" + encodeURIComponent(w.principal) + "/approve"), {}, { key: K.key() }).then(function (res) {
              if (res.ok) {
                D.toast(p.label + " is approved. Your computer signs it the next time it runs Krate.");
                load().then(function () { D.show("people"); });
                return;
              }
              b.disabled = false;
              b.textContent = "Approve " + p.label;
              var x = D.words(res, { what: "approve publishers", whoCan: "owners and admins" });
              note.textContent = x.title + ". " + x.body;
              note.hidden = false;
            });
          });
          acts = el("div", { class: "ra" }, [b]);
        }
        card.appendChild(el("div", { class: "rw" }, [
          el("span", { class: "who", "aria-hidden": "true", text: p.display.slice(0, 1).toUpperCase() }),
          el("div", { class: "t" }, [el("b", { text: p.label }), el("small", { text: said }), note]),
          acts,
        ]));
      });
      pane.appendChild(card);
    }

    /// Settings: approving publishers by hand, and the release rule.
    function settings(pane) {
      var o = D.S.org || {};
      var owner = D.roleOf() === "owner";
      if (o.release_rule === "solo" || o.release_rule === "two-person") {
        var solo = o.release_rule === "solo";
        var box = el("input", { type: "checkbox", id: "twoPerson" });
        box.checked = !solo;
        box.disabled = !owner || !solo;
        var note = el("p", { class: "note", hidden: true });
        box.addEventListener("change", function () {
          box.disabled = true;
          K.api("PUT", path("/release-rule"), { two_person: box.checked }, { key: K.key() }).then(function (res) {
            if (res.ok) { D.toast("From now on, every release in " + nameOf() + " needs a second person."); o.release_rule = "two-person"; return; }
            box.checked = false;
            box.disabled = false;
            var x = D.words(res, { what: "change who approves", whoCan: "an owner" });
            note.textContent = x.title + ". " + x.body;
            note.hidden = false;
          });
        });
        pane.appendChild(el("div", { class: "card", id: "ruleCard" }, [
          el("h3", { text: "Who approves a release" }),
          el("p", { text: solo
            ? "Nobody else in " + nameOf() + " can approve yet, so you release your own apps, and each one is marked as released by its sender. The moment a reviewer, an admin or a second owner joins, every release needs a second person."
            : "Every release needs a second person: whoever sends an app never approves it." }),
          el("label", { class: "chk", style: "margin-top:12px" }, [box, el("span", { text: solo ? "Require a second person from now on" : "A second person approves every release" })]),
          note,
        ]));
      }
      // The publishers' answer may still be on its way on a page opened
      // straight on Settings: the card goes where it belongs when it comes.
      var slot = el("div");
      pane.appendChild(slot);
      (loaded || load()).then(function () { byHand(slot); });
    }

    function byHand(slot) {
      var owner = D.roleOf() === "owner";
      if (!state || typeof state.by_hand !== "boolean" || !high()) return;
      var hand = el("input", { type: "checkbox", id: "pubByHand" });
      hand.checked = state.by_hand;
      hand.disabled = !owner;
      var hnote = el("p", { class: "note", hidden: true });
      hand.addEventListener("change", function () {
        hand.disabled = true;
        var want = hand.checked;
        K.api("PUT", path("/publishers/by-hand"), { by_hand: want }, { key: K.key() }).then(function (res) {
          hand.disabled = false;
          if (res.ok) { state.by_hand = want; D.toast(want ? "New publishers now wait for an owner to approve them here." : "New publishers are approved by an owner's computer by itself."); return; }
          hand.checked = !want;
          var x = D.words(res, { what: "change how publishers are approved", whoCan: "an owner" });
          hnote.textContent = x.title + ". " + x.body;
          hnote.hidden = false;
        });
      });
      slot.appendChild(el("div", { class: "card", id: "pubSetCard" }, [
        el("h3", { text: "New publishers" }),
        el("p", { text: "When a developer first sends an app to " + nameOf() + ", an owner's or admin's computer running Krate approves them by itself. Turn this on to approve each one here first." }),
        el("label", { class: "chk", style: "margin-top:12px" }, [hand, el("span", { text: "Approve new publishers by hand" })]),
        hnote,
      ]));
    }

    return { load: load, notice: notice, peopleCard: peopleCard, settings: settings };
  };
})(typeof window !== "undefined" ? window : globalThis);
