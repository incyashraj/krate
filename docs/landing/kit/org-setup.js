/* Setting an organisation up from the website (IC-958 B and its one-minute
 * addendum), shared by /orgs/ (right after Create) and /org/ (the Getting
 * started list and the empty Releases tab):
 *
 *   firstApp   pick a ready app or upload a .krate; sign it in this browser,
 *              send it, release it (solo release while nobody else can
 *              approve), saying each step in words
 *   released   "Acme can use it now": the link, and the team invite
 *   invite     pasted emails, one role for the batch, sent by the hub's mail
 *   checklist  Getting started, ticking itself from what the hub says
 *   itPack     the organisation's policy for its IT team, made here
 *
 * What this page builds against and does not own (each hidden or replaced
 * by the Krate Studio path when it is missing, and listed in the IC-958
 * report):
 *   window.KrateOrgDevice (from /kit/org-device.js): the browser as the
 *     owner's first device. ready(slug, {name}) makes and keeps the
 *     organisation's signature in this browser and registers it with the
 *     hub; sign(slug, bytes) marks a .krate for the organisation and signs
 *     it. Rejections carry .reason: "not-holder", "unsupported", or other.
 *   Solo release on the hub: POST .../candidates/<digest>/approve by the
 *     sender succeeds while nobody else may approve, answering
 *     {release: {digest, url, solo: true}}; refused with self-approval
 *     otherwise, exactly as today.
 *   /orgs/starters.json: the ready apps (placeholders until they land).
 *
 * Plain functions, like the rest of the org pages; docs/landing/studio/
 * org-setup-test.mjs runs this file as it ships.
 */
(function (root) {
  "use strict";

  /// Personal mail: no company sign-in to offer for these domains.
  var PERSONAL = /^(gmail\.com|googlemail\.com|outlook\.com|hotmail\.[a-z.]+|live\.[a-z.]+|msn\.com|yahoo\.[a-z.]+|icloud\.com|me\.com|mac\.com|proton\.me|protonmail\.com|pm\.me|aol\.com|gmx\.[a-z.]+|yandex\.[a-z.]+|mail\.ru|qq\.com|163\.com|126\.com|fastmail\.com|hey\.com|zoho\.com)$/;
  var EMAIL = /^[^\s@,;<>]+@[^\s@,;<>]+\.[^\s@,;<>]+$/;
  var INVITE_ROLES = ["recipient", "developer", "reviewer"];

  /// The company domain an owner's address is at, or "" for personal mail.
  function companyDomain(email) {
    var m = String(email || "").toLowerCase().match(/@([a-z0-9.-]+\.[a-z]{2,})$/);
    return m && !PERSONAL.test(m[1]) ? m[1] : "";
  }

  /// Pasted addresses: one per line, or separated by commas, semicolons or
  /// spaces; "Sam <sam@acme.com>" reads as sam@acme.com. Lower-cased, each
  /// once, the ones that are not addresses kept apart.
  function parseEmails(text) {
    var good = [], bad = [], seen = {};
    // Entries are split by lines, commas and semicolons; inside one, every
    // address counts and the rest is a name ("Sam Lee <sam@acme.com>"). An
    // entry with no address at all is kept, as something to fix.
    String(text || "").split(/[\n\r,;]+/).forEach(function (entry) {
      var e = entry.trim();
      if (!e) return;
      var found = e.match(/[^\s@,;<>()"']+@[^\s@,;<>()"']+\.[^\s@,;<>()"'.]+/g);
      if (!found) { if (bad.indexOf(e) < 0) bad.push(e); return; }
      found.forEach(function (raw) {
        var t = raw.toLowerCase();
        if (!EMAIL.test(t) || seen[t]) return;
        seen[t] = true;
        good.push(t);
      });
    });
    return { emails: good, bad: bad };
  }

  root.KrateOrgSetup = function (C) {
    var K = root.KrateOrg;
    var el = K.el;
    var slug = C.slug;
    var path = function (p) { return "/orgs/" + slug + (p || ""); };
    var nameOf = function () { return (C.org() && C.org().name) || slug; };
    var doc = root.document;

    // ------------------------------------------------- remembered here
    // Per-viewer conveniences only (a dismissed list, a draft, what was
    // downloaded); the hub holds every real state.
    var SKEY = "krate-setup:" + slug;
    function mem() {
      try { return JSON.parse(root.localStorage.getItem(SKEY) || "{}") || {}; } catch (e) { return {}; }
    }
    function remember(patch) {
      var m = mem();
      for (var k in patch) m[k] = patch[k];
      try { root.localStorage.setItem(SKEY, JSON.stringify(m)); } catch (e) {}
      return m;
    }

    function site(p) {
      var origin = (root.location && root.location.origin) || "";
      return root.fetch(origin + p).then(function (r) { return r.ok ? r : Promise.reject(new Error("missing")); });
    }

    // --------------------------------------------- the browser's device
    var deviceP = null;
    /// This browser as a device of the organisation (KrateOrgDevice), or
    /// null where that has not shipped: loaded once, quietly, so a page
    /// without it shows the Krate Studio path instead of an error.
    function device() {
      if (deviceP) return deviceP;
      deviceP = new Promise(function (resolve) {
        if (root.KrateOrgDevice) { resolve(root.KrateOrgDevice); return; }
        if (!doc || !doc.head || typeof doc.createElement !== "function") { resolve(null); return; }
        var s = doc.createElement("script");
        s.src = "/kit/org-device.js";
        s.async = true;
        s.onload = function () { resolve(root.KrateOrgDevice || null); };
        s.onerror = function () { resolve(null); };
        doc.head.appendChild(s);
      });
      return deviceP;
    }

    var startersP = null;
    function starters() {
      if (!startersP) {
        startersP = site("/orgs/starters.json").then(function (r) { return r.json(); }).then(function (j) {
          return Array.isArray(j && j.starters) ? j.starters.filter(function (s) { return s && s.id && s.name && s.bundle; }) : [];
        }, function () { return []; });
      }
      return startersP;
    }

    /// Send a .krate's bytes to the organisation, as `krate publish --org`.
    function upload(bytes, key) {
      var headers = { authorization: "Bearer " + K.token(), "content-type": "application/octet-stream" };
      if (key) headers["idempotency-key"] = key;
      return root.fetch(K.hub() + path("/candidates"), { method: "POST", headers: headers, body: bytes }).then(function (r) {
        return r.text().then(function (t) {
          var b = null;
          try { b = t ? JSON.parse(t) : null; } catch (e) {}
          if (!r.ok && (!b || !b.error)) b = { error: r.status === 401 ? "not-signed-in" : r.status === 413 ? "too-large" : "hub-error", message: "" };
          return { ok: r.ok, status: r.status, body: b };
        });
      }, function () { return { ok: false, status: 0, body: { error: "offline", message: "" } }; });
    }

    function words(res, what) {
      var reason = res.body ? res.body.error : "hub-error";
      if (reason === "not-signed-for-this-org") {
        // The machinery stays out of the way: what it means, and the fix.
        return { title: "This app is not signed by " + nameOf() + " yet", body: "Release it from this browser, which signs it for " + nameOf() + ", or from Krate Studio. Nothing was released.", action: null };
      }
      return K.refusal(reason, { org: nameOf(), slug: slug, here: C.here || "/orgs/", message: res.body && res.body.message, what: what });
    }

    // =========================================================== first app

    /// Pick a ready app or a file, then one button releases it. `o.onDone`
    /// gets (release, label) once it is released; `o.onWaiting(label)`
    /// when the two-person rule holds it for someone else.
    function firstApp(box, o) {
      o = o || {};
      box.textContent = "";
      var wrap = el("div", { class: "fa" });
      box.appendChild(wrap);
      if (o.title !== false) {
        wrap.appendChild(el("div", { class: "fa-h" }, [
          el(o.h || "h2", { text: "Give " + nameOf() + " its first app" }),
          el("p", { text: "Pick one and " + nameOf() + " can use it in a few seconds. You can add your own apps any time." }),
        ]));
      }
      // With more than a handful of ready apps: a row of kinds and a search
      // above them, so the one wanted is two clicks away, not a scroll.
      var filter = el("div", { class: "st-filter", hidden: true });
      wrap.appendChild(filter);
      var list = el("div", { class: "starters", role: "radiogroup", "aria-label": "Ready apps" }, [el("p", { class: "loading", text: "Loading the ready apps" })]);
      wrap.appendChild(list);
      var none = el("p", { class: "sub st-none", hidden: true });
      wrap.appendChild(none);
      var why = el("p", { class: "fa-why", hidden: true });
      wrap.appendChild(why);
      var go = el("button", { class: "btn dark fa-go", type: "button", text: "Release to " + nameOf() });
      var file = el("input", { type: "file", id: "faFile-" + slug, class: "vh", accept: ".krate,application/octet-stream", "aria-label": "Upload a .krate you have" });
      var pick = el("label", { class: "btn ghost", for: "faFile-" + slug, text: "Upload a .krate you have" });
      wrap.appendChild(el("div", { class: "fa-acts" }, [go, pick, file]));
      var status = el("p", { class: "fa-step", role: "status", "aria-live": "polite", hidden: true });
      var out = el("div", { class: "fa-out" });
      wrap.appendChild(status);
      wrap.appendChild(out);

      var chosen = null; // {kind: "starter", s} | {kind: "file", f}
      var keys = {};     // one Idempotency-Key per chosen thing, kept across retries
      var cards = [];
      function label() { return chosen ? (chosen.kind === "starter" ? chosen.s.name : chosen.f.name.replace(/\.krate$/i, "")) : "it"; }
      function choose(c, card) {
        chosen = c;
        cards.forEach(function (x) { x.setAttribute("aria-checked", x === card ? "true" : "false"); x.setAttribute("tabindex", x === card ? "0" : "-1"); });
        go.textContent = "Release " + label() + " to " + nameOf();
        go.disabled = false;
        why.textContent = c.kind === "starter" && c.s.why ? c.s.why : "";
        why.hidden = !why.textContent;
        out.textContent = "";
      }
      /// One choice: a picture of the app in use (or a file's tile), its
      /// name, what it is for in a line, and its three best things. The
      /// whole list of what it can do stays on the button for a reader.
      function card(c, title, says, top, tag, shot, can) {
        var b = el("button", { class: "starter" + (shot ? " has-shot" : ""), type: "button", role: "radio", "aria-checked": "false", "aria-description": can || null }, [
          shot
            ? el("span", { class: "st-shot", "aria-hidden": "true" }, [el("img", { src: shot, alt: "", loading: "lazy", decoding: "async" })])
            : el("span", { class: "st-tile", "aria-hidden": "true", text: title.slice(0, 1).toUpperCase() }),
          el("span", { class: "st-t" }, [
            el("span", { class: "st-name" }, [
              el("b", {}, [title, tag ? el("span", { class: "tag", text: tag }) : null]),
              el("span", { class: "st-tick", "aria-hidden": "true" }),
            ]),
            el("small", { text: says }),
            top && top.length ? el("ul", { class: "st-top" }, top.map(function (t) { return el("li", { text: t }); })) : null,
          ]),
        ]);
        b.addEventListener("click", function () { choose(c, b); });
        // A radio group: arrow keys move the choice, Tab leaves the group.
        b.addEventListener("keydown", function (e) {
          var k = e.key, i = cards.indexOf(b);
          // Only the ones on show: a kind or a search hides the rest.
          var shown = cards.filter(function (x) { return !x.hidden; });
          i = shown.indexOf(b);
          var to = k === "ArrowRight" || k === "ArrowDown" ? i + 1 : k === "ArrowLeft" || k === "ArrowUp" ? i - 1 : k === "Home" ? 0 : k === "End" ? shown.length - 1 : null;
          if (to === null || !shown.length) return;
          if (e.preventDefault) e.preventDefault();
          var next = shown[(to + shown.length) % shown.length];
          next.click();
          if (next.focus) next.focus();
        });
        cards.push(b);
        return b;
      }
      starters().then(function (all) {
        list.textContent = "";
        all.forEach(function (s) {
          var c = card({ kind: "starter", s: s }, s.name, s.says || "", s.top || (s.can || []).slice(0, 3), s.placeholder ? "Placeholder" : "", s.shot || "", (s.can || []).join(". "));
          c._starter = s;
          list.appendChild(c);
        });
        if (all.length > 6) kinds(all);
        // Decided for them: the first ready app is already picked.
        if (all.length) choose({ kind: "starter", s: all[0] }, cards[0]);
        else {
          list.appendChild(el("p", { class: "sub", text: "Upload a .krate you have, or make one in Krate Studio." }));
          go.disabled = true;
          go.textContent = "Choose a .krate to release";
        }
      });
      /// The kinds the ready apps come in (starters.json `category`), each a
      /// chip with how many, then a search over names and what they do.
      function kinds(all) {
        var cats = [];
        all.forEach(function (s) { if (s.category && cats.indexOf(s.category) < 0) cats.push(s.category); });
        var want = "", words = "";
        var chips = [];
        function apply() {
          var q = words.trim().toLowerCase(), shown = 0;
          cards.forEach(function (c) {
            var s = c._starter;
            if (!s) return; // a file of their own stays on show
            var hay = [s.name, s.says || "", (s.top || []).join(" "), (s.can || []).join(" "), s.category || ""].join(" ").toLowerCase();
            var on = (!want || s.category === want) && (!q || hay.indexOf(q) >= 0);
            c.hidden = !on;
            if (on) shown++;
          });
          chips.forEach(function (ch) { ch.setAttribute("aria-pressed", ch._cat === want ? "true" : "false"); });
          none.hidden = shown > 0;
          none.textContent = shown ? "" : "No ready app matches “" + words.trim() + "”. Clear the search, or upload a .krate you have.";
        }
        var row = el("div", { class: "st-kinds", role: "group", "aria-label": "Kinds of app" });
        [["", "All " + all.length]].concat(cats.map(function (c) {
          return [c, c + " " + all.filter(function (s) { return s.category === c; }).length];
        })).forEach(function (pair) {
          var ch = el("button", { class: "chip", type: "button", "aria-pressed": pair[0] ? "false" : "true", text: pair[1] });
          ch._cat = pair[0];
          ch.addEventListener("click", function () { want = pair[0]; apply(); });
          chips.push(ch);
          row.appendChild(ch);
        });
        var find = el("input", { class: "inp st-find", type: "search", placeholder: "Find an app", "aria-label": "Find a ready app", autocomplete: "off" });
        find.addEventListener("input", function () { words = find.value || ""; apply(); });
        filter.appendChild(row);
        filter.appendChild(find);
        filter.hidden = false;
      }
      file.addEventListener("change", function () {
        var f = file.files && file.files[0];
        if (!f) return;
        cards = cards.filter(function (x) { return !x.classList.contains("mine"); });
        var mine = card({ kind: "file", f: f }, f.name.replace(/\.krate$/i, ""), Math.max(1, Math.round(f.size / 1024)) + " KB, from this computer", null, "", "", "");
        mine.classList.add("mine");
        var old = list.querySelector(".mine");
        if (old && old !== mine) old.remove();
        list.appendChild(mine);
        choose({ kind: "file", f: f }, mine);
      });

      function step(t) { status.textContent = t; status.hidden = !t; }
      function fail(w, retry) {
        step("");
        go.disabled = false;
        go.textContent = "Release " + label() + " to " + nameOf();
        out.textContent = "";
        out.appendChild(K.refusalBlock(w, "bad"));
        if (retry === "studio") out.appendChild(studioPath());
      }
      function bytesOf(c) {
        if (c.kind === "file") return c.f.arrayBuffer().then(function (b) { return new Uint8Array(b); });
        return site(c.s.bundle).then(function (r) { return r.arrayBuffer(); }).then(function (b) { return new Uint8Array(b); });
      }

      go.addEventListener("click", function () {
        if (!chosen) return;
        var c = chosen, name = label();
        var id = c.kind === "starter" ? "s:" + c.s.id : "f:" + c.f.name + ":" + c.f.size;
        go.disabled = true;
        go.textContent = "Releasing " + name;
        out.textContent = "";
        step("Getting " + name + " ready");
        var dev = null;
        Promise.all([bytesOf(c), device()]).then(function (got) {
          var bytes = got[0];
          dev = got[1];
          if (!dev) {
            // No signing in this browser yet: a file already signed for the
            // organisation (by Krate Studio or the command line) still goes.
            if (c.kind === "file") return bytes;
            return Promise.reject({ studio: true });
          }
          step("Setting up " + nameOf() + "'s signature in this browser");
          // The browser names itself ("Chrome on Mac"); the organisation's
          // name is not a computer's name (K-1113).
          return Promise.resolve(dev.ready(slug, {})).then(function () {
            step("Signing " + name + " for " + nameOf());
            return dev.sign(slug, bytes, { name: nameOf() });
          });
        }).then(function (signed) {
          step("Sending " + name + " to " + nameOf());
          if (!keys[id]) keys[id] = K.key();
          return upload(signed, keys[id]);
        }).then(function (sent) {
          if (!sent.ok) return Promise.reject({ res: sent, what: "send apps" });
          var digest = sent.body && sent.body.digest;
          if (sent.body && sent.body.state === "released") return { ok: true, body: { release: { digest: digest, url: K.hub() + "/a/" + digest } } };
          // The hub says when the sender may not release it alone (someone
          // else can approve): it waits, with no refused attempt on record.
          if (sent.body && sent.body.solo_release === false) return { ok: false, body: { error: "self-approval" } };
          step("Releasing " + name);
          return K.api("POST", path("/candidates/" + digest + "/approve"), { accept: [] }, { key: K.key() }).then(function (r) { r.digest = digest; return r; });
        }).then(function (rel) {
          if (rel.ok) {
            step("");
            var release = (rel.body && rel.body.release) || {};
            if (o.onDone) o.onDone(release, name);
            return;
          }
          if (rel.body && rel.body.error === "self-approval") {
            step("");
            if (o.onWaiting) o.onWaiting(name);
            else fail({ title: name + " is waiting for a second person", body: "In " + nameOf() + " someone other than the sender approves each release. Invite a reviewer and they approve it in Waiting.", action: null });
            return;
          }
          return Promise.reject({ res: rel, what: "release apps" });
        }).catch(function (e) {
          if (e && e.studio) {
            fail({ title: "Releasing from this browser is not switched on yet", body: "Release " + name + " from Krate Studio on your computer, or upload a .krate that Studio made.", action: null }, "studio");
          } else if (e && e.res) {
            if (e.res.body && e.res.body.error === "not-signed-in") { K.forget(); root.location.reload(); return; }
            fail(words(e.res, e.what));
          } else if (e && e.reason === "not-holder") {
            fail({ title: nameOf() + " signs its apps on another computer", body: "Release " + name + " from Krate Studio on that computer. To sign here too, open Krate Studio there and add this browser.", action: null }, "studio");
          } else if (e && e.reason === "unsupported") {
            fail({ title: "This browser cannot sign apps", body: "Use a current Chrome, Edge, Firefox or Safari, or release it from Krate Studio.", action: null }, "studio");
          } else if (e && e.message === "missing") {
            fail({ title: name + " is not ready to download yet", body: "Upload a .krate you have instead, or skip this and add an app later.", action: null });
          } else {
            fail({ title: "Signing " + name + " did not finish", body: "Nothing was released. Press the button again.", action: null });
          }
        });
      });
      return wrap;
    }

    /// Krate Studio for whatever this browser cannot do yet: open it, or
    /// get it and come back.
    function studioPath() {
      var link = "krate://org/setup?org=" + encodeURIComponent(slug);
      return el("div", { class: "studio-path" }, [
        el("a", { class: "btn ghost sm", href: link, text: "Open Krate Studio" }),
        el("a", { class: "btn ghost sm", href: "/download/", text: "Get Krate Studio" }),
        el("small", { text: "No Krate Studio yet? Get it, then come back to this page." }),
      ]);
    }

    // ============================================================ released

    function released(box, release, label, o) {
      o = o || {};
      box.textContent = "";
      var url = release.url || (release.digest ? K.hub() + "/a/" + release.digest : "");
      // The link as a person reads it: the app's name and a short form of
      // the address; the whole address is what Copy puts on the clipboard.
      var short = url.replace(/^https?:\/\//, "").replace(/([0-9a-f]{6})[0-9a-f]{52}([0-9a-f]{6})/, "$1…$2");
      var link = el("a", { class: "lchip", href: url, title: url, "data-url": url }, [
        el("b", { text: label }),
        el("small", { text: short }),
      ]);
      var cp = el("button", { class: "btn dark", type: "button", text: "Copy link", onclick: function () { K.copy(url, cp); } });
      var invBox = el("div", { class: "rel-inv" });
      var kids = [
        el("div", { class: "done-mark", "aria-hidden": "true", text: "✓" }),
        el("h2", { tabindex: "-1", class: "rel-h", text: nameOf() + " can use " + label + " now" }),
        el("p", { text: "Anyone in " + nameOf() + " opens it from this link. Nobody else can, even with the link." }),
        el("div", { class: "linkbox" }, [link, cp]),
        el("p", { class: "fa-why", text: "Every version is signed by " + nameOf() + " and checked on each computer before it opens." }),
        release.solo
          ? el("p", { class: "sub", text: "You released it yourself because nobody else in " + nameOf() + " can approve yet. Once a reviewer joins, each new version waits for a second person." })
          : null,
        el("h3", { class: "rel-inv-h", text: "Invite your team" }),
        invBox,
        o.next ? el("div", { class: "rel-next" }, [el("a", { class: "btn link", href: o.next.href, text: o.next.label })]) : null,
      ];
      var wrap = el("div", { class: "rel" }, kids);
      box.appendChild(wrap);
      invite(invBox, { onSent: o.onSent });
      var h = wrap.querySelector(".rel-h");
      if (h && h.focus) { try { h.focus(); } catch (e) {} }
      return wrap;
    }

    // ============================================================== invite

    /// Paste addresses, pick what they can do, one button. Each address is
    /// invited on its own (the hub mails each one); what did not go stays
    /// in the box with the reason, so nothing typed is lost.
    function invite(box, o) {
      o = o || {};
      box.textContent = "";
      var can = K.invitable(C.role());
      var may = INVITE_ROLES.filter(function (r) { return can.indexOf(r) >= 0; });
      if (!may.length) {
        box.appendChild(el("p", { class: "sub", text: "An owner or an admin of " + nameOf() + " invites people." }));
        return box;
      }
      var draft = mem().draft || "";
      var ta = el("textarea", { class: "inp inv-ta", id: "bulkWho-" + slug, rows: "3", autocomplete: "off", autocapitalize: "none", spellcheck: "false", placeholder: "sam@acme.com, lee@acme.com", "aria-describedby": "bulkHint-" + slug });
      ta.value = draft;
      var sel = el("select", { class: "sel", id: "bulkRole-" + slug, "aria-label": "What they can do" });
      may.forEach(function (r) {
        var opt = el("option", { value: r, text: K.ROLE_PLAIN[r] });
        if (r === "recipient") opt.selected = true;
        sel.appendChild(opt);
      });
      var hint = el("small", { id: "bulkHint-" + slug, text: "Work emails, one per line or separated by commas. Each gets an email with one button to join." });
      var go = el("button", { class: "btn dark", type: "button", text: "Send invitations" });
      var out = el("div", { class: "inv-out", role: "status", "aria-live": "polite" });
      function sync() {
        var p = parseEmails(ta.value);
        var n = p.emails.length;
        go.disabled = n === 0;
        go.textContent = n > 1 ? "Send " + n + " invitations" : n === 1 ? "Send 1 invitation" : "Send invitations";
        remember({ draft: ta.value });
        return p;
      }
      ta.addEventListener("input", sync);
      sync();
      var keys = {}; // email+role -> Idempotency-Key: a retried send is the same invitation
      go.addEventListener("click", function () {
        var p = sync();
        if (!p.emails.length) return;
        var role = sel.value;
        go.disabled = true;
        out.textContent = "";
        var sum = el("p", { class: "inv-sum", text: "Inviting " + p.emails.length + (p.emails.length === 1 ? " person" : " people") });
        var rows = el("ul", { class: "inv-rows" });
        out.appendChild(sum);
        out.appendChild(rows);
        var left = [], sent = 0, mailedN = 0, i = 0;
        function next() {
          if (i >= p.emails.length) return finish();
          var email = p.emails[i++];
          go.textContent = "Inviting " + i + " of " + p.emails.length;
          var k = email + "/" + role;
          if (!keys[k]) keys[k] = K.key();
          return K.api("POST", path("/invites"), { email: email, role: role }, { key: keys[k] }).then(function (res) {
            var li = el("li", {});
            if (res.ok) {
              sent++;
              var mailed = res.body && res.body.emailed === true;
              if (mailed) mailedN++;
              li.appendChild(el("span", { class: "tag " + (mailed ? "ok" : "warn"), text: mailed ? "Emailed" : "Send the link" }));
              li.appendChild(el("span", { class: "who-e", text: email }));
              if (!mailed && res.body && res.body.join_url) {
                var b = el("button", { class: "btn ghost sm", type: "button", text: "Copy link" });
                b.addEventListener("click", function () { K.copy(res.body.join_url, b); });
                li.appendChild(b);
              }
            } else {
              if (res.body.error === "not-signed-in") { K.forget(); root.location.reload(); return; }
              left.push(email);
              var w = words(res, "invite people");
              li.appendChild(el("span", { class: "tag warn", text: "Not sent" }));
              li.appendChild(el("span", { class: "who-e", text: email + ": " + w.title }));
            }
            rows.appendChild(li);
            return next();
          });
        }
        function finish() {
          ta.value = left.concat(p.bad).join("\n");
          sync();
          var unsent = left.length + p.bad.length;
          var plural = function (n, one, many) { return n === 1 ? "1 " + one : n + " " + many; };
          var head = !sent ? "No invitation went."
            : mailedN === sent ? plural(sent, "invitation", "invitations") + " sent. They join with one click from the email."
              : (mailedN ? plural(mailedN, "invitation was", "invitations were") + " emailed. " : "") +
                plural(sent - mailedN, "link", "links") + " could not be emailed: copy " + (sent - mailedN === 1 ? "it" : "each one") + " and send it yourself.";
          sum.textContent = head + (unsent ? " " + (unsent === 1 ? "1 address is" : unsent + " addresses are") + " still in the box: fix " + (unsent === 1 ? "it" : "them") + " and send again." : "");
          if (sent) {
            var m = mem();
            remember({ invited: (m.invited || 0) + sent });
            if (o.onSent) o.onSent(sent);
          }
        }
        if (p.bad.length) {
          var li0 = el("li", {}, [el("span", { class: "tag warn", text: "Skipped" }), el("span", { class: "who-e", text: p.bad.join(", ") + (p.bad.length === 1 ? " is" : " are") + " not an email address" })]);
          rows.appendChild(li0);
        }
        next();
      });
      box.appendChild(el("div", { class: "inv" }, [
        el("label", { class: "fld", for: "bulkWho-" + slug }, [el("span", { text: "Who" }), ta, hint]),
        el("div", { class: "inv-row" }, [el("label", { class: "fld inv-role", for: "bulkRole-" + slug }, [el("span", { text: "They" }), sel]), go]),
        out,
      ]));
      return box;
    }

    // ============================================================= IT pack

    function save(name, bytes, type) {
      var blob = new root.Blob([bytes], { type: type || "application/octet-stream" });
      var a = doc.createElement("a");
      a.href = root.URL.createObjectURL(blob);
      a.download = name;
      doc.body.appendChild(a);
      a.click();
      setTimeout(function () { root.URL.revokeObjectURL(a.href); a.remove(); }, 4000);
    }

    var tableP = null;
    function policyTable() {
      if (!tableP) tableP = site("/deploy/policy-keys.json").then(function (r) { return r.json(); });
      return tableP;
    }

    /// The pack's three files for this organisation, as {name: bytes}.
    function packFiles() {
      var o = C.org() || {};
      return policyTable().then(function (table) {
        return root.KratePolicy.pack(table, { name: nameOf(), slug: slug, hub: K.hub(), roots: [{ name: nameOf(), key: o.root_key }] });
      });
    }

    function itPack(box, o) {
      o = o || {};
      box.textContent = "";
      var org = C.org() || {};
      var self = el("div", { class: "card soft pack-c" }, [
        el("b", { text: "People install it themselves" }),
        el("p", { text: "Each invitation email carries the download. Krate is free on Mac, Windows and Linux." }),
      ]);
      var dl = ((root.location && root.location.origin) || "https://krate.tech") + "/download/";
      var cp = el("button", { class: "btn ghost sm", type: "button", text: "Copy the download link" });
      cp.addEventListener("click", function () { K.copy(dl, cp); remember({ pack: "self" }); if (o.onDone) o.onDone(); });
      self.appendChild(el("div", { class: "row2" }, [cp]));

      var it = el("div", { class: "card soft pack-c" }, [
        el("b", { text: "IT installs it for everyone" }),
        el("p", { text: "Ready-made settings for your device management: these computers open " + nameOf() + "'s apps, signed by " + nameOf() + ", and nothing signed by anyone else." }),
      ]);
      if (!org.root_key) {
        it.appendChild(el("p", { class: "note", text: "Ready once " + nameOf() + " has released its first app, which gives it its signature." }));
      } else if (!root.KratePolicy) {
        it.appendChild(el("p", { class: "note", text: "The settings files did not load. Reload the page and try again." }));
      } else {
        var note = el("p", { class: "note", role: "status", hidden: true });
        var files = [
          ["Mac", "krate-policy-" + slug + ".mobileconfig", "application/x-apple-aspen-config", [
            "Jamf Pro: Computers, Configuration Profiles, Upload, then choose this file.",
            "Intune: Devices, macOS, Configuration, Create, Templates, Custom, then upload it.",
            "Assign it to the computers that use " + nameOf() + "'s apps, and install Krate from krate.tech/download.",
          ]],
          ["Windows", "krate-policy-" + slug + ".reg", "application/octet-stream", [
            "One test computer: double-click this file to add the settings.",
            "A fleet: Intune or Group Policy takes the Krate template (krate.admx and krate.adml from krate.tech/deploy/windows).",
            "In the template, set the same values this file holds; open it in Notepad to copy them.",
          ]],
          ["Any computer", "policy.toml", "application/toml", [
            "Linux: /etc/krate/policy.toml",
            "Mac: /Library/Application Support/Krate/policy.toml",
            "Windows: %ProgramData%\\Krate\\policy.toml",
          ]],
        ];
        var list = el("div", { class: "pack-files" });
        files.forEach(function (f) {
          var b = el("button", { class: "btn ghost sm", type: "button", text: f[0] === "Any computer" ? "Download policy.toml" : "Download for " + f[0] });
          b.addEventListener("click", function () {
            b.disabled = true;
            var was = b.textContent;
            b.textContent = "Making it";
            packFiles().then(function (all) {
              b.disabled = false;
              b.textContent = was;
              save(f[1], all[f[1]], f[2]);
              note.textContent = "Saved " + f[1] + ".";
              note.hidden = false;
              remember({ pack: "it" });
              if (o.onDone) o.onDone();
            }, function () {
              b.disabled = false;
              b.textContent = was;
              note.textContent = "The settings could not be made. Reload the page and press it again.";
              note.hidden = false;
            });
          });
          list.appendChild(el("div", { class: "pack-f" }, [
            el("div", { class: "pack-fh" }, [el("b", { text: f[0] }), b]),
            el("ol", {}, f[3].map(function (s) { return el("li", { text: s }); })),
          ]));
        });
        it.appendChild(list);
        it.appendChild(note);
      }
      box.appendChild(el("div", { class: "pack" }, [self, it]));
      return box;
    }

    // =========================================================== checklist

    /// Getting started, for the people who run the organisation. Each item
    /// ticks itself from what the hub says; the list can be hidden, and
    /// brought back.
    function checklist(box, D) {
      box.textContent = "";
      var role = C.role();
      if (role !== "owner" && role !== "admin") { box.hidden = true; return; }
      if (mem().hidden) {
        box.hidden = false;
        var back = el("button", { class: "btn link sm", type: "button", text: "Show Getting started" });
        back.addEventListener("click", function () { remember({ hidden: false }); checklist(box, D); });
        box.appendChild(el("div", { class: "cl-off" }, [back]));
        return;
      }
      box.hidden = false;
      var owner = role === "owner";
      var ask = function (p) { return K.api("GET", path(p)); };
      Promise.all([
        ask("/releases"), ask("/candidates"),
        owner ? ask("/sso") : Promise.resolve(null),
        owner ? ask("/saml") : Promise.resolve(null),
        ask("/keys"),
        K.api("GET", "/me"),
      ]).then(function (r) {
        var rels = r[0].ok && Array.isArray(r[0].body) ? r[0].body : [];
        var cands = r[1].ok && Array.isArray(r[1].body) ? r[1].body : [];
        var waitingC = cands.filter(function (c) { return c.state === "waiting-for-approval"; });
        var sso = r[2] && r[2].ok && r[2].body && r[2].body.issuer;
        var saml = r[3] && r[3].ok && r[3].body && r[3].body.idp;
        var devices = r[4].ok && r[4].body && Array.isArray(r[4].body.devices) ? r[4].body.devices : null;
        var meEmail = r[5].ok && r[5].body && r[5].body.user ? r[5].body.user.email : "";
        var domain = companyDomain(meEmail) || ((r[2] && r[2].ok && r[2].body && r[2].body.domain) || "");
        var members = (C.members() || []).filter(function (m) { return (m.state || "active") === "active"; });
        var m = mem();
        var computer = devices && devices.some(function (d) { return d.kind ? d.kind !== "browser" : !/browser/i.test(d.name || ""); });

        var items = [];
        items.push({
          id: "app", title: "Release your first app",
          done: rels.length > 0,
          said: rels.length ? K.appName(rels[0]) + " is out. Everyone in " + nameOf() + " can open it."
            : waitingC.length ? K.appName(waitingC[0]) + " is waiting for someone else to approve it."
              : "A ready app or your own .krate, released in a few seconds.",
          body: function (b) { firstApp(b, { title: false, onDone: function () { D.refresh(); }, onWaiting: function () { D.refresh(); } }); },
        });
        items.push({
          id: "team", title: "Invite your team",
          done: members.length > 1,
          waits: Boolean(m.invited),
          said: members.length > 1 ? members.length + " people are in " + nameOf() + "."
            : m.invited ? (m.invited === 1 ? "1 person is invited." : m.invited + " people are invited.") + " Waiting for them to join from the email."
              : "Paste their work emails. Each joins with one click.",
          body: function (b, up) { invite(b, { onSent: function () { var n = mem().invited || 0; up(false, (n === 1 ? "1 person is invited." : n + " people are invited.") + " Waiting for them to join from the email."); } }); },
        });
        if (owner) {
          items.push({
            id: "sso", title: domain ? "Company sign-in for " + domain : "Company sign-in",
            done: Boolean(sso || saml),
            said: sso || saml ? "Your people sign in with their work accounts." : "Your people sign in with the work accounts they already have.",
            body: function (b) { ssoChoices(b, D, domain); },
          });
        }
        items.push({
          id: "pack", title: "Get Krate onto your team's computers",
          done: Boolean(m.pack),
          said: m.pack === "it" ? "Your IT pack is downloaded." : m.pack === "self" ? "People install it from their invitation." : "They install it themselves, or IT installs it for everyone.",
          body: function (b, up) { itPack(b, { onDone: function () { up(true, mem().pack === "it" ? "Your IT pack is downloaded." : "People install it from their invitation."); } }); },
        });
        items.push({
          id: "studio", title: "Krate Studio on your computer",
          done: Boolean(computer),
          said: computer ? "Your computer can sign and send " + nameOf() + "'s apps." : "Build and send apps from your computer, and keep a second copy of " + nameOf() + "'s signature there.",
          body: function (b) { b.textContent = ""; b.appendChild(studioPath()); },
        });
        items.push({
          id: "ai", title: "Build an app with AI",
          done: rels.length > 1,
          said: "Say what your team needs; Krate Studio builds it in a few minutes.",
          body: function (b) { b.textContent = ""; b.appendChild(el("a", { class: "btn ghost sm", href: "/app/", text: "Build an app" })); },
        });

        var count = function () { return items.filter(function (i) { return i.done; }).length; };
        var hide = el("button", { class: "btn link sm", type: "button", text: "Hide" });
        hide.addEventListener("click", function () { remember({ hidden: true }); checklist(box, D); });
        var counter = el("p", {});
        var fill = el("span", {});
        var bar = el("div", { class: "cl-bar", role: "progressbar", "aria-label": "Getting started", "aria-valuemin": "0", "aria-valuemax": String(items.length) }, [fill]);
        function tally() {
          var n = count();
          counter.textContent = n + " of " + items.length + " done";
          bar.setAttribute("aria-valuenow", String(n));
          fill.setAttribute("style", "width:" + Math.round((n / items.length) * 100) + "%");
        }
        var list = el("ol", { class: "cl" });
        var togglers = [];
        // The first thing not done is open: one obvious next step.
        // A step waiting on other people (invited, not joined yet) is not it.
        var open = items.filter(function (i) { return !i.done && !i.waits; })[0];
        items.forEach(function (it) {
          var body = el("div", { class: "cl-b", hidden: true });
          var tick = el("span", { class: "cl-tick", "aria-hidden": "true" });
          var said = el("small", {});
          var sr = el("span", { class: "vh" });
          var btn = el("button", { class: "cl-row", type: "button", "aria-expanded": "false" }, [
            tick, el("span", { class: "cl-t" }, [el("b", { text: it.title }), said]), sr,
          ]);
          var li = el("li", { class: it === open ? "next" : "" }, [btn, body]);
          /// A step that finished here ticks in place: nothing jumps.
          function up(done, words) {
            it.done = done || it.done;
            tick.className = "cl-tick" + (it.done ? " on" : "");
            tick.textContent = it.done ? "✓" : "";
            sr.textContent = it.done ? "Done" : "Not done yet";
            li.className = it.done ? "done" : (it === open ? "next" : "");
            if (words) said.textContent = words;
            tally();
          }
          up(it.done, it.said);
          var drawn = false;
          function toggle(on) {
            // One step open at a time: the list stays short.
            if (on) togglers.forEach(function (t) { if (t !== toggle) t(false); });
            body.hidden = !on;
            btn.setAttribute("aria-expanded", on ? "true" : "false");
            if (on && !drawn) { drawn = true; it.body(body, up); }
          }
          btn.addEventListener("click", function () { toggle(body.hidden); });
          togglers.push(toggle);
          list.appendChild(li);
          // Compact (the Dashboard): every step one line and the next one
          // marked -- except a new organisation's first app, the one thing
          // it has to do, which stays open.
          if (it === open && (!D.compact || it === items[0])) toggle(true);
        });
        tally();
        box.appendChild(el("div", { class: "card cl-card" + (D.compact ? " compact" : ""), id: "gettingStarted" }, [
          el("div", { class: "cl-h" }, [el("div", {}, [el("h2", { text: "Getting started" }), counter]), hide]),
          bar,
          list,
        ]));
      });
    }

    /// Company sign-in, picked by provider; the Settings form opens with
    /// the provider and the domain already filled in.
    function ssoChoices(b, D, domain) {
      b.textContent = "";
      var pick = function (provider) {
        D.S.ssoPrefill = { provider: provider, domain: domain };
        D.show("settings");
      };
      b.appendChild(el("p", { class: "sub", text: domain
        ? "Which accounts do people at " + domain + " use? The form opens with " + domain + " filled in, and the domain check right below it."
        : "Which accounts do your people use? The form opens ready, with the domain check right below it." }));
      b.appendChild(el("div", { class: "chips" }, [
        ["google", "Google Workspace"], ["microsoft", "Microsoft"], ["okta", "Okta"], ["saml", "Other (SAML)"],
      ].map(function (p) { return el("button", { class: "chip", type: "button", text: p[1], onclick: function () { pick(p[0]); } }); })));
    }

    return {
      firstApp: firstApp, released: released, invite: invite, itPack: itPack, checklist: checklist,
      studioPath: studioPath, device: device, starters: starters, upload: upload, remember: remember, mem: mem,
    };
  };

  root.KrateOrgSetup.parseEmails = parseEmails;
  root.KrateOrgSetup.companyDomain = companyDomain;
})(typeof window !== "undefined" ? window : globalThis);
