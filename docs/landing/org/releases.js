/* The organisation dashboard's release control and groups (IC-949,
 * FLEET-CONTRACT.md R): Releases by app, with the version that opens now,
 * the history, Withdraw (whose sheet says what offline access already given
 * out still does), Make current (the rollback) and who may open each
 * release; and the Groups card on People.
 *
 * dashboard.js owns the page and hands this part what it needs (D). Like
 * the rest of the page, it asks the hub when it is drawn, hides only what
 * the role matrix says a role can never use, and says refusals in words.
 */
(function (root) {
  "use strict";

  root.KrateOrgReleases = function (D) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var nameOf = function () { return (D.S.org && D.S.org.name) || slug; };
    var can = function (action) { return K.can(D.roleOf(), action) === "yes"; };
    var closing = function () { return D.S.org && D.S.org.state === "closing"; };
    var groupsCache = null;

    function line(res, ctx) {
      var w = D.words(res, ctx);
      return w.title + ". " + w.body;
    }

    /// The groups, asked once per drawing of a pane.
    function groups() {
      if (!groupsCache) {
        groupsCache = K.api("GET", path("/groups")).then(function (res) {
          return res.ok && res.body && Array.isArray(res.body.groups) ? res.body.groups : null;
        });
      }
      return groupsCache;
    }

    function audienceWords(r) {
      return Array.isArray(r.audience) && r.audience.length
        ? "Only for " + K.groupsSaid(r.audience)
        : "For everyone in " + nameOf();
    }

    // ------------------------------------------------------------ releases

    function pane(p) {
      groupsCache = null;
      K.api("GET", path("/apps")).then(function (res) {
        // A hub without release control answers 404 for /apps: the flat
        // list is what it has.
        if (!res.ok && res.status === 404 && res.body.error !== "no-such-org") { D.flat(p); return; }
        D.head(p, "Releases", "Approved apps, each with the version that opens now. A copied link alone opens nothing.",
          D.releaseFlow && can("candidate.publish") ? [{ label: "Release an app", dark: true, onclick: function () { D.releaseFlow(); } }] : null);
        if (!res.ok) return D.failed(p, res, { what: "see the releases" });
        var apps = (res.body && res.body.apps) || [];
        apps.forEach(function (a) { a.releases.forEach(function (r) { D.S.names[r.digest] = K.appName(r); }); });
        if (!apps.length) {
          var empty = el("div", { class: "card rows" });
          p.appendChild(empty);
          if (D.empty) { D.empty(empty); return; }
          empty.appendChild(el("div", { class: "empty" }, [el("b", { text: "Nothing released yet" }), el("p", { text: "When someone in " + nameOf() + " releases an app, it appears here, ready to open." })]));
          return;
        }
        apps.forEach(function (a) { p.appendChild(appCard(a, p)); });
      });
    }

    function appCard(a, p) {
      var card = el("div", { class: "card rows appc" });
      var cur = a.releases.filter(function (r) { return r.digest === a.current; })[0] || null;
      var released = a.releases.filter(function (r) { return r.state === "released"; });
      // The offline class is per app (Addendum 3 G1): once, in the head.
      var offNote = el("p", { class: "note", hidden: true });
      var offline = D.P && released.length ? D.P.releaseOffline(cur || released[0], offNote) : null;
      card.appendChild(el("div", { class: "apph" }, [
        el("div", { class: "t" }, [
          el("h3", { text: a.name || a.app_id }),
          offNote,
        ]),
        offline ? el("div", { class: "ra" }, [offline]) : null,
      ]));
      if (!cur) {
        card.appendChild(el("p", { class: "nocur", text: released.length
          ? "No version of " + (a.name || a.app_id) + " is set to open."
          : "Every version of " + (a.name || a.app_id) + " was withdrawn, so nothing of it opens." }));
      }
      // The version that opens now first, then the other released ones,
      // newest approval first, then the withdrawn.
      var rest = a.releases.filter(function (r) { return r !== cur; });
      var order = (cur ? [cur] : [])
        .concat(rest.filter(function (r) { return r.state !== "withdrawn"; }))
        .concat(rest.filter(function (r) { return r.state === "withdrawn"; }));
      var older = order.filter(function (r) { return r !== cur; });
      if (cur) card.appendChild(releaseRow(a, cur, true, p));
      if (older.length) {
        card.appendChild(el("p", { class: "grp", text: cur ? "Other versions" : "Versions" }));
        older.forEach(function (r) { card.appendChild(releaseRow(a, r, false, p)); });
      }
      return card;
    }

    function releaseRow(a, r, isCurrent, p) {
      var gone = r.state === "withdrawn";
      var note = el("p", { class: "note", hidden: true });
      var say = function (t) { note.textContent = t; note.hidden = !t; };
      var tag = gone
        ? el("span", { class: "tag warn", text: "Withdrawn" })
        : isCurrent ? el("span", { class: "tag ok", text: "Opens now" }) : null;
      var facts = [
        el("b", {}, [K.appName(r), tag ? " " : null, tag]),
        // Solo release (IC-958 addendum): said every time it is shown.
        el("small", { text: r.solo
          ? "Released by its sender, " + K.who(r.uploaded_by).label + " (no second approver yet) · " + K.day(r.at)
          : "Sent by " + K.who(r.uploaded_by).label + " · approved by " + K.who(r.approved_by).label + " · " + K.day(r.at) }),
      ];
      if (gone) {
        facts.push(el("small", { class: "why", text: "Withdrawn by " + K.who(r.withdrawn_by).label + " on " + K.day(r.withdrawn_at) + ": “" + (r.withdraw_reason || "no reason given") + "”" }));
      } else {
        facts.push(el("small", { class: "aud", text: audienceWords(r) }));
      }
      facts.push(note);
      var row = el("div", { class: "rw flat rel" + (gone ? " gone" : "") + (D.inspect ? " opens" : "") }, [el("div", { class: "t" }, facts)]);
      // A click on the row, not its buttons, opens the version on the right.
      if (D.inspect) row.addEventListener("click", function (e) {
        var t = e && e.target;
        if (t && t.closest && t.closest("select, button, a, input")) return;
        inspectRelease(a, r, isCurrent);
      });
      if (gone) return row;

      var acts = [];
      var open = el("button", { class: "btn dark sm", type: "button", text: "Open in Krate" });
      open.addEventListener("click", function () {
        open.disabled = true;
        open.textContent = "Getting it";
        var file = K.fileName(r.name);
        K.download(r.url, file).then(function (out) {
          open.disabled = false;
          open.textContent = "Open in Krate";
          if (out.ok) { say("Saved as " + file + ". Double-click it to open it in Krate."); return; }
          say(line({ body: out.body }, { what: "open it" }));
        });
      });
      var copyBtn = el("button", { class: "btn ghost sm", type: "button", text: "Copy link" });
      copyBtn.addEventListener("click", function () { K.copy(r.url, copyBtn); });
      acts.push(open, copyBtn);

      var manage = [];
      if (!isCurrent && can("release.set-current")) {
        manage.push(el("button", { class: "btn ghost sm", type: "button", text: "Make current", onclick: function () { makeCurrent(a, r); } }));
      }
      var audBox = el("div", { class: "audbox", hidden: true });
      if (can("release.audience")) {
        manage.push(el("button", { class: "btn ghost sm", type: "button", text: "Who can open it", onclick: function () { audience(r, audBox, say); } }));
      }
      if (can("release.withdraw")) {
        manage.push(el("button", { class: "btn quiet sm", type: "button", text: "Withdraw", onclick: function () { withdraw(a, r, isCurrent); } }));
      }
      row.appendChild(el("div", { class: "ra" }, acts));
      if (manage.length) row.appendChild(el("div", { class: "ra manage" }, manage));
      row.appendChild(audBox);
      return row;
    }

    /// The withdraw sheet: what stops at once, what offline access already
    /// given out still does until it runs out, and what opens instead.
    /// One version, in full, in the right-hand panel.
    function inspectRelease(a, r, isCurrent) {
      var gone = r.state === "withdrawn";
      var kv = [
        ["State", gone ? "Withdrawn" : isCurrent ? "Opens now" : "Released, not the one that opens"],
        ["Sent by", K.who(r.uploaded_by).label],
        ["Approved by", r.solo ? "Its sender, alone (no second approver yet)" : K.who(r.approved_by).label],
        ["Released", K.moment(r.at)],
        ["Who opens it", audienceWords(r).replace(/^For /, "")],
      ];
      if (r.offline) kv.push(["Offline", K.offlineName ? K.offlineName(r.offline) : r.offline]);
      if (gone) kv.push(["Withdrawn", K.day(r.withdrawn_at) + " by " + K.who(r.withdrawn_by).label + ": " + (r.withdraw_reason || "no reason given")]);
      var dl = el("dl", { class: "cr-kv" });
      kv.forEach(function (x) { dl.appendChild(el("dt", { text: x[0] })); dl.appendChild(el("dd", { text: x[1] })); });
      var copyBtn = el("button", { class: "btn ghost sm", type: "button", text: "Copy link" });
      copyBtn.addEventListener("click", function () { K.copy(r.url, copyBtn); });
      D.inspect(K.appName(r), [
        dl,
        gone ? null : el("div", { class: "row2" }, [copyBtn]),
        el("details", { class: "cr-tech" }, [
          el("summary", { text: "Technical details" }),
          el("dl", { class: "cr-kv" }, [
            el("dt", { text: "App" }), el("dd", { class: "dg", text: a.app_id }),
            el("dt", { text: "Fingerprint" }), el("dd", { class: "dg", text: r.digest }),
          ]),
        ]),
      ]);
    }

    function withdraw(a, r, isCurrent) {
      var key = K.key();
      var leases = r.leases || { active: 0, until: null };
      var held = leases.active > 0
        ? "Offline access already given out keeps working without a connection until it runs out: " +
          leases.active + (leases.active === 1 ? " person holds it" : " people hold it") + ", the last until " + K.moment(leases.until) + "."
        : r.offline === "online"
          ? "It is online only, so it stops everywhere at once."
          : "Nobody holds offline access to it, so it stops everywhere at once.";
      var next = a.releases.filter(function (x) { return x.state === "released" && x.digest !== r.digest; })[0];
      var after = isCurrent
        ? (next ? "It is the version that opens now; " + K.appName(next) + " opens instead." : "It is the only version, so nothing of " + (a.name || a.app_id) + " opens after this.")
        : null;
      D.confirmSheet({
        title: "Withdraw " + K.appName(r) + "?",
        body: "From now on nobody can download it or renew offline access to it, and a computer that has it refuses it the next time it connects.",
        points: [{ text: held, key: leases.active > 0 }, after, "This is not undone: to release it again, send a new version for approval."],
        digest: r.digest,
        input: { label: "Why? The people who had it see this", placeholder: "For example: it loses data on save" },
        check: function (v) {
          var t = String(v || "").trim();
          if (!t || t.length > 300) return K.refusal("reason-required", {});
          return null;
        },
        yes: "Withdraw it",
        busy: "Withdrawing",
        ctx: { what: "withdraw releases", whoCan: "owners, admins and reviewers" },
        act: function (v) { return K.api("POST", path("/releases/" + r.digest + "/withdraw"), { reason: String(v || "").trim() }, { key: key }); },
        done: function (res) {
          var c = res.body && res.body.current;
          var now = c && c.digest ? D.S.names[c.digest] || "another version" : null;
          D.toast(K.appName(r) + " is withdrawn." + (isCurrent ? (now ? " " + now + " opens now." : " Nothing of " + (a.name || a.app_id) + " opens now.") : ""));
          D.show("releases");
        },
      });
    }

    /// Make an older version the one that opens: the rollback.
    function makeCurrent(a, r) {
      var key = K.key();
      var cur = a.releases.filter(function (x) { return x.digest === a.current; })[0];
      D.confirmSheet({
        title: "Make " + K.appName(r) + " current?",
        body: "From now on, opening " + (a.name || a.app_id) + " from " + nameOf() + " gets " + (r.version || "this version") +
          (cur ? " instead of " + (cur.version || K.appName(cur)) + ", which stays released, so you can switch back." : "."),
        digest: r.digest,
        yes: "Make it current",
        busy: "Switching",
        ctx: { what: "choose which version opens", whoCan: "owners and admins" },
        act: function () { return K.api("PUT", path("/apps/" + encodeURIComponent(a.app_id) + "/current"), { digest: r.digest }, { key: key }); },
        done: function () {
          D.toast(K.appName(r) + " is the version that opens now.");
          D.show("releases");
        },
      });
    }

    /// Who may open one release: everyone, or some groups.
    function audience(r, box, say) {
      if (!box.hidden) { box.hidden = true; return; }
      box.textContent = "";
      box.appendChild(el("p", { class: "sub", text: "Loading" }));
      box.hidden = false;
      groups().then(function (list) {
        box.textContent = "";
        if (list === null) { box.appendChild(el("p", { class: "note", text: "The groups could not be read just now. Try again in a moment." })); return; }
        var limited = Array.isArray(r.audience) && r.audience.length > 0;
        var picked = {};
        (r.audience || []).forEach(function (g) { picked[g] = true; });
        var seg = el("div", { class: "seg", role: "group", "aria-label": "Who can open " + K.appName(r) });
        var checks = el("div", { class: "gchecks" });
        var out = el("div");
        function setMode(only) {
          limited = only;
          [].forEach.call(seg.children, function (b) { b.setAttribute("aria-pressed", (b.dataset.mode === "only") === only ? "true" : "false"); });
          checks.hidden = !only;
        }
        [["all", "Everyone in " + nameOf()], ["only", "Only some groups"]].forEach(function (m) {
          var b = el("button", { type: "button", text: m[1] });
          b.dataset.mode = m[0];
          b.addEventListener("click", function () { setMode(m[0] === "only"); });
          seg.appendChild(b);
        });
        if (!list.length) {
          checks.appendChild(el("p", { class: "sub", text: "There are no groups yet. Make one on People, then come back." }));
        }
        list.forEach(function (g) {
          var c = el("input", { type: "checkbox", id: "aud-" + r.digest.slice(0, 8) + "-" + g.name });
          c.checked = !!picked[g.name];
          c.addEventListener("change", function () { picked[g.name] = c.checked; });
          checks.appendChild(el("label", { class: "chk" }, [c, el("span", { text: g.name + " (" + g.members.length + (g.members.length === 1 ? " person)" : " people)") })]));
        });
        var save = el("button", { class: "btn dark sm", type: "button", text: "Save" });
        var key = null, sig = null;
        save.addEventListener("click", function () {
          var chosen = Object.keys(picked).filter(function (g) { return picked[g]; }).sort();
          out.textContent = "";
          if (limited && !chosen.length) {
            out.appendChild(K.refusalBlock({ title: "Pick at least one group", body: "Or choose Everyone in " + nameOf() + ".", action: null }, "warn"));
            return;
          }
          var body = limited ? { groups: chosen } : { everyone: true };
          var s = JSON.stringify(body);
          if (s !== sig) { sig = s; key = K.key(); }
          save.disabled = true;
          K.api("PUT", path("/releases/" + r.digest + "/audience"), body, { key: key }).then(function (res) {
            save.disabled = false;
            if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, { what: "choose who opens it", whoCan: "owners and admins" }), "bad")); return; }
            r.audience = res.body.audience;
            box.hidden = true;
            say("");
            D.toast(K.appName(r) + ": " + audienceWords(r).toLowerCase() + ". Owners, admins and reviewers always open it.");
            D.show("releases");
          });
        });
        var cancel = el("button", { class: "btn ghost sm", type: "button", text: "Cancel", onclick: function () { box.hidden = true; } });
        box.appendChild(el("p", { class: "sub", text: "Owners, admins and reviewers always open it, so they can check it." }));
        box.appendChild(seg);
        box.appendChild(checks);
        box.appendChild(out);
        box.appendChild(el("div", { class: "row2" }, [save, cancel]));
        setMode(limited);
      });
    }

    // -------------------------------------------------------------- groups

    /// The Groups card on People. `active` is the active members, for the
    /// Add list.
    function groupsCard(at, active) {
      K.api("GET", path("/groups")).then(function (res) {
        // A hub without groups: nothing to show.
        if (!res.ok && res.status === 404) return;
        at.textContent = "";
        var manage = can("group.manage") && !closing();
        var card = el("div", { class: "card", id: "groupsCard" }, [
          el("h3", { text: "Groups" }),
          el("p", { text: D.roleOf() === "recipient"
            ? "The groups of " + nameOf() + " you are in. Some apps open only for certain groups."
            : "Named sets of people, like finance. In Releases a version can be limited to groups; owners, admins and reviewers always open everything." }),
        ]);
        at.appendChild(card);
        at.hidden = false;
        if (!res.ok) { card.appendChild(K.refusalBlock(D.words(res, { what: "see the groups" }))); return; }
        var list = (res.body && res.body.groups) || [];
        var rows = el("div", { class: "rows glist" });
        if (!list.length) rows.appendChild(el("p", { class: "sub", text: manage ? "No groups yet. Make the first one below." : "No groups yet." }));
        list.forEach(function (g) { rows.appendChild(groupRow(g, active, manage)); });
        card.appendChild(rows);
        if (manage) card.appendChild(createForm());
      });
    }

    function groupRow(g, active, manage) {
      var note = el("p", { class: "note", hidden: true });
      var say = function (t) { note.textContent = t; note.hidden = !t; };
      var chips = el("div", { class: "chips" });
      if (!g.members.length) chips.appendChild(el("span", { class: "sub", text: "Nobody yet" }));
      g.members.forEach(function (p) {
        var w = K.who(p);
        var chip = el("span", { class: "chip member" }, [w.label]);
        if (manage) {
          var x = el("button", { class: "x", type: "button", "aria-label": "Take " + w.label + " out of " + g.name, text: "×" });
          x.addEventListener("click", function () {
            x.disabled = true;
            K.api("DELETE", path("/groups/" + encodeURIComponent(g.name) + "/members/" + encodeURIComponent(p)), null, { key: K.key() }).then(function (res) {
              x.disabled = false;
              if (!res.ok) { say(line(res, { what: "manage groups", whoCan: "owners and admins" })); return; }
              D.toast(w.label + " is out of " + g.name + ".");
              D.show("people");
            });
          });
          chip.appendChild(x);
        }
        chips.appendChild(chip);
      });
      var kids = [el("div", { class: "t" }, [el("b", { text: g.name }), chips, note])];
      if (manage) {
        var outside = active.filter(function (m) { return g.members.indexOf(m.principal) < 0; });
        if (outside.length) {
          var sel = el("select", { class: "sel", "aria-label": "Add someone to " + g.name });
          sel.appendChild(el("option", { value: "", text: "Add someone" }));
          outside.forEach(function (m) { sel.appendChild(el("option", { value: m.principal, text: K.who(m).label })); });
          var add = el("button", { class: "btn ghost sm", type: "button", text: "Add" });
          add.addEventListener("click", function () {
            if (!sel.value) { say("Pick who to add first."); return; }
            var p = sel.value, w = K.who(p);
            add.disabled = true;
            K.api("PUT", path("/groups/" + encodeURIComponent(g.name) + "/members/" + encodeURIComponent(p)), null, { key: K.key() }).then(function (res) {
              add.disabled = false;
              if (!res.ok) { say(line(res, { what: "manage groups", whoCan: "owners and admins" })); return; }
              D.toast(w.label + " is in " + g.name + ".");
              D.show("people");
            });
          });
          kids.push(el("div", { class: "ra" }, [sel, add]));
        }
      }
      return el("div", { class: "rw flat grow" }, kids);
    }

    function createForm() {
      var input = el("input", { class: "inp", id: "groupName", autocomplete: "off", autocapitalize: "none", spellcheck: "false", "aria-label": "New group name", placeholder: "New group, like finance" });
      var go = el("button", { class: "btn dark", type: "submit", text: "Make group" });
      var out = el("div");
      var pending = null;
      var form = el("form", { class: "gform", novalidate: true }, [input, go]);
      form.addEventListener("submit", function (e) {
        e.preventDefault();
        var name = input.value.trim().toLowerCase();
        out.textContent = "";
        if (!/^[a-z][a-z0-9-]{0,38}$/.test(name)) { out.appendChild(K.refusalBlock(K.refusal("group-invalid", {}), "warn")); return; }
        if (!pending || pending.name !== name) pending = { name: name, key: K.key() };
        go.disabled = true;
        K.api("POST", path("/groups"), { name: name }, { key: pending.key }).then(function (res) {
          go.disabled = false;
          if (!res.ok) { out.appendChild(K.refusalBlock(D.words(res, { what: "make groups", whoCan: "owners and admins" }), "bad")); return; }
          pending = null;
          D.toast("The group " + name + " is made. Add people to it below.");
          D.show("people");
        });
      });
      return el("div", {}, [form, out]);
    }

    return { pane: pane, groupsCard: groupsCard, appCard: appCard, audienceWords: audienceWords };
  };
})(typeof window !== "undefined" ? window : globalThis);
