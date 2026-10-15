/* The organisation dashboard's Apps tab (IC-954): every app the
 * organisation has, with the record IT asks for -- who owns it, who last
 * sent it, how it was made, what it can do, the company connections it
 * uses, who it is for, how many people opened it in the last 30 days (and,
 * for owners and admins, who), and its state. An app whose owner was
 * removed comes first and says so, with the one control that fixes it.
 * The whole list downloads as a spreadsheet.
 *
 * dashboard.js owns the page and hands this part what it needs (D). It
 * asks the hub when it is drawn, draws only the controls the role matrix
 * gives this person, and says refusals in words.
 */
(function (root) {
  "use strict";

  root.KrateOrgInventory = function (D) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var nameOf = function () { return (D.S.org && D.S.org.name) || slug; };
    var can = function (action) { return K.can(D.roleOf(), action) === "yes"; };

    function line(res, ctx) {
      var w = D.words(res, ctx);
      return w.title + ". " + w.body;
    }

    var STATE = {
      released: { text: "In use", tag: "ok" },
      waiting: { text: "Waiting for approval", tag: "" },
      withdrawn: { text: "Withdrawn", tag: "warn" },
      none: { text: "Nothing released", tag: "" },
    };

    function forWords(a) {
      return Array.isArray(a.audience) && a.audience.length
        ? "Only for " + K.groupsSaid(a.audience)
        : "For everyone in " + nameOf();
    }

    function peopleWords(a) {
      var n = a.people_30_days || 0;
      var said = n === 0 ? "Nobody opened it in the last 30 days" : (n === 1 ? "1 person" : n + " people") + " opened it in the last 30 days";
      if (Array.isArray(a.people) && a.people.length) {
        said += ": " + a.people.map(function (p) { return K.who(p.principal).label; }).join(", ");
      }
      return said;
    }

    function canWords(a) {
      var words = (a.capabilities || []).map(function (c) { return c.words || c.cap; }).filter(Boolean);
      return words.length ? "Can " + words.join("; ") : "Asks for nothing on a computer";
    }

    // ---------------------------------------------------------------- pane

    function pane(p) {
      K.api("GET", path("/inventory")).then(function (res) {
        var acts = [];
        if (D.releaseFlow && K.can(D.roleOf(), "candidate.publish") === "yes") acts.push({ label: "Release an app", dark: true, onclick: function () { D.releaseFlow(); } });
        D.head(p, "Apps", "Every app " + nameOf() + " has: who owns it, how it was made, what it can do and who uses it.", acts);
        if (!res.ok) return D.failed(p, res, { what: "see the organisation's apps", whoCan: "owners, admins and reviewers" });
        var apps = (res.body && res.body.apps) || [];
        if (!apps.length) {
          p.appendChild(el("div", { class: "card rows" }, [el("div", { class: "empty" }, [
            el("b", { text: "No apps yet" }),
            el("p", { text: "When someone sends an app to " + nameOf() + " -- in Krate Studio, Share, then Send to " + nameOf() + " for approval -- it appears here with who made it and who uses it." }),
          ])]));
          return;
        }
        var grid = el("div", { class: "invgrid" });
        p.appendChild(exportBar(apps, grid));
        p.appendChild(grid);
        apps.forEach(function (a) { var c = appCard(a, p); c.dataset.state = a.needs_owner ? "needs" : a.state || "none"; grid.appendChild(c); });
      });
    }

    /// The one action for the whole list: take it to a spreadsheet.
    function exportBar(apps, grid) {
      var note = el("p", { class: "note", role: "status", hidden: true });
      var b = el("button", { class: "btn ghost sm", type: "button", text: "Download the list as a spreadsheet" });
      b.addEventListener("click", function () {
        b.disabled = true;
        b.textContent = "Preparing the spreadsheet";
        K.download(K.hub() + path("/inventory.csv"), slug + "-apps.csv").then(function (out) {
          b.disabled = false;
          b.textContent = "Download the list as a spreadsheet";
          note.textContent = out.ok ? "Saved as " + slug + "-apps.csv, one row per app." : line({ body: out.body }, { what: "download the list" });
          note.hidden = false;
        });
      });
      var needs = apps.filter(function (a) { return a.needs_owner; }).length;
      // Narrow it down: by what needs doing, and by name.
      var KINDS = [["all", "All"], ["needs", "Needs an owner"], ["released", "In use"], ["waiting", "Waiting"], ["withdrawn", "Withdrawn"]];
      var kind = "all", words = "";
      function filter() {
        Array.prototype.slice.call(grid.children || []).forEach(function (c) {
          var st = c.dataset ? c.dataset.state : "";
          var okKind = kind === "all" || st === kind;
          var okWords = !words || String(c.textContent || "").toLowerCase().indexOf(words) >= 0;
          c.hidden = !(okKind && okWords);
        });
      }
      var chips = el("div", { class: "chips", role: "group", "aria-label": "Show" }, KINDS.filter(function (k) {
        return k[0] === "all" || apps.some(function (a) { return (a.needs_owner ? "needs" : a.state || "none") === k[0]; });
      }).map(function (k, i, list) {
        var n = k[0] === "all" ? apps.length : apps.filter(function (a) { return (a.needs_owner ? "needs" : a.state || "none") === k[0]; }).length;
        return el("button", { class: "chip", type: "button", "aria-pressed": i === 0 ? "true" : "false", text: k[1] + " " + n, onclick: function () {
          kind = k[0];
          Array.prototype.slice.call(chips.children).forEach(function (c, j) { c.setAttribute("aria-pressed", list[j][0] === kind ? "true" : "false"); });
          filter();
        } });
      }));
      var find = el("input", { class: "inp psearch", type: "search", placeholder: "Search apps", "aria-label": "Search apps", autocomplete: "off" });
      find.addEventListener("input", function () { words = find.value.trim().toLowerCase(); filter(); });
      return el("div", { class: "invbar" }, [
        el("div", { class: "evtools" }, [chips, apps.length > 3 ? find : null]),
        el("div", { class: "ptools" }, [
          el("p", { class: "sum", text: apps.length + (apps.length === 1 ? " app" : " apps") + (needs ? " · " + needs + " need" + (needs === 1 ? "s" : "") + " an owner" : "") }),
          b,
        ]),
        note,
      ]);
    }

    function appCard(a, p) {
      var st = STATE[a.state] || STATE.none;
      var card = el("div", { class: "card rows appc inv" + (a.needs_owner ? " needs" : "") });
      var head = el("div", { class: "apph" + (D.inspect ? " opens" : "") }, [
        el("span", { class: "apptile", "aria-hidden": "true", text: String(a.name || a.app_id).slice(0, 1).toUpperCase() }),
        el("div", { class: "t" }, [
          el("h3", {}, [a.name || a.app_id, " ", el("span", { class: "tag " + st.tag, text: st.text })]),
          el("small", { text: (a.version ? "Version " + a.version : "No version yet") + " · " + forWords(a) }),
        ]),
      ]);
      // The app's head opens the whole of it on the right.
      if (D.openApp) head.addEventListener("click", function () { D.openApp(a.app_id); });
      else if (D.inspect) head.addEventListener("click", function () { inspectApp(a); });
      card.appendChild(head);
      card.appendChild(ownerRow(a, p));
      // What it is, one fact a line, each with its mark.
      var facts = [];
      function fact(svg, text, cls) {
        var ic = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        ic.setAttribute("viewBox", "0 0 16 16"); ic.setAttribute("width", "14"); ic.setAttribute("height", "14"); ic.setAttribute("aria-hidden", "true");
        ic.setAttribute("fill", "none"); ic.setAttribute("stroke", "currentColor"); ic.setAttribute("stroke-width", "1.4"); ic.setAttribute("stroke-linecap", "round"); ic.setAttribute("stroke-linejoin", "round");
        ic.innerHTML = svg;
        facts.push(el("div", { class: "af" + (cls ? " " + cls : "") }, [ic, el("span", { text: text })]));
      }
      fact('<path d="M3 13l6.5-6.5M9 3.5l3.5 3.5-2 2L7 5.5z"/>', (a.built ? "Made with " + a.built_words : "Made by hand") + (a.built && a.built.revisions ? ", changed " + a.built.revisions + (a.built.revisions === 1 ? " time" : " times") + " since" : ""));
      fact('<path d="M8 1.8l5 2v3.6c0 3.2-2.2 5.6-5 6.8-2.8-1.2-5-3.6-5-6.8V3.8z"/>', canWords(a));
      if (a.connections && a.connections.length) {
        fact('<path d="M6.5 9.5l3-3"/><path d="M5 7.5L3.6 8.9a2.5 2.5 0 0 0 3.5 3.5L8.5 11M11 8.5l1.4-1.4a2.5 2.5 0 0 0-3.5-3.5L7.5 5"/>', "Uses the " + a.connections.join(", ") + " connection" + (a.connections.length === 1 ? "" : "s") + ": the app never holds the credential");
      }
      fact('<path d="M3 8h8M8 4.5L11.5 8 8 11.5"/>', "Last sent by " + K.who(a.last_sent_by).label + (a.last_sent_at ? " on " + K.day(a.last_sent_at) : ""));
      fact('<circle cx="6" cy="5.5" r="2.3"/><path d="M2 13c.4-2.2 2-3.5 4-3.5s3.6 1.3 4 3.5"/><path d="M10.5 3.4a2.2 2.2 0 0 1 0 4.2M11.6 9.7c1.3.4 2.2 1.6 2.4 3.3"/>', peopleWords(a), "ppl");
      card.appendChild(el("div", { class: "afacts" }, facts));
      // The exact terms, for whoever wants them, behind Details.
      var details = [el("summary", { text: "Details" }), el("small", { text: "App id: " + a.app_id })];
      if (a.built && a.built.prompt) details.push(el("small", { text: "Asked for: “" + a.built.prompt + "”" }));
      if (a.built && a.built.finished_at) details.push(el("small", { text: "Built on " + K.day(a.built.finished_at) }));
      (a.capabilities || []).forEach(function (c) { details.push(el("small", { class: "dg", text: c.cap })); });
      card.appendChild(el("details", { class: "invdet" }, details));
      return card;
    }

    /// One app, in full, in the right-hand panel.
    function inspectApp(a) {
      var st = STATE[a.state] || STATE.none;
      var kv = [
        ["State", st.text],
        ["Version", a.version || "None yet"],
        ["For", forWords(a)],
        ["Owner", a.owner ? K.who(a.owner.principal).label + (a.needs_owner ? " (no longer here)" : "") : "Nobody"],
        ["Made", a.built ? "With " + a.built_words : "By hand"],
        ["Last sent", K.who(a.last_sent_by).label + (a.last_sent_at ? " on " + K.day(a.last_sent_at) : "")],
        ["Used", peopleWords(a)],
      ];
      var dl = el("dl", { class: "cr-kv" });
      kv.forEach(function (x) { dl.appendChild(el("dt", { text: x[0] })); dl.appendChild(el("dd", { text: x[1] })); });
      D.inspect(a.name || a.app_id, [
        dl,
        el("div", { class: "cr-sec" }, [el("h3", { text: "It can" }), el("p", { class: "cr-sub", text: canWords(a) })]),
        el("div", { class: "row2" }, [el("button", { class: "btn ghost sm", type: "button", text: "Its releases", onclick: function () { D.show("releases"); } })]),
        el("details", { class: "cr-tech" }, [el("summary", { text: "Technical details" }), el("dl", { class: "cr-kv" }, [
          el("dt", { text: "App id" }), el("dd", { class: "dg", text: a.app_id }),
        ].concat(a.built && a.built.prompt ? [el("dt", { text: "Asked for" }), el("dd", { text: a.built.prompt })] : []))]),
      ]);
    }

    function ownerRow(a, p) {
      var note = el("p", { class: "note", role: "status", hidden: true });
      var say = function (t) { note.textContent = t; note.hidden = !t; };
      var owner = a.owner ? K.who(a.owner.principal).label : "nobody";
      var facts = a.needs_owner
        ? [el("b", {}, [el("span", { class: "tag warn", text: "Needs an owner" })]),
           el("small", { text: (a.owner ? owner + ", who owned it, is no longer in " + nameOf() + "." : "Nobody owns it.") + " The owner is who your IT team asks about this app." })]
        : [el("b", { text: "Owned by " + owner }), el("small", { text: "Who your IT team asks about this app" })];
      facts.push(note);
      var row = el("div", { class: "rw flat own" }, [el("div", { class: "t" }, facts)]);
      if (!can("app.owner")) return row;
      // The members who can own it, the current owner first.
      var choices = (D.S.members || []).filter(function (m) { return m.state === "active" || !m.state; });
      if (!choices.length) return row;
      var sel = el("select", { class: "sel", "aria-label": "Owner of " + (a.name || a.app_id) });
      choices.forEach(function (m) {
        var o = el("option", { value: m.principal, text: K.who(m).label });
        if (a.owner && m.principal === a.owner.principal && !a.needs_owner) o.selected = true;
        sel.appendChild(o);
      });
      var go = el("button", { class: "btn dark sm", type: "button", text: "Make them the owner" });
      go.addEventListener("click", function () {
        var who = sel.value;
        go.disabled = true;
        say("Making " + K.who(who).label + " the owner");
        K.api("PUT", path("/apps/" + encodeURIComponent(a.app_id) + "/owner"), { principal: who }, { key: K.key() }).then(function (res) {
          go.disabled = false;
          if (!res.ok) { say(line(res, { what: "choose the owner", whoCan: "owners and admins" })); return; }
          D.toast(K.who(who).label + " owns " + (a.name || a.app_id) + " now. It is in the activity.");
          D.show("apps");
        });
      });
      var picker = el("div", { class: "ra manage" }, [sel, go]);
      if (a.needs_owner) {
        // The one thing this card needs: the picker is open.
        row.appendChild(picker);
        return row;
      }
      // An app with an owner keeps the change one click away, not open.
      picker.hidden = true;
      var change = el("button", { class: "btn ghost sm", type: "button", text: "Change owner", "aria-expanded": "false" });
      change.addEventListener("click", function () {
        picker.hidden = false;
        change.hidden = true;
        change.setAttribute("aria-expanded", "true");
      });
      row.appendChild(el("div", { class: "ra" }, [change]));
      row.appendChild(picker);
      return row;
    }

    return { pane: pane, ownerRow: ownerRow, canWords: canWords, peopleWords: peopleWords, forWords: forWords, STATE: STATE };
  };
})(typeof window !== "undefined" ? window : globalThis);
