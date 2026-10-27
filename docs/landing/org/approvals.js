/* The code behind an approval, the approval rules, and one version on its
 * own page (G15, G16, G22).
 *
 *   codeBox(c)        "What changed in the code" on a waiting card: the
 *                     hub's risk rows, each file added, removed or changed,
 *                     and its line diff, one file at a time. Read from
 *                     GET /orgs/<slug>/candidates/<digest>/changes; nothing
 *                     here guesses at code.
 *   standing(c)       where a version stands against the rules: "Needs one
 *                     more approval from security", and who approved.
 *   rulesCard(pane)   Settings: the approval rules, set by an owner.
 *   focus(pane, ...)  the approval link in the email: one version, with its
 *                     code and what it can do, Approve and Decline -- or, if
 *                     it was decided already, who decided and when.
 */
(function (root) {
  "use strict";
  var K = root.KrateOrg;
  var el = K.el;

  root.KrateOrgApprovals = function (D) {
    var slug = D.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var asked = {};
    function nameOf() { return (D.S.org && D.S.org.name) || slug; }
    function plural(n, one, many) { return n + " " + (n === 1 ? one : many); }
    function changes(digest) {
      if (!asked[digest]) asked[digest] = K.api("GET", path("/candidates/" + digest + "/changes"));
      return asked[digest];
    }

    // ------------------------------------------------------ where it stands
    function standing(c) {
      var a = c.approval;
      if (!a || (!a.rules && !(a.approvals || []).length)) return null;
      var who = (a.approvals || []).map(function (x) { return K.who(x.principal).label; });
      var kids = [
        el("span", { class: "pdot " + (a.satisfied ? "ok" : "warn") }, [el("i", { "aria-hidden": "true" }), a.words || "Approved by enough people"]),
        who.length ? el("small", { text: "Approved by " + who.join(", ") }) : null,
      ];
      (a.groups || []).forEach(function (g) {
        kids.push(el("small", { class: g.met ? "met" : "", text: (g.met ? "Has an approver from " : "Needs an approver from ") + g.group + ": " + g.why }));
      });
      return el("div", { class: "apol", role: "status" }, kids);
    }

    // ---------------------------------------------------- the code changes
    var STATUS = { added: "Added", removed: "Removed", changed: "Changed" };
    function codeBox(c, opts) {
      opts = opts || {};
      var box = el("section", { class: "cchg", "aria-label": "What changed in the code" }, [
        el("h4", { text: "What changed in the code" }),
        el("p", { class: "sub", text: "Reading the code" }),
      ]);
      changes(c.digest).then(function (res) {
        box.textContent = "";
        // An older hub, or a person who may not read it: nothing to say here.
        if (!res.ok) {
          if (res.status >= 500) { box.appendChild(el("h4", { text: "What changed in the code" })); box.appendChild(el("p", { class: "sub", text: "The code could not be read just now. Reload to try again." })); }
          else box.hidden = true;
          return;
        }
        draw(box, res.body, opts);
      });
      return box;
    }

    function summaryLine(r) {
      var t = r.totals || { files: 0, added_lines: 0, removed_lines: 0 };
      if (!r.source || !r.source.files) return "";
      var lines = "+" + t.added_lines + " −" + t.removed_lines;
      if (r.new_app) return "A new app: " + plural(t.files, "file", "files") + ", " + lines + ".";
      if (!t.files) return "Compared with " + (r.against && r.against.version ? r.against.version : "the version that opens now") + ": the code is the same.";
      return "Compared with " + (r.against && r.against.version ? r.against.version : "the version that opens now") + ": " + plural(t.files, "file", "files") + " changed, " + lines + (r.unchanged ? ", " + plural(r.unchanged, "file", "files") + " the same" : "") + ".";
    }

    function draw(box, r, opts) {
      box.appendChild(el("h4", { text: "What changed in the code" }));
      var line = summaryLine(r);
      if (line) box.appendChild(el("p", { class: "sub", text: line }));
      var risks = (r.risks || []);
      if (risks.length) {
        box.appendChild(el("ul", { class: "crisk", "aria-label": "What to look at" }, risks.map(function (x) {
          return el("li", { class: x.level === "high" ? "high" : "" }, [el("i", { "aria-hidden": "true" }), el("span", { text: x.text })]);
        })));
      }
      var files = r.files || [];
      if (!files.length) return;
      var list = el("div", { class: "cfiles" });
      var opened = [];
      files.forEach(function (f, i) {
        var counts = f.binary ? "binary" : f.skipped ? "not compared" : "+" + f.added_lines + " −" + f.removed_lines;
        var det = el("details", { class: "cfile" }, [
          el("summary", {}, [
            el("span", { class: "cst " + f.status, text: STATUS[f.status] || f.status }),
            el("span", { class: "cpath mono", text: f.path }),
            el("span", { class: "cnum mono", text: counts }),
          ]),
        ]);
        var built = false;
        var build = function () {
          if (built) return;
          built = true;
          det.appendChild(fileBody(f));
        };
        det.addEventListener("toggle", function () { if (det.open || det.getAttribute("open") !== null) build(); });
        if (opts.focus ? i < 12 && f.hunks && f.hunks.length : false) { det.setAttribute("open", ""); build(); }
        det.build = build;
        opened.push(det);
        list.appendChild(det);
      });
      if (files.length > 1) {
        var all = el("button", { class: "btn link sm", type: "button", text: "Open every file", onclick: function () {
          var open = all.textContent === "Open every file";
          opened.forEach(function (d) { if (open) { d.setAttribute("open", ""); d.build(); } else d.removeAttribute("open"); });
          all.textContent = open ? "Close every file" : "Open every file";
        } });
        box.appendChild(el("div", { class: "cfh" }, [el("span", { class: "sub", text: plural(files.length, "file", "files") }), all]));
      }
      box.appendChild(list);
      if (r.truncated) {
        box.appendChild(el("p", { class: "note", text: "Some changes are too long to show here (over " + Math.round(((r.limits && r.limits.total) || 409600) / 1024) + " KB in all). Every file is listed; open the app in Krate Studio to read the rest." }));
      }
    }

    function fileBody(f) {
      if (f.binary) return el("p", { class: "cnote", text: "A binary file: its contents are not shown." });
      if (f.skipped) return el("p", { class: "cnote", text: "Too big to compare here. Open the app in Krate Studio to read it." });
      if (f.omitted) return el("p", { class: "cnote", text: "Not shown: the changes above reached the limit of what this page shows." });
      if (!f.hunks || !f.hunks.length) return el("p", { class: "cnote", text: "Only line endings or nothing visible changed." });
      var rows = [];
      f.hunks.forEach(function (h) {
        rows.push(el("tr", { class: "hh" }, [el("td", { colspan: "3", text: h.new_lines ? "Lines " + h.new_start + " to " + (h.new_start + h.new_lines - 1) : "Removed lines " + h.old_start + " to " + (h.old_start + h.old_lines - 1) })]));
        var o = h.old_start, n = h.new_start;
        h.lines.forEach(function (l) {
          var sign = l.charAt(0), text = l.slice(1);
          var cls = sign === "+" ? "add" : sign === "-" ? "del" : "ctx";
          rows.push(el("tr", { class: cls }, [
            el("td", { class: "ln", text: sign === "+" ? "" : String(o) }),
            el("td", { class: "ln", text: sign === "-" ? "" : String(n) }),
            el("td", { class: "cd" }, [el("span", { class: "sg", "aria-label": sign === "+" ? "added" : sign === "-" ? "removed" : "", text: sign === " " ? " " : sign === "-" ? "−" : "+" }), text]),
          ]));
          if (sign !== "+") o += 1;
          if (sign !== "-") n += 1;
        });
      });
      var kids = [el("div", { class: "cdiff", role: "region", tabindex: "0", "aria-label": "Changes to " + f.path }, [el("table", { class: "dtab" }, [el("tbody", {}, rows)])])];
      if (f.truncated) kids.push(el("p", { class: "cnote", text: "Cut here: the rest of this file's changes are too long to show. Open the app in Krate Studio to read all of it." }));
      return el("div", { class: "cbody" }, kids);
    }

    // ------------------------------------------------------ the rules card
    function rulesCard(pane) {
      var card = el("div", { class: "card", id: "rulesCard" }, [el("h3", { text: "Approval rules" })]);
      pane.appendChild(card);
      var owner = K.can(D.roleOf(), "approval.policy") === "yes";
      Promise.all([
        K.api("GET", path("/settings/approvals")),
        owner ? K.api("GET", path("/groups")) : Promise.resolve({ ok: false }),
        owner ? K.api("GET", path("/apps")) : Promise.resolve({ ok: false }),
      ]).then(function (got) {
        var res = got[0];
        if (!res.ok) { card.hidden = true; return; }
        var v = res.body || {};
        var p = v.policy || { approvals: 1, sensitive_approvals: 1, reviewer_groups: [], security_group: null };
        if (!owner) {
          card.appendChild(el("p", { text: v.default ? "One person approves each version, never the person who sent it." : "Each version needs " + v.words + "." }));
          card.appendChild(el("p", { class: "sub", text: "An owner of " + nameOf() + " sets these." }));
          return;
        }
        var groups = got[1].ok ? ((got[1].body && (got[1].body.groups || got[1].body)) || []).map(function (g) { return g.name || g; }).filter(Boolean) : [];
        var apps = got[2].ok ? ((got[2].body && got[2].body.apps) || []) : [];
        card.appendChild(el("p", { text: "Who has to approve a version before everyone it is for can open it. The person who sent it never approves it." }));
        var state = { approvals: p.approvals, sensitive_approvals: p.sensitive_approvals, reviewer_groups: (p.reviewer_groups || []).slice(), security_group: p.security_group };
        function seg(label, key) {
          var b1, b2;
          var pick = function (n) { state[key] = n; b1.setAttribute("aria-pressed", n === 1 ? "true" : "false"); b2.setAttribute("aria-pressed", n === 2 ? "true" : "false"); };
          b1 = el("button", { type: "button", text: "One person", onclick: function () { pick(1); } });
          b2 = el("button", { type: "button", text: "Two people", onclick: function () { pick(2); } });
          pick(state[key]);
          return el("div", { class: "rule" }, [el("span", { class: "rl", text: label }), el("div", { class: "seg", role: "group", "aria-label": label }, [b1, b2])]);
        }
        function groupSelect(label, value, none) {
          var s = el("select", { class: "inp", "aria-label": label }, [el("option", { value: "", text: none })].concat(groups.map(function (g) { return el("option", { value: g, text: g }); })));
          s.value = value || "";
          return s;
        }
        card.appendChild(seg("Every version", "approvals"));
        card.appendChild(seg("A version that asks for the network, files or the camera, or adds a permission", "sensitive_approvals"));
        // One reviewer group for every app, and one per app on top.
        var every = (state.reviewer_groups.filter(function (r) { return r.app === "*"; })[0] || {}).group || "";
        var everySel = groupSelect("A reviewer from a group, for every app", every, "Anyone who approves");
        card.appendChild(el("div", { class: "rule" }, [el("span", { class: "rl", text: "One approver from a group, for every app" }), everySel]));
        var perApp = el("div", { class: "rapps" });
        function appRow(r) {
          var appSel = el("select", { class: "inp", "aria-label": "App" }, apps.map(function (a) { return el("option", { value: a.app_id, text: a.name || a.app_id }); }));
          if (r.app && !apps.some(function (a) { return a.app_id === r.app; })) appSel.appendChild(el("option", { value: r.app, text: r.app }));
          appSel.value = r.app || (apps[0] && apps[0].app_id) || "";
          var gSel = groupSelect("Group for this app", r.group, "Choose a group");
          var row = el("div", { class: "rapp" }, [appSel, gSel, el("button", { class: "btn ghost sm", type: "button", text: "Remove", onclick: function () { row.remove(); } })]);
          row.read = function () { return { app: appSel.value, group: gSel.value }; };
          perApp.appendChild(row);
        }
        state.reviewer_groups.filter(function (r) { return r.app !== "*"; }).forEach(appRow);
        card.appendChild(perApp);
        if (apps.length && groups.length) card.appendChild(el("button", { class: "btn link sm", type: "button", text: "Add a group for one app", onclick: function () { appRow({}); } }));
        var secSel = groupSelect("When a version connects to a new host", state.security_group, "Anyone who approves");
        card.appendChild(el("div", { class: "rule" }, [el("span", { class: "rl", text: "When a version connects to a new host, one approver from" }), secSel]));
        if (!groups.length) card.appendChild(el("p", { class: "sub", text: "Make a group in People to ask for an approver from it." }));
        var warn = el("div", { class: "rwarn" });
        function warnings(list) {
          warn.textContent = "";
          (list || []).forEach(function (w) { warn.appendChild(el("p", { class: "note", text: w + "." })); });
        }
        warnings(v.warnings);
        card.appendChild(warn);
        var say = el("small", { class: "lc-say", role: "status" });
        var save = el("button", { class: "btn dark sm", type: "button", text: "Save the rules", onclick: function () {
          var body = {
            approvals: state.approvals,
            sensitive_approvals: state.sensitive_approvals,
            reviewer_groups: (everySel.value ? [{ app: "*", group: everySel.value }] : []).concat(Array.prototype.slice.call(perApp.children || []).map(function (r) { return r.read(); }).filter(function (r) { return r.app && r.group; })),
            security_group: secSel.value || null,
          };
          save.disabled = true;
          K.api("PUT", path("/settings/approvals"), body, { key: K.key() }).then(function (out) {
            save.disabled = false;
            if (!out.ok) { var x = D.words(out, { what: "change the approval rules", whoCan: "an owner" }); say.textContent = x.title + ". " + x.body; return; }
            say.textContent = "";
            warnings(out.body.warnings);
            D.toast(out.body.default ? "Back to one approval from anyone who approves." : "Saved. From now on each version needs " + out.body.words + ".");
          });
        } });
        card.appendChild(el("div", { class: "row2" }, [save]));
        card.appendChild(say);
        card.appendChild(el("p", { class: "sub", text: "With these left as they are, an owner who is the only one who can approve still releases their own apps. Any other rules need someone besides the sender." }));
      });
      return card;
    }

    // ----------------------------------------------- one version, on its own
    function focus(pane, digest, all, card) {
      var c = all.filter(function (x) { return x.digest === digest; })[0];
      var back = el("button", { class: "btn ghost sm", type: "button", text: "Everything waiting", onclick: function () { D.show("waiting", { c: "" }); } });
      if (c && c.state === "waiting-for-approval") {
        pane.appendChild(card(c));
        pane.appendChild(el("div", { class: "row2" }, [back]));
        return;
      }
      var title, body;
      if (!c) {
        title = "This version is not here";
        body = "It may have been sent to another organisation, or the link was cut short. Everything waiting in " + nameOf() + " is one click away.";
      } else if (c.state === "declined") {
        title = "Already declined";
        body = K.who(c.declined_by).label + " declined " + K.appName(c) + " " + K.moment(c.declined_at) + (c.decline_reason ? ": “" + c.decline_reason + "”" : "") + ". Its sender sends a new version.";
      } else if (c.state === "withdrawn") {
        title = "Already withdrawn";
        body = K.appName(c) + " was approved and later withdrawn" + (c.withdraw_reason ? ": “" + c.withdraw_reason + "”" : "") + ". Nothing is left to approve.";
      } else {
        title = "Already approved";
        var by = (c.approved_with && c.approved_with.length ? c.approved_with : [c.approved_by]).map(function (p) { return K.who(p).label; });
        body = (c.solo ? K.who(c.approved_by).label + " released " + K.appName(c) + " themselves" : by.join(" and ") + " approved " + K.appName(c)) + " " + K.moment(c.approved_at) + ". It is released: everyone it is for can open it.";
      }
      pane.appendChild(el("div", { class: "card decided" }, [
        el("div", { class: "empty" }, [el("b", { text: title }), el("p", { text: body })]),
        el("div", { class: "row2" }, [back]),
      ]));
    }

    return { codeBox: codeBox, standing: standing, rulesCard: rulesCard, focus: focus };
  };
})(typeof window !== "undefined" ? window : globalThis);
